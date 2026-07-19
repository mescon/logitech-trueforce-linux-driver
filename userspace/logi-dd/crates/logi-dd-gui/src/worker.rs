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
//! waits for a `Request::Discover` to try again. The UI keeps its normal
//! shell (Setup/Test stay usable) with a banner and a Retry button; device
//! requests sent meanwhile (the sidebar stays navigable) are answered with
//! a fresh `NoWheel` rather than silently dropped, so the banner's message
//! never goes stale.

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
    /// Fresh rows for a whole `category` (`LoadCategory`, `Refresh`, the
    /// no-wheel screen's Retry, or the follow-up a mode switch sends). The
    /// UI rebuilds every row's widget from this, which is fine here since
    /// every row genuinely is fresh: a category switch, or a refresh that
    /// legitimately wants the device's current state reflected everywhere.
    Rows { category: Category, rows: Vec<Row> },
    /// The one row a `Request::Edit` touched, success or failure. `error`
    /// carries the edit's error message on failure (`None` on success); the
    /// row's own value always reflects the fresh read that follows the
    /// write attempt, so a failed edit's row shows the device's actual
    /// (reverted) value alongside `error`, not a stale local guess. Kept
    /// separate from `Rows` so the UI only ever updates that one row's
    /// widget in place instead of rebuilding the whole list on every edit,
    /// which used to destroy and recreate every control mid-interaction.
    RowUpdated { category: Category, row: Row, error: Option<String> },
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
            let mut no_wheel_msg = no_wheel_message(&resp);
            on_response(resp);
            for req in rx {
                if matches!(req, Request::Discover) {
                    let (new_vm, resp) = discover_outcome(Device::discover());
                    vm = new_vm;
                    no_wheel_msg = no_wheel_message(&resp);
                    on_response(resp);
                    continue;
                }
                dispatch(&vm, &no_wheel_msg, req, &on_response);
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

/// The message the last discovery failure carried, for re-answering device
/// requests that arrive while there is no wheel (see `dispatch`). Empty
/// after a successful discovery, in which case it is never sent.
fn no_wheel_message(resp: &Response) -> String {
    match resp {
        Response::NoWheel(msg) => msg.clone(),
        _ => String::new(),
    }
}

/// Route one non-`Discover` request: with a device, `handle` runs it; with
/// none, answer `NoWheel` (the shell stays fully navigable without a wheel
/// now, so category loads DO arrive in this state; a reply keeps the UI's
/// banner message fresh instead of leaving the request silently dropped).
fn dispatch<S: SysfsIo>(
    vm: &Option<ViewModel<S>>,
    no_wheel_msg: &str,
    req: Request,
    on_response: &dyn Fn(Response),
) {
    match vm {
        Some(v) => handle(v, req, on_response),
        None => on_response(Response::NoWheel(no_wheel_msg.to_string())),
    }
}

fn handle<S: SysfsIo>(vm: &ViewModel<S>, req: Request, on_response: &dyn Fn(Response)) {
    match req {
        Request::LoadCategory(category) => {
            on_response(Response::Rows { category, rows: vm.rows_for(category) });
        }
        Request::Refresh(category) => {
            on_response(Response::Rows { category, rows: vm.rows_for(category) });
            if let Ok(info) = vm.info() {
                on_response(Response::Info(info));
            }
        }
        Request::Edit { category, attr, input } => {
            // `vm.edit` either writes the new value or leaves the device
            // untouched; either way, re-reading `attr`'s own row afterwards
            // reports what the device actually holds now (the write, or the
            // unchanged prior value on failure), so `row` is never a stale
            // local guess. Only that one row goes back to the UI: the rest
            // of the category did not change, and resending the whole list
            // would make the UI rebuild every row's widget for a one-field
            // edit (see `Response::RowUpdated`'s doc comment).
            let error = vm.edit(&attr, input).err().map(|e| e.to_string());
            let mode_changed = attr == "wheel_mode" && error.is_none();
            // A slot rename's reply gets the active slot's row from the
            // SAME read sent ahead of it: the UI attributes the re-read
            // name to its per-slot cache via the last-known slot, and a
            // stale slot there would file the name under the wrong entry
            // (the poisoned-cache bug). Both rows coming from one
            // `rows_for` pass makes the pairing exact by construction.
            let mut slot_row = None;
            let mut edited_row = None;
            for row in vm.rows_for(category) {
                if attr == "wheel_led_slot_name" && row.attr == "wheel_led_slot" {
                    slot_row = Some(row);
                } else if row.attr == attr {
                    edited_row = Some(row);
                }
            }
            if let Some(row) = slot_row {
                on_response(Response::RowUpdated { category, row, error: None });
            }
            if let Some(row) = edited_row {
                on_response(Response::RowUpdated { category, row, error });
            }
            // A successful mode edit through the settings row changes more
            // than its own row: every mode-gated row's state and the
            // header's mode label. Follow up with the same fresh
            // rows-plus-info a SetMode/Refresh pair would have produced, so
            // the header toggle never computes its target from a stale
            // mode. Only on success: a failed edit left the mode alone, and
            // a Rows reload here would wipe the error message the
            // RowUpdated above just attached.
            if mode_changed {
                on_response(Response::Rows { category, rows: vm.rows_for(category) });
                if let Ok(info) = vm.info() {
                    on_response(Response::Info(info));
                }
            }
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
    use logi_dd_core::Value;
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
            Response::RowUpdated { .. } => panic!("expected Info, got RowUpdated"),
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
    fn device_requests_without_a_wheel_answer_no_wheel() {
        // The shell stays navigable with no wheel, so category loads keep
        // arriving; each must get a NoWheel reply (with the discovery
        // failure's message) instead of being silently dropped.
        let vm: Option<ViewModel<FakeSysfs>> = None;
        let responses = responses(|on_response| {
            dispatch(&vm, "no wheel bound", Request::LoadCategory(Category::Ffb), on_response);
            dispatch(&vm, "no wheel bound", Request::Refresh(Category::Info), on_response);
        });
        assert_eq!(responses.len(), 2);
        for r in &responses {
            match r {
                Response::NoWheel(msg) => assert_eq!(msg, "no wheel bound"),
                _ => panic!("expected NoWheel"),
            }
        }
    }

    #[test]
    fn dispatch_with_a_wheel_delegates_to_handle() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_strength", "80");
        let vm = Some(ViewModel::with_io(fs));
        let responses =
            responses(|on_response| dispatch(&vm, "", Request::LoadCategory(Category::Ffb), on_response));
        assert_eq!(responses.len(), 1);
        assert!(matches!(&responses[0], Response::Rows { .. }));
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
            Response::Rows { category, rows } => {
                assert_eq!(*category, Category::Ffb);
                assert!(rows.iter().any(|r| r.attr == "wheel_strength"));
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
    fn edit_failure_reports_a_row_update_with_the_error_attached() {
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
            Response::RowUpdated { category, row, error } => {
                assert_eq!(*category, Category::Ffb);
                assert_eq!(row.attr, "wheel_strength");
                assert!(error.is_some(), "expected the out-of-range edit to fail");
                // The rejected write never landed, so the row still shows
                // the device's actual (unchanged) value, not the invalid
                // input.
                assert_eq!(row.value, Some(Value::Percent(80)));
            }
            _ => panic!("expected RowUpdated"),
        }
    }

    #[test]
    fn wheel_mode_edit_also_refreshes_rows_and_info() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        let vm = ViewModel::with_io(fs);

        let req = Request::Edit {
            category: Category::Profiles,
            attr: "wheel_mode".to_string(),
            input: WidgetInput::Choice(1), // -> onboard
        };
        let responses = responses(|on_response| handle(&vm, req, on_response));
        assert_eq!(responses.len(), 3);
        match &responses[0] {
            Response::RowUpdated { row, error, .. } => {
                assert_eq!(row.attr, "wheel_mode");
                assert!(error.is_none());
            }
            _ => panic!("expected RowUpdated first"),
        }
        match &responses[1] {
            Response::Rows { category, .. } => assert_eq!(*category, Category::Profiles),
            _ => panic!("expected Rows second"),
        }
        match &responses[2] {
            Response::Info(info) => assert_eq!(info.mode, Mode::Onboard),
            _ => panic!("expected Info third"),
        }
    }

    #[test]
    fn failed_wheel_mode_edit_sends_only_the_row_update() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        let vm = ViewModel::with_io(fs);

        let req = Request::Edit {
            category: Category::Profiles,
            attr: "wheel_mode".to_string(),
            input: WidgetInput::Choice(9), // no such variant
        };
        let responses = responses(|on_response| handle(&vm, req, on_response));
        assert_eq!(responses.len(), 1, "a failed mode edit must not follow up with Rows/Info");
        match &responses[0] {
            Response::RowUpdated { row, error, .. } => {
                assert_eq!(row.attr, "wheel_mode");
                assert!(error.is_some());
            }
            _ => panic!("expected RowUpdated"),
        }
    }

    #[test]
    fn slot_name_edit_sends_the_active_slot_row_first_from_the_same_read() {
        // Regression: the UI files a rename's re-read name in a per-slot
        // cache keyed by the last-known active slot. The worker must send
        // the slot row (same rows_for pass) ahead of the name row, so the
        // pairing can never use a stale slot.
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_led_slot", "2");
        fs.set("wheel_led_slot_name", "OLD");
        let vm = ViewModel::with_io(fs);

        let req = Request::Edit {
            category: Category::Leds,
            attr: "wheel_led_slot_name".to_string(),
            input: WidgetInput::Text("RACE".into()),
        };
        let responses = responses(|on_response| handle(&vm, req, on_response));
        assert_eq!(responses.len(), 2);
        match &responses[0] {
            Response::RowUpdated { row, error, .. } => {
                assert_eq!(row.attr, "wheel_led_slot");
                assert_eq!(row.value, Some(Value::Int(2)));
                assert!(error.is_none(), "the slot row never carries the rename's error");
            }
            _ => panic!("expected the slot RowUpdated first"),
        }
        match &responses[1] {
            Response::RowUpdated { row, error, .. } => {
                assert_eq!(row.attr, "wheel_led_slot_name");
                assert_eq!(row.value, Some(Value::Text("RACE".into())));
                assert!(error.is_none());
            }
            _ => panic!("expected the name RowUpdated second"),
        }
    }

    #[test]
    fn other_edits_send_only_their_own_row() {
        // The slot pairing is rename-specific: a plain Leds edit (e.g. the
        // brightness) must not grow a second response.
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_led_slot", "2");
        fs.set("wheel_led_brightness", "80");
        let vm = ViewModel::with_io(fs);

        let req = Request::Edit {
            category: Category::Leds,
            attr: "wheel_led_brightness".to_string(),
            input: WidgetInput::Slider(70),
        };
        let responses = responses(|on_response| handle(&vm, req, on_response));
        assert_eq!(responses.len(), 1);
        match &responses[0] {
            Response::RowUpdated { row, .. } => assert_eq!(row.attr, "wheel_led_brightness"),
            _ => panic!("expected RowUpdated"),
        }
    }

    #[test]
    fn edit_success_reports_a_row_update_with_no_error() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_strength", "80");
        let vm = ViewModel::with_io(fs);

        let req = Request::Edit {
            category: Category::Ffb,
            attr: "wheel_strength".to_string(),
            input: WidgetInput::Slider(55),
        };
        let responses = responses(|on_response| handle(&vm, req, on_response));
        assert_eq!(responses.len(), 1);
        match &responses[0] {
            Response::RowUpdated { category, row, error } => {
                assert_eq!(*category, Category::Ffb);
                assert_eq!(row.attr, "wheel_strength");
                assert!(error.is_none());
                assert_eq!(row.value, Some(Value::Percent(55)));
            }
            _ => panic!("expected RowUpdated"),
        }
    }
}
