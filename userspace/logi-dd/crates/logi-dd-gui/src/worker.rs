//! A background thread that owns the `Device`/`ViewModel` and does all
//! sysfs I/O, so the UI thread never blocks on a wheel that is slow (or a
//! rejected write that would otherwise stall the event loop).
//!
//! The UI thread sends `Request`s over an `mpsc` channel; this thread runs
//! the blocking call and hands the result back via a callback the caller
//! supplies at `spawn` time. That callback runs *on this worker thread*, so
//! it must hop back to the UI thread itself (`slint::invoke_from_event_loop`)
//! before touching any Slint model or window.

use std::sync::mpsc;
use std::thread;

use logi_dd_core::sysfs::RealSysfs;
use logi_dd_core::{Category, Device, Mode};

use crate::viewmodel::{Row, ViewModel, WidgetInput};

/// A request from the UI thread. Every variant that touches the device
/// blocks on sysfs I/O; that is the whole reason this thread exists.
pub enum Request {
    LoadCategory(Category),
    /// Re-read a category without writing anything (e.g. after a mode
    /// change made elsewhere). Not sent yet: only one category is wired up,
    /// and nothing in this task changes the mode out from under it.
    #[allow(dead_code)]
    Refresh(Category),
    Edit { category: Category, attr: String, input: WidgetInput },
    /// Not sent yet: the mode-switch control belongs to the Profiles / mode
    /// category, which a later task wires up.
    #[allow(dead_code)]
    SetMode(Mode),
}

/// What the worker sends back.
pub enum Response {
    /// Fresh rows for `category`. `edit_error` is set when this follows a
    /// failed `Request::Edit`: `(attr, message)` for the row that failed, so
    /// the UI can show an inline error while every row's value reverts to
    /// what the device actually holds (from the same read).
    Rows { category: Category, rows: Vec<Row>, edit_error: Option<(String, String)> },
    /// The device could not be reached at all (no wheel bound, wrong
    /// permissions, ...). Sent once, from the very first request.
    Error(String),
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
            let vm = match Device::discover() {
                Ok(device) => ViewModel::new(device),
                Err(e) => {
                    on_response(Response::Error(e.to_string()));
                    // Keep draining so the UI thread's `Sender::send` calls
                    // never see a disconnected channel; there is simply no
                    // device for this session to act on.
                    for _req in rx {}
                    return;
                }
            };
            for req in rx {
                handle(&vm, req, &on_response);
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

fn handle(vm: &ViewModel<RealSysfs>, req: Request, on_response: &dyn Fn(Response)) {
    match req {
        Request::LoadCategory(category) | Request::Refresh(category) => {
            on_response(Response::Rows { category, rows: vm.rows_for(category), edit_error: None });
        }
        Request::Edit { category, attr, input } => {
            let edit_error = vm.edit(&attr, input).err().map(|e| (attr, e.to_string()));
            on_response(Response::Rows { category, rows: vm.rows_for(category), edit_error });
        }
        Request::SetMode(mode) => {
            // The caller does not learn the outcome here; it is expected to
            // follow up with its own Refresh/LoadCategory for whatever
            // category is on screen once the mode switch settles.
            let _ = vm.set_mode(mode);
        }
    }
}
