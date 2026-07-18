//! A background thread that owns the `Device`/`ViewModel` and does all
//! sysfs I/O, so the UI thread never blocks on a wheel that is slow (or a
//! rejected write that would otherwise stall the event loop).
//!
//! The UI thread sends `Request`s over an `mpsc` channel; this thread runs
//! the blocking call and hands the result back via a callback the caller
//! supplies at `spawn` time. That callback runs *on this worker thread*, so
//! it must hop back to the UI thread itself (`slint::invoke_from_event_loop`)
//! before touching any Slint model or window.
//!
//! Discovery is not a one-shot gate any more: if `Device::discover()` fails
//! (no wheel bound, wrong permissions, unplugged mid-session, ...) the
//! worker keeps running with no device, reports `Response::NoWheel`, and
//! waits for a `Request::Discover` to try again. The UI turns that into the
//! no-wheel screen with a Retry button rather than a dead window.

use std::sync::mpsc;
use std::thread;

use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::{Category, Device, DeviceInfo, Error, Mode};

use crate::viewmodel::{Row, ViewModel, WidgetInput};

/// A request from the UI thread. Every variant that touches the device
/// blocks on sysfs I/O; that is the whole reason this thread exists.
pub enum Request {
    LoadCategory(Category),
    /// Re-read a category without writing anything (the refresh action, or
    /// the follow-up a mode switch needs once it settles). Also re-reads
    /// the device info shown in the header.
    Refresh(Category),
    Edit { category: Category, attr: String, input: WidgetInput },
    /// Switch the wheel between desktop and onboard mode. The caller does
    /// not learn the outcome here; it is expected to follow up with its own
    /// `Refresh` for whatever category is on screen once the mode switch
    /// settles.
    SetMode(Mode),
    /// (Re-)attempt `Device::discover()`. Sent once implicitly at startup
    /// and again whenever the no-wheel screen's Retry button is pressed.
    Discover,
}

/// What the worker sends back.
pub enum Response {
    /// Fresh rows for `category`. `edit_error` is set when this follows a
    /// failed `Request::Edit`: `(attr, message)` for the row that failed, so
    /// the UI can show an inline error while every row's value reverts to
    /// what the device actually holds (from the same read).
    Rows { category: Category, rows: Vec<Row>, edit_error: Option<(String, String)> },
    /// The device's identity/mode, for the header. Sent whenever discovery
    /// succeeds and after every `SetMode`/`Refresh`, so the mode shown in
    /// the header never drifts from what is actually on the wheel.
    Info(DeviceInfo),
    /// No device reachable right now (discovery failed, or has not been
    /// retried since the last failure). Carries the error text for display.
    NoWheel(String),
}

/// Handle to the worker thread. Cheap to clone (it is just a channel
/// sender), so each UI callback can hold its own copy.
#[derive(Clone)]
pub struct Worker {
    tx: mpsc::Sender<Request>,
}

impl Worker {
    /// Spawns the worker thread. `on_response` is called from the worker
    /// thread for every reply; it is responsible for getting back onto the
    /// UI thread before touching Slint state.
    pub fn spawn(on_response: impl Fn(Response) + Send + 'static) -> Worker {
        let (tx, rx) = mpsc::channel::<Request>();
        thread::spawn(move || {
            let (mut vm, resp) = discover_outcome(Device::discover());
            on_response(resp);
            for req in rx {
                if matches!(req, Request::Discover) {
                    let (new_vm, resp) = discover_outcome(Device::discover());
                    vm = new_vm;
                    on_response(resp);
                    continue;
                }
                // If there is no device, drop the request rather than
                // resend NoWheel per request (which would just repeat the
                // same message); the UI should only be sending requests
                // other than Discover once it has seen a non-NoWheel
                // response, so this is not expected in practice.
                if let Some(v) = &vm {
                    handle(v, req, &on_response);
                }
            }
        });
        Worker { tx }
    }

    /// Enqueue a request. The worker thread only ever exits by draining the
    /// channel to completion (see `spawn`), so a failed send would mean the
    /// thread panicked; there is nothing the UI can do about that here.
    pub fn request(&self, req: Request) {
        let _ = self.tx.send(req);
    }
}

/// Turn a `Device::discover()` result into the worker's next state plus the
/// response that announces it. Pulled out of `Worker::spawn` so it is
/// testable without a thread or a real hidraw device: a `FakeSysfs` device
/// wrapped in `Ok`, or a plain `Err`, stand in for what `discover()` would
/// have produced.
fn discover_outcome<S: SysfsIo>(result: Result<Device<S>, Error>) -> (Option<ViewModel<S>>, Response) {
    match result {
        Ok(device) => {
            let vm = ViewModel::new(device);
            match vm.info() {
                Ok(info) => (Some(vm), Response::Info(info)),
                // Discovery itself succeeded but the info read failed
                // (e.g. serial unreadable); treat it the same as no wheel
                // rather than show a header with blank fields. Discard the vm
                // so a subsequent Retry re-discovers cleanly.
                Err(e) => (None, Response::NoWheel(e.to_string())),
            }
        }
        Err(e) => (None, Response::NoWheel(e.to_string())),
    }
}

