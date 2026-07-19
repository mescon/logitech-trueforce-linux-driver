mod app;
mod curve_editor;
mod edit;
mod ui;
mod wheel_test;

use app::App;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use logi_dd_core::sysfs::RealSysfs;
use logi_dd_core::Device;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::time::{Duration, Instant};

/// How long the idle loop waits for a key before running one external-
/// change check (`App::check_drift`): the wheel's physical profile button
/// changes settings without any key arriving, so blocking indefinitely on
/// input would leave stale values on screen. While the Test monitor's own
/// 33ms tick shortens the poll, drift checks stay capped to this cadence.
const DRIFT_POLL_TIMEOUT: Duration = Duration::from_secs(1);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // No wheel is not fatal: start the shell anyway (red header note,
    // Setup fully usable, the Info monitor's empty state) with a
    // placeholder device that reads as absent; `r` retries discovery.
    let (device, discover_error) = match Device::discover() {
        Ok(d) => (d, None),
        Err(e) => {
            (Device::with_io(RealSysfs::new(std::path::PathBuf::from("/nonexistent"))), Some(e))
        }
    };
    let mut app = App::new(device);
    if let Some(e) = discover_error {
        app.status = format!("{e} (r to retry)");
    }
    run(app)
}

fn run(mut app: App<RealSysfs>) -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    if let Err(e) = execute!(out, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    let mut term = match Terminal::new(CrosstermBackend::new(out)) {
        Ok(t) => t,
        Err(e) => {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            return Err(e.into());
        }
    };

    // Run the loop, capturing any error via `break` instead of `?`, so the
    // teardown below always runs and never leaves the terminal in raw mode.
    let mut last_drift_check = Instant::now();
    let res: Result<(), Box<dyn std::error::Error>> = loop {
        if let Err(e) = term.draw(|f| ui::draw(f, &app)) {
            break Err(e.into());
        }
        // While the Test view's monitor is live, poll with a short timeout
        // (so the loop keeps redrawing at ~30 Hz) and drain the wheel's
        // pending evdev events each tick; everywhere else, wait up to
        // `DRIFT_POLL_TIMEOUT` so an idle app still notices external
        // profile/mode changes instead of blocking on the next key forever.
        let timeout = if app.test_polling() {
            Duration::from_millis(33)
        } else if app.tf_sweep_active() {
            // A test sweep is playing: poll fast enough that its
            // completion (reaped below) shows up promptly.
            Duration::from_millis(250)
        } else {
            DRIFT_POLL_TIMEOUT
        };
        let key_ready = match event::poll(timeout) {
            Ok(ready) => ready,
            Err(e) => break Err(e.into()),
        };
        if key_ready {
            match event::read() {
                Ok(Event::Key(k)) if k.kind == event::KeyEventKind::Press => app.on_key(k.code),
                Ok(_) => {}
                Err(e) => break Err(e.into()),
            }
        }
        if app.test_polling() && !app.test.tick() {
            app.status = "test: wheel disconnected".to_string();
        }
        // Reap a finished test sweep (a no-op while none plays).
        app.tick_tf_sweep();
        // An idle tick (no key): check for external profile/mode drift, at
        // most once per `DRIFT_POLL_TIMEOUT` even while the monitor's 33ms
        // tick is driving the loop.
        if !key_ready && last_drift_check.elapsed() >= DRIFT_POLL_TIMEOUT {
            app.check_drift();
            last_drift_check = Instant::now();
        }
        // A queued re-discovery (r in the no-wheel state): a find swaps
        // the device in and reloads; a miss refreshes the status line.
        if app.take_retry_request() {
            match Device::discover() {
                Ok(d) => app.adopt_device(d),
                Err(e) => app.status = format!("{e} (r to retry)"),
            }
        }
        // A queued shim run blocks, so show a status line first, run,
        // rescan the games list (the row's shim status just changed),
        // then drop any keypresses that queued up meanwhile: a buffered
        // second 'i' would otherwise re-trigger the installer the moment
        // it finished.
        if let Some((args, verb)) = app.take_pending_shim() {
            app.status = format!("shim {verb}: running...");
            if let Err(e) = term.draw(|f| ui::draw(f, &app)) {
                break Err(e.into());
            }
            app.run_shim(&args, verb);
            app.scan_games();
            while let Ok(true) = event::poll(std::time::Duration::ZERO) {
                if event::read().is_err() {
                    break;
                }
            }
        }
        // A queued LIGHTSYNC try-on-wheel blocks for its 5 s hold, so it
        // runs here (after a draw showed the status line), then drops any
        // keypresses buffered meanwhile, same reasoning as the shim runs.
        if app.take_pending_led_try() {
            app.status = "try on wheel: showing the selected lighting for 5 s...".to_string();
            if let Err(e) = term.draw(|f| ui::draw(f, &app)) {
                break Err(e.into());
            }
            app.run_led_try(Duration::from_secs(5));
            while let Ok(true) = event::poll(std::time::Duration::ZERO) {
                if event::read().is_err() {
                    break;
                }
            }
        }
        if app.quit {
            break Ok(());
        }
    };

    // Always restore the terminal, regardless of how the loop ended.
    let _ = disable_raw_mode();
    let _ = execute!(term.backend_mut(), LeaveAlternateScreen);
    let _ = term.show_cursor();
    res
}
