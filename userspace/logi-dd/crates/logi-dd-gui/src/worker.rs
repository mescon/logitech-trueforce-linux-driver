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
//!
//! While the request channel is idle the thread doubles as a drift watcher:
//! every `DRIFT_POLL_INTERVAL` it re-reads `wheel_profile`/`wheel_mode` and,
//! when either moved underneath the app (the wheel's physical profile
//! button, another tool writing sysfs), refreshes the page exactly like a
//! manual Refresh would; see `check_drift`.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::{Category, Device, DeviceInfo, Error, Mode, Value};

use crate::viewmodel::{Row, ViewModel, WidgetInput};

/// How long the worker waits for the next request before running one
/// external-change check (`check_drift`). The wheel's physical profile
/// button (and any other tool writing sysfs) changes settings without any
/// request passing through here; this is what keeps the pages honest about
/// it. Two sysfs reads per tick, and only while the channel is idle, so a
/// request in flight is never delayed by the watcher.
const DRIFT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// How long a "Try on wheel" run leaves the chosen LIGHTSYNC state on the
/// physical strip before restoring what was there. The hold runs on this
/// worker thread, so queued requests (and the drift watcher) wait it out;
/// that is deliberate: the UI disables the button while the try runs, and
/// nothing else should rewrite LED state mid-try anyway.
const LED_TRY_HOLD: Duration = Duration::from_secs(5);

/// The `wheel_led_effect` value that plays a stored CUSTOM slot (see
/// `logi_dd_core::lightsync`): the only selection whose try also plays a
/// rev sweep, since a slot's colours and direction are host-visible while
/// the built-in effects (1-4) are firmware-owned.
const CUSTOM_EFFECT: u8 = 5;