fn handle<S: SysfsIo>(vm: &ViewModel<S>, req: Request, on_response: &dyn Fn(Response)) {
    match req {
        Request::LoadCategory(category) => {
            on_response(Response::Rows { category, rows: vm.rows_for(category), edit_error: None });
        }
        Request::Refresh(category) => {
            on_response(Response::Rows { category, rows: vm.rows_for(category), edit_error: None });
            if let Ok(info) = vm.info() {
                on_response(Response::Info(info));
            }
        }
        Request::Edit { category, attr, input } => {
            let edit_error = vm.edit(&attr, input).err().map(|e| (attr, e.to_string()));
            on_response(Response::Rows { category, rows: vm.rows_for(category), edit_error });
        }
        Request::SetMode(mode) => {
            let _ = vm.set_mode(mode);
            if let Ok(info) = vm.info() {
                on_response(Response::Info(info));
            }
        }
        // Handled in `spawn`'s loop before `handle` is ever called, so that
        // it can replace `vm` itself; `handle` only ever borrows it.
        Request::Discover => unreachable!("Discover is intercepted before handle()"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use logi_dd_core::sysfs::FakeSysfs;
    use std::cell::RefCell;

    fn responses(f: impl FnOnce(&dyn Fn(Response))) -> Vec<Response> {
        let out = RefCell::new(Vec::new());
        f(&|r| out.borrow_mut().push(r));
        out.into_inner()
    }

    #[test]
    fn discover_failure_yields_no_wheel_and_no_view_model() {
        let responses = responses(|on_response| {
            let (vm, resp) = discover_outcome::<FakeSysfs>(Err(Error::NoWheel));
            assert!(vm.is_none());
            on_response(resp);
        });
        assert_eq!(responses.len(), 1);
        assert!(matches!(&responses[0], Response::NoWheel(_)));
    }

    #[test]
    fn discover_success_yields_info_and_a_view_model() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_serial", "ABC123");
        fs.set("wheel_firmware", "1.2.3");
        let device = Device::with_io(fs);

        let responses = responses(|on_response| {
            let (vm, resp) = discover_outcome(Ok(device));
            assert!(vm.is_some());
            on_response(resp);
        });
        assert_eq!(responses.len(), 1);
        match &responses[0] {
            Response::Info(info) => {
                assert_eq!(info.serial, "ABC123");
                assert_eq!(info.mode, Mode::Desktop);
            }
            Response::NoWheel(msg) => panic!("expected Info, got NoWheel({msg})"),
            Response::Rows { .. } => panic!("expected Info, got Rows"),
        }
    }

    #[test]
    fn discover_success_but_info_fails_yields_no_wheel_and_no_view_model() {
        let fs = FakeSysfs::new();
        // Don't set required fields; info() will fail when trying to read them.
        let device = Device::with_io(fs);

        let responses = responses(|on_response| {
            let (vm, resp) = discover_outcome(Ok(device));
            assert!(vm.is_none(), "vm should be discarded when info() fails");
            on_response(resp);
        });
        assert_eq!(responses.len(), 1);
        assert!(
            matches!(&responses[0], Response::NoWheel(_)),
            "should return NoWheel when info() fails"
        );
    }

    #[test]
    fn load_category_handle_reports_that_categorys_rows() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_strength", "80");
        let vm = ViewModel::with_io(fs);

        let responses = responses(|on_response| handle(&vm, Request::LoadCategory(Category::Ffb), on_response));
        assert_eq!(responses.len(), 1);
        match &responses[0] {
            Response::Rows { category, rows, edit_error } => {
                assert_eq!(*category, Category::Ffb);
                assert!(rows.iter().any(|r| r.attr == "wheel_strength"));
                assert!(edit_error.is_none());
            }
            _ => panic!("expected Rows"),
        }
    }

    #[test]
    fn refresh_also_sends_fresh_info() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        let vm = ViewModel::with_io(fs);

        let responses = responses(|on_response| handle(&vm, Request::Refresh(Category::Info), on_response));
        assert_eq!(responses.len(), 2);
        assert!(matches!(&responses[0], Response::Rows { .. }));
        match &responses[1] {
            Response::Info(info) => assert_eq!(info.mode, Mode::Onboard),
            _ => panic!("expected Info as the second response"),
        }
    }

    #[test]
    fn set_mode_switches_and_reports_the_new_mode() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        let vm = ViewModel::with_io(fs);

        let responses = responses(|on_response| handle(&vm, Request::SetMode(Mode::Onboard), on_response));
        assert_eq!(responses.len(), 1);
        match &responses[0] {
            Response::Info(info) => assert_eq!(info.mode, Mode::Onboard),
            _ => panic!("expected Info"),
        }
    }

    #[test]
    fn edit_failure_still_reports_rows_with_the_error_attached() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_strength", "80");
        let vm = ViewModel::with_io(fs);

        let req = Request::Edit {
            category: Category::Ffb,
            attr: "wheel_strength".to_string(),
            input: WidgetInput::Slider(999),
        };
        let responses = responses(|on_response| handle(&vm, req, on_response));
        assert_eq!(responses.len(), 1);
        match &responses[0] {
            Response::Rows { edit_error, .. } => {
                let (attr, _msg) = edit_error.as_ref().expect("edit should have failed");
                assert_eq!(attr, "wheel_strength");
            }
            _ => panic!("expected Rows"),
        }
    }
}