/// Pace of the try-on-wheel rev sweep: one `wheel_rev_level` step per this
/// interval. Must stay above the ~160 ms floor from the protocol docs
/// (faster bursts starve the wheel's shared HID++ command processor and
/// can cut FFB); 21 steps at 180 ms make the sweep run just under 4 s.
const REV_SWEEP_STEP: Duration = Duration::from_millis(180);

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
    /// Save the wheel's current settings as computer profile `name`
    /// (desktop-mode Profiles page). Replied with `Response::Profiles`.
    ProfileSave(String),
    /// Replay computer profile `name` onto the wheel. Replied with
    /// `Response::Profiles`, then fresh `Rows` for the Profiles category
    /// plus `Info` (an apply rewrites settings device-wide).
    ProfileApply(String),
    /// Delete computer profile `name`. Replied with `Response::Profiles`.
    ProfileDelete(String),
    /// The LIGHTSYNC page's "Try on wheel": apply `effect` (+ `slot` when
    /// the effect is the custom 5) to the physical strip, show it (a
    /// custom slot plays one animated rev sweep in the slot's colours and
    /// direction; a built-in effect holds for [`LED_TRY_HOLD`]), then
    /// restore the prior effect+slot state. Only LED state is written
    /// (nothing moves). Replied with `Response::LedTryDone`.
    LedTry { effect: u8, slot: u8 },
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
    /// The computer-side profile store's state: the saved names (sorted)
    /// plus the outcome line of the request that triggered this reply
    /// ("" for a plain page load). `error` says whether `status` reports
    /// a failure, so the UI can color it without string-sniffing. Sent
    /// after every profile request and alongside every Profiles-category
    /// `Rows` reply.
    Profiles { names: Vec<String>, status: String, error: bool },
    /// A `Request::LedTry` finished (the hold elapsed and the prior state
    /// was written back). `error` carries the first failure, if any; the
    /// restore is attempted even after a failed apply.
    LedTryDone { error: Option<String> },
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
            // What the drift watcher needs between requests: the category
            // the UI is looking at (tracked from the requests themselves,
            // starting at the UI's own startup default) and the last-seen
            // profile/mode pair.
            let mut on_screen = Category::ALL[0];
            let mut last_seen = drift_baseline(&vm);
            loop {
                let req = match rx.recv_timeout(DRIFT_POLL_INTERVAL) {
                    Ok(req) => req,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // Idle: nothing is in flight, so the watcher's two
                        // sysfs reads cannot delay a reply the UI awaits.
                        check_drift(&mut vm, &mut no_wheel_msg, &mut last_seen, on_screen, &on_response);
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                if matches!(req, Request::Discover) {
                    let (new_vm, resp) = discover_outcome(Device::discover());
                    vm = new_vm;
                    no_wheel_msg = no_wheel_message(&resp);
                    on_response(resp);
                    last_seen = drift_baseline(&vm);
                    continue;
                }
                if let Some(cat) = request_category(&req) {
                    on_screen = cat;
                }
                dispatch(&vm, &no_wheel_msg, req, &on_response);
                // The request itself may have moved the profile or mode (an
                // edit, a SetMode, a profile apply); resync the baseline so
                // the user's own change never reads as drift a tick later.
                last_seen = drift_baseline(&vm);
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

/// One drift observation: the active onboard profile slot (`None` on a
/// wheel without `wheel_profile`) and the current mode. See
/// `ViewModel::drift_snapshot` for the read semantics.
type DriftSnapshot = (Option<Value>, Mode);

/// The watcher's baseline right after a request settled (or discovery
/// ran): what the device reports now, or `None` when there is no device
/// (or it failed to answer, in which case the next idle tick's own read
/// decides what happens).
fn drift_baseline<S: SysfsIo>(vm: &Option<ViewModel<S>>) -> Option<DriftSnapshot> {
    vm.as_ref().and_then(|v| v.drift_snapshot().ok())
}

/// The category a request is about, for the drift watcher's notion of what
/// is on screen. `SetMode`/`Discover` carry none (and the UI keeps its own
/// category across both), so they leave the tracked value alone.
fn request_category(req: &Request) -> Option<Category> {
    match req {
        Request::LoadCategory(c) | Request::Refresh(c) => Some(*c),
        Request::Edit { category, .. } => Some(*category),
        Request::ProfileSave(_) | Request::ProfileApply(_) | Request::ProfileDelete(_) => {
            Some(Category::Profiles)
        }
        Request::LedTry { .. } => Some(Category::Leds),
        Request::SetMode(_) | Request::Discover => None,
    }
}

/// Run one "Try on wheel": remember the current effect+slot, write the
/// chosen ones (the slot first, then the effect: the driver re-applies the
/// slot's stored config on the transition to the custom effect), show
/// them (a custom slot plays one [`rev_sweep`] when the wheel exposes
/// `wheel_rev_level`; anything else holds for `hold`), then write the
/// prior state back. Restoring `wheel_led_effect` also exits any rev fill
/// back to the idle pattern. The restore runs even when the apply or the
/// sweep failed (a half-applied try must not stick); the first error
/// wins. Everything goes through the same `ViewModel::edit` path the
/// settings widgets use, so validation and mode gating apply as usual.
/// Pulled out of `handle` so tests can pass zero durations.
fn led_try_on_wheel<S: SysfsIo>(
    vm: &ViewModel<S>,
    effect: u8,
    slot: u8,
    hold: Duration,
    sweep_step: Duration,
) -> Result<(), Error> {
    let prior_effect = match vm.device_read("wheel_led_effect")? {
        Value::Int(n) => n.clamp(1, 9),
        _ => return Err(Error::Invalid),
    };
    let prior_slot = match vm.device_read("wheel_led_slot") {
        Ok(Value::Int(n)) => n.clamp(0, 4),
        _ => 0,
    };
    let applied = vm
        .edit("wheel_led_slot", WidgetInput::Slider(i64::from(slot)))
        .and_then(|()| vm.edit("wheel_led_effect", WidgetInput::Slider(i64::from(effect))));
    let shown = if applied.is_ok() {
        if effect == CUSTOM_EFFECT && vm.device_read("wheel_rev_level").is_ok() {
            rev_sweep(vm, sweep_step)
        } else {
            // Built-in effects (and wheels without a rev-level attribute)
            // keep the static apply-hold-restore.
            thread::sleep(hold);
            Ok(())
        }
    } else {
        Ok(())
    };
    let restored_slot = vm.edit("wheel_led_slot", WidgetInput::Slider(i64::from(prior_slot)));
    let restored_effect = vm.edit("wheel_led_effect", WidgetInput::Slider(i64::from(prior_effect)));
    applied.and(shown).and(restored_slot).and(restored_effect)
}

/// One animated rev sweep on the physical strip: `wheel_rev_level`
/// stepping 0..10..0, one step per `step` (kept above the ~160 ms pacing
/// floor by [`REV_SWEEP_STEP`]). The fill uses the active slot's colours
/// and follows its direction, so the user sees the slot as a live
/// animated fill. The caller restores `wheel_led_effect` afterwards,
/// which exits the fill back to the idle pattern.
fn rev_sweep<S: SysfsIo>(vm: &ViewModel<S>, step: Duration) -> Result<(), Error> {
    for level in (0..=10i64).chain((0..10).rev()) {
        vm.edit("wheel_rev_level", WidgetInput::Slider(level))?;
        thread::sleep(step);
    }
    Ok(())
}

/// One idle-tick external-change check. When the profile or mode moved
/// underneath the app (the wheel's profile button, another tool), reply
/// with the same fresh `Rows` (+ the Profiles list on that page) + `Info`
/// a manual `Refresh` would have produced, so no page keeps stale values.
/// A read error means the wheel went away between polls: drop the view
/// model and answer through the same no-wheel path a failed discovery
/// takes, so the banner shows and Retry works.
fn check_drift<S: SysfsIo>(
    vm: &mut Option<ViewModel<S>>,
    no_wheel_msg: &mut String,
    last_seen: &mut Option<DriftSnapshot>,
    category: Category,
    on_response: &dyn Fn(Response),
) {
    let Some(v) = vm.as_ref() else { return };
    match v.drift_snapshot() {
        Ok(snap) => {
            let drifted = last_seen.as_ref().is_some_and(|last| *last != snap);
            *last_seen = Some(snap);
            if !drifted {
                return;
            }
            on_response(Response::Rows { category, rows: v.rows_for(category) });
            if category == Category::Profiles {
                on_response(profiles_state(v));
            }
            if let Ok(info) = v.info() {
                on_response(Response::Info(info));
            }
        }
        Err(e) => {
            *vm = None;
            *no_wheel_msg = e.to_string();
            *last_seen = None;
            on_response(Response::NoWheel(no_wheel_msg.clone()));
        }
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

/// The `Response::Profiles` a plain Profiles-category load carries: the
/// list with no status line.
fn profiles_state<S: SysfsIo>(vm: &ViewModel<S>) -> Response {
    Response::Profiles { names: vm.profile_list(), status: String::new(), error: false }
}

fn handle<S: SysfsIo>(vm: &ViewModel<S>, req: Request, on_response: &dyn Fn(Response)) {
    match req {
        Request::LoadCategory(category) => {
            on_response(Response::Rows { category, rows: vm.rows_for(category) });
            // The Profiles page also renders the computer-side store;
            // ship its list with every page load so it is never stale.
            if category == Category::Profiles {
                on_response(profiles_state(vm));
            }
        }
        Request::Refresh(category) => {
            on_response(Response::Rows { category, rows: vm.rows_for(category) });
            if category == Category::Profiles {
                on_response(profiles_state(vm));
            }
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
            // A successful mode OR active-profile edit changes more than its
            // own row: a mode flip re-gates every mode-gated row (and the
            // header label), and an onboard profile switch rewrites the
            // effective settings across categories. Both follow up with the
            // fresh rows-plus-info a Refresh would have produced.
            let device_wide = matches!(attr.as_str(), "wheel_mode" | "wheel_profile") && error.is_none();
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
            // Only on success: a failed edit left the device alone, and a
            // Rows reload here would wipe the error message the RowUpdated
            // above just attached.
            if device_wide {
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
        Request::ProfileSave(name) => {
            let (status, error) = match vm.profile_save(&name) {
                Ok(()) => (format!("Saved '{}'.", name.trim()), false),
                Err(e) => (format!("Save failed: {e}"), true),
            };
            on_response(Response::Profiles { names: vm.profile_list(), status, error });
        }
        Request::ProfileDelete(name) => {
            let (status, error) = match vm.profile_delete(&name) {
                Ok(()) => (format!("Deleted '{name}'."), false),
                Err(e) => (format!("Delete failed: {e}"), true),
            };
            on_response(Response::Profiles { names: vm.profile_list(), status, error });
        }
        Request::ProfileApply(name) => {
            let (status, error) = match vm.profile_apply(&name) {
                Ok(errors) if errors.is_empty() => (format!("Applied '{name}'."), false),
                Ok(errors) => {
                    let (attr, msg) = &errors[0];
                    (
                        format!(
                            "Applied '{name}' with {} failed setting(s), first: {attr}: {msg}",
                            errors.len()
                        ),
                        true,
                    )
                }
                Err(e) => (format!("Apply failed: {e}"), true),
            };
            on_response(Response::Profiles { names: vm.profile_list(), status, error });
            // An apply rewrites settings device-wide (possibly the mode
            // too); refresh what the Profiles page shows plus the header.
            on_response(Response::Rows {
                category: Category::Profiles,
                rows: vm.rows_for(Category::Profiles),
            });
            if let Ok(info) = vm.info() {
                on_response(Response::Info(info));
            }
        }
        Request::LedTry { effect, slot } => {
            let error = led_try_on_wheel(vm, effect, slot, LED_TRY_HOLD, REV_SWEEP_STEP)
                .err()
                .map(|e| e.to_string());
            on_response(Response::LedTryDone { error });
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
            _ => panic!("expected Info"),
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
    fn wheel_profile_edit_also_refreshes_rows_and_info() {
        // An onboard profile switch rewrites effective settings across
        // categories, so a successful wheel_profile edit follows up the
        // same way a wheel_mode edit does.
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        fs.set("wheel_profile", "1");
        let vm = ViewModel::with_io(fs);

        let req = Request::Edit {
            category: Category::Profiles,
            attr: "wheel_profile".to_string(),
            input: WidgetInput::Slider(3),
        };
        let responses = responses(|on_response| handle(&vm, req, on_response));
        assert_eq!(responses.len(), 3);
        match &responses[0] {
            Response::RowUpdated { row, error, .. } => {
                assert_eq!(row.attr, "wheel_profile");
                assert!(error.is_none());
                assert_eq!(row.value, Some(Value::Int(3)));
            }
            _ => panic!("expected RowUpdated first"),
        }
        match &responses[1] {
            Response::Rows { category, .. } => assert_eq!(*category, Category::Profiles),
            _ => panic!("expected Rows second"),
        }
        assert!(matches!(&responses[2], Response::Info(_)));
    }

    #[test]
    fn failed_wheel_profile_edit_sends_only_the_row_update() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        fs.set("wheel_profile", "1");
        let vm = ViewModel::with_io(fs);

        let req = Request::Edit {
            category: Category::Profiles,
            attr: "wheel_profile".to_string(),
            input: WidgetInput::Slider(9), // out of the 0-5 slot range
        };
        let responses = responses(|on_response| handle(&vm, req, on_response));
        assert_eq!(responses.len(), 1, "a failed profile edit must not follow up with Rows/Info");
        match &responses[0] {
            Response::RowUpdated { row, error, .. } => {
                assert_eq!(row.attr, "wheel_profile");
                assert!(error.is_some());
                assert_eq!(row.value, Some(Value::Int(1)), "the rejected write must not land");
            }
            _ => panic!("expected RowUpdated"),
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

    // --- the computer-side profile store ---

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh, unique temp directory per test.
    fn tempdir() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "logi-dd-gui-worker-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn profile_vm(dir: &std::path::Path) -> ViewModel<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_strength", "80");
        fs.set("wheel_profile", "0");
        fs.set("wheel_profile_names", "1: A");
        let mut vm = ViewModel::with_io(fs);
        vm.set_profiles_dir(dir.to_path_buf());
        vm
    }

    #[test]
    fn loading_the_profiles_category_also_ships_the_computer_profile_list() {
        let dir = tempdir();
        let vm = profile_vm(&dir);
        vm.profile_save("race").unwrap();
        let replies =
            responses(|on_response| handle(&vm, Request::LoadCategory(Category::Profiles), on_response));
        assert_eq!(replies.len(), 2);
        assert!(matches!(&replies[0], Response::Rows { .. }));
        match &replies[1] {
            Response::Profiles { names, status, error } => {
                assert_eq!(names, &vec!["race".to_string()]);
                assert_eq!(status, "");
                assert!(!error);
            }
            _ => panic!("expected Profiles second"),
        }
        // Other categories do not grow a Profiles reply.
        let other = responses(|on_response| handle(&vm, Request::LoadCategory(Category::Ffb), on_response));
        assert_eq!(other.len(), 1);
    }

    #[test]
    fn profile_save_and_delete_reply_with_the_fresh_list() {
        let dir = tempdir();
        let vm = profile_vm(&dir);
        let responses = responses(|on_response| {
            handle(&vm, Request::ProfileSave("race".to_string()), on_response);
            handle(&vm, Request::ProfileSave("".to_string()), on_response);
            handle(&vm, Request::ProfileDelete("race".to_string()), on_response);
        });
        assert_eq!(responses.len(), 3);
        match &responses[0] {
            Response::Profiles { names, status, error } => {
                assert_eq!(names, &vec!["race".to_string()]);
                assert!(status.contains("Saved"), "status: {status}");
                assert!(!error);
            }
            _ => panic!("expected Profiles"),
        }
        match &responses[1] {
            Response::Profiles { names, status, error } => {
                assert_eq!(names, &vec!["race".to_string()], "invalid name saved nothing");
                assert!(status.contains("failed"), "status: {status}");
                assert!(error);
            }
            _ => panic!("expected Profiles"),
        }
        match &responses[2] {
            Response::Profiles { names, error, .. } => {
                assert!(names.is_empty());
                assert!(!error);
            }
            _ => panic!("expected Profiles"),
        }
    }

    #[test]
    fn profile_apply_follows_up_with_rows_and_info() {
        let dir = tempdir();
        let vm = profile_vm(&dir);
        vm.profile_save("race").unwrap();
        // Drift a setting so the apply has something to restore.
        vm.edit("wheel_strength", WidgetInput::Slider(10)).unwrap();
        let responses = responses(|on_response| {
            handle(&vm, Request::ProfileApply("race".to_string()), on_response)
        });
        assert_eq!(responses.len(), 3);
        match &responses[0] {
            Response::Profiles { status, error, .. } => {
                assert!(status.contains("Applied"), "status: {status}");
                assert!(!error);
            }
            _ => panic!("expected Profiles first"),
        }
        match &responses[1] {
            Response::Rows { category, .. } => assert_eq!(*category, Category::Profiles),
            _ => panic!("expected Rows second"),
        }
        assert!(matches!(&responses[2], Response::Info(_)));
        assert_eq!(vm.device_read("wheel_strength").unwrap(), Value::Percent(80));
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

    // --- the LIGHTSYNC try-on-wheel run ---

    #[test]
    fn led_try_restores_the_prior_effect_and_slot() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_led_effect", "1");
        fs.set("wheel_led_slot", "0");
        let vm = ViewModel::with_io(fs);
        // Zero durations: the test only cares about the write/restore
        // contract, not the hold or the sweep pacing the real request uses.
        led_try_on_wheel(&vm, 5, 2, Duration::ZERO, Duration::ZERO).unwrap();
        assert_eq!(vm.device_read("wheel_led_effect").unwrap(), Value::Int(1), "effect restored");
        assert_eq!(vm.device_read("wheel_led_slot").unwrap(), Value::Int(0), "slot restored");
    }

    #[test]
    fn led_try_on_a_custom_slot_plays_one_rev_sweep_between_apply_and_restore() {
        let fs = std::rc::Rc::new(FakeSysfs::new());
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_led_effect", "1");
        fs.set("wheel_led_slot", "0");
        fs.set("wheel_rev_level", "0");
        let vm = ViewModel::with_io(fs.clone());
        led_try_on_wheel(&vm, 5, 2, Duration::ZERO, Duration::ZERO).unwrap();
        // The exact sequence: apply (slot, then effect), ONE 0..10..0
        // sweep, restore (slot, then effect; the effect write exits the
        // fill back to the idle pattern).
        let mut expected = vec![
            ("wheel_led_slot".to_string(), "2".to_string()),
            ("wheel_led_effect".to_string(), "5".to_string()),
        ];
        expected.extend(
            (0..=10i64)
                .chain((0..10).rev())
                .map(|n| ("wheel_rev_level".to_string(), n.to_string())),
        );
        expected.push(("wheel_led_slot".to_string(), "0".to_string()));
        expected.push(("wheel_led_effect".to_string(), "1".to_string()));
        assert_eq!(fs.writes(), expected);
    }

    #[test]
    fn led_try_on_a_builtin_effect_writes_no_rev_levels() {
        // Built-in effects (1-4) keep the static apply-hold-restore: their
        // colours are firmware-owned, so a rev fill would show nothing of
        // the user's own choices.
        let fs = std::rc::Rc::new(FakeSysfs::new());
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_led_effect", "1");
        fs.set("wheel_led_slot", "0");
        fs.set("wheel_rev_level", "0");
        let vm = ViewModel::with_io(fs.clone());
        led_try_on_wheel(&vm, 3, 0, Duration::ZERO, Duration::ZERO).unwrap();
        assert!(
            fs.writes().iter().all(|(attr, _)| attr != "wheel_rev_level"),
            "no rev writes for a built-in effect: {:?}",
            fs.writes()
        );
    }

    #[test]
    fn led_try_on_a_custom_slot_without_a_rev_attr_falls_back_to_the_hold() {
        // A wheel (or driver) without `wheel_rev_level` still gets the
        // plain apply-hold-restore, with no error from the missing sweep.
        let fs = std::rc::Rc::new(FakeSysfs::new());
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_led_effect", "1");
        fs.set("wheel_led_slot", "0");
        let vm = ViewModel::with_io(fs.clone());
        led_try_on_wheel(&vm, 5, 2, Duration::ZERO, Duration::ZERO).unwrap();
        assert!(fs.writes().iter().all(|(attr, _)| attr != "wheel_rev_level"));
        assert_eq!(vm.device_read("wheel_led_effect").unwrap(), Value::Int(1), "effect restored");
    }

    #[test]
    fn led_try_restores_even_when_the_apply_fails() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_led_effect", "3");
        fs.set("wheel_led_slot", "0");
        fs.set_errno("wheel_led_effect", 5); // EIO on the effect write
        let vm = ViewModel::with_io(fs);
        let err = led_try_on_wheel(&vm, 5, 2, Duration::ZERO, Duration::ZERO);
        assert!(err.is_err(), "the apply failure is reported");
        // The slot write landed before the effect failed; the restore must
        // still put the prior slot back so nothing half-applied sticks.
        assert_eq!(vm.device_read("wheel_led_slot").unwrap(), Value::Int(0), "slot restored");
        assert_eq!(vm.device_read("wheel_led_effect").unwrap(), Value::Int(3), "effect untouched");
    }

    #[test]
    fn led_try_with_an_unreadable_effect_writes_nothing() {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_led_slot", "0");
        let vm = ViewModel::with_io(fs);
        assert!(
            led_try_on_wheel(&vm, 5, 2, Duration::ZERO, Duration::ZERO).is_err(),
            "no prior state to restore means no try at all"
        );
        assert_eq!(vm.device_read("wheel_led_slot").unwrap(), Value::Int(0), "slot untouched");
    }

    // --- the drift watcher ---

    use std::rc::Rc;

    /// A view model plus a second handle to its `FakeSysfs`, so a test can
    /// mutate attributes behind the vm's back (what the wheel's physical
    /// profile button looks like from here).
    fn drift_vm() -> (Rc<FakeSysfs>, Option<ViewModel<Rc<FakeSysfs>>>) {
        let fs = Rc::new(FakeSysfs::new());
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_profile", "1");
        fs.set("wheel_strength", "80");
        let vm = Some(ViewModel::with_io(fs.clone()));
        (fs, vm)
    }

    #[test]
    fn drift_tick_without_changes_stays_silent() {
        let (_fs, mut vm) = drift_vm();
        let mut msg = String::new();
        let mut last = drift_baseline(&vm);
        let replies = responses(|on_response| {
            check_drift(&mut vm, &mut msg, &mut last, Category::Ffb, on_response);
            check_drift(&mut vm, &mut msg, &mut last, Category::Ffb, on_response);
        });
        assert!(replies.is_empty(), "no drift, no responses");
        assert!(vm.is_some());
    }

    #[test]
    fn profile_drift_refreshes_rows_and_info_once() {
        let (fs, mut vm) = drift_vm();
        let mut msg = String::new();
        let mut last = drift_baseline(&vm);
        // The wheel's profile button fired: the slot AND an effective
        // setting move without any request passing through the worker.
        fs.set("wheel_profile", "3");
        fs.set("wheel_strength", "40");
        let replies = responses(|on_response| {
            check_drift(&mut vm, &mut msg, &mut last, Category::Ffb, on_response);
        });
        assert_eq!(replies.len(), 2);
        match &replies[0] {
            Response::Rows { category, rows } => {
                assert_eq!(*category, Category::Ffb);
                let strength = rows.iter().find(|r| r.attr == "wheel_strength").unwrap();
                assert_eq!(strength.value, Some(Value::Percent(40)), "rows must be re-read, not stale");
            }
            _ => panic!("expected Rows first"),
        }
        assert!(matches!(&replies[1], Response::Info(_)));
        // The baseline advanced with the emit: the next tick is quiet again.
        let more = responses(|on_response| {
            check_drift(&mut vm, &mut msg, &mut last, Category::Ffb, on_response);
        });
        assert!(more.is_empty());
    }

    #[test]
    fn mode_drift_on_the_profiles_page_recomposes_it_with_the_store_list() {
        let (fs, mut vm) = drift_vm();
        let mut msg = String::new();
        let mut last = drift_baseline(&vm);
        fs.set("wheel_mode", "onboard");
        let replies = responses(|on_response| {
            check_drift(&mut vm, &mut msg, &mut last, Category::Profiles, on_response);
        });
        // Same shape as a manual Refresh of the Profiles page: rows, the
        // computer-side store list, then the header info (whose new mode is
        // what makes the composed page recompose).
        assert_eq!(replies.len(), 3);
        assert!(matches!(&replies[0], Response::Rows { category: Category::Profiles, .. }));
        assert!(matches!(&replies[1], Response::Profiles { .. }));
        match &replies[2] {
            Response::Info(info) => assert_eq!(info.mode, Mode::Onboard),
            _ => panic!("expected Info third"),
        }
    }

    #[test]
    fn drift_read_error_routes_through_the_no_wheel_path() {
        let (fs, mut vm) = drift_vm();
        let mut msg = String::new();
        let mut last = drift_baseline(&vm);
        // The wheel is gone: every attribute read now fails.
        fs.set_absent("wheel_mode");
        fs.set_absent("wheel_profile");
        fs.set_absent("wheel_strength");
        let replies = responses(|on_response| {
            check_drift(&mut vm, &mut msg, &mut last, Category::Ffb, on_response);
        });
        assert_eq!(replies.len(), 1);
        assert!(matches!(&replies[0], Response::NoWheel(_)));
        assert!(vm.is_none(), "the vm is dropped so requests answer NoWheel until a retry");
        assert!(!msg.is_empty(), "later requests re-answer with this message");
    }

    #[test]
    fn drift_tick_without_a_wheel_is_a_no_op() {
        let mut vm: Option<ViewModel<FakeSysfs>> = None;
        let mut msg = "no wheel bound".to_string();
        let mut last = None;
        let replies = responses(|on_response| {
            check_drift(&mut vm, &mut msg, &mut last, Category::Ffb, on_response);
        });
        assert!(replies.is_empty(), "retry is user-driven; the watcher never spams NoWheel");
    }

    #[test]
    fn combined_pedals_edit_after_mode_switch_keeps_mode_ok() {
        // The "Wrong mode" trap: wheel starts onboard (Combined pedals is
        // DesktopOnly, so its row is gated), the user presses the row's
        // Switch mode button (Request::SetMode + Refresh, exactly what the
        // GUI sends), then toggles Combined pedals ON. The edit's
        // RowUpdated must carry mode_ok=true: the device is in desktop
        // mode and the write landed.
        //
        // Guards the whole request pipeline against ever re-gating a row
        // whose write just succeeded. The live bug this pins down was NOT
        // in this pipeline: the kernel driver misparsed the wheel's
        // combined-pedals change notification (0x80D0, fn0 sw0, value in
        // the profile byte) as "profile 1 -> onboard", so wheel_mode
        // transiently READ as onboard right after the write and this
        // pipeline faithfully reported it. Fixed at the source in
        // mainline/hid-logitech-hidpp.c (fn gate restored); this test
        // keeps the front-end half of the contract honest.
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "onboard");
        fs.set("wheel_combined_pedals", "0");
        let vm = ViewModel::with_io(fs);

        // Onboard: the row is correctly gated.
        let gated = vm
            .rows_for(Category::Pedals)
            .into_iter()
            .find(|r| r.attr == "wheel_combined_pedals")
            .unwrap();
        assert!(!gated.mode_ok, "onboard: the DesktopOnly row must be gated");

        // The Switch mode button's path: SetMode then a Refresh.
        let replies = responses(|on_response| {
            handle(&vm, Request::SetMode(Mode::Desktop), on_response);
            handle(&vm, Request::Refresh(Category::Pedals), on_response);
        });
        match &replies[0] {
            Response::Info(info) => assert_eq!(info.mode, Mode::Desktop),
            _ => panic!("expected Info from SetMode"),
        }

        // Toggle Combined pedals ON.
        let req = Request::Edit {
            category: Category::Pedals,
            attr: "wheel_combined_pedals".to_string(),
            input: WidgetInput::Switch(true),
        };
        let replies = responses(|on_response| handle(&vm, req, on_response));
        assert_eq!(replies.len(), 1);
        match &replies[0] {
            Response::RowUpdated { row, error, .. } => {
                assert_eq!(row.attr, "wheel_combined_pedals");
                assert!(error.is_none(), "the write must succeed in desktop mode: {error:?}");
                assert_eq!(row.value, Some(Value::Bool(true)));
                assert!(
                    row.mode_ok,
                    "desktop mode holds after the edit; the row must not re-gate itself"
                );
            }
            _ => panic!("expected RowUpdated"),
        }
    }

    #[test]
    fn request_category_tracks_what_is_on_screen() {
        assert_eq!(request_category(&Request::LoadCategory(Category::Leds)), Some(Category::Leds));
        assert_eq!(request_category(&Request::Refresh(Category::Ffb)), Some(Category::Ffb));
        assert_eq!(
            request_category(&Request::Edit {
                category: Category::Steering,
                attr: "wheel_range".to_string(),
                input: WidgetInput::Slider(540),
            }),
            Some(Category::Steering)
        );
        assert_eq!(request_category(&Request::ProfileSave("x".into())), Some(Category::Profiles));
        assert_eq!(request_category(&Request::SetMode(Mode::Desktop)), None);
        assert_eq!(request_category(&Request::Discover), None);
    }
}
