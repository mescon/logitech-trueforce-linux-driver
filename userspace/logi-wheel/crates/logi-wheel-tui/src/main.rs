mod app;
mod color_picker;
mod curve_editor;
mod edit;
mod keymap;
mod ui;
mod wheel_test;

use app::App;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use logi_wheel_core::sysfs::RealSysfs;
use logi_wheel_core::Device;
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
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("logi-wheel {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // A one-shot diagnostic, not part of the app. It exists because a
    // wheel's capabilities are otherwise learned one bug report at a time:
    // the G923 Xbox edition takes the G920 code path, which never runs the
    // direct-drive feature discovery, so nothing here had ever asked that
    // wheel what it supports (issue #27).
    if std::env::args().any(|a| a == "--hidpp-features") {
        return hidpp_features();
    }
    if std::env::args().any(|a| a == "--led-probe") {
        return led_probe();
    }
    if std::env::args().any(|a| a == "--report") {
        return report();
    }

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
        // Pick up a just-finished sequence's summary (a no-op while none
        // has posted one since the last tick); the live per-step plan
        // itself needs no separate tick, since the draw below reads
        // `TestView::sim_progress` fresh every time.
        app.tick_sim_status();
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

/// Print which HID++ features the attached wheel implements.
///
/// Every line is a `Root.getFeature` read, the same transaction the Info
/// page already makes, so nothing on the wheel is changed. Needs read/write
/// access to the wheel's HID++ hidraw node, which this project's udev rules
/// grant; without them, run it with sudo.
fn hidpp_features() -> Result<(), Box<dyn std::error::Error>> {
    let device = Device::discover()?;
    println!("wheel: {:?}", device.model());
    match device.hidpp_features() {
        None => {
            println!();
            println!("No HID++ interface could be opened.");
            println!("Either this wheel has none, or the node is not readable:");
            println!("try again with sudo, or check the udev rules are installed.");
        }
        Some(rows) => {
            println!();
            for (id, what, index) in rows {
                match index {
                    Some(i) => println!("  0x{id:04X}  index 0x{i:02X}  {what}"),
                    None => println!("  0x{id:04X}  -           {what}"),
                }
            }
            println!();
            println!("A feature with an index is implemented by the wheel.");
        }
    }
    Ok(())
}

/// Try each known way of driving a wheel's rev strip, one at a time, and
/// let the person watching say which one worked.
///
/// Written because the feature map cannot answer this. The PlayStation G923
/// implements 0x807A and yet obeys the classic lg4ff command instead, so
/// "has LIGHTSYNC" does not imply "lights up when spoken to that way". On a
/// wheel nobody here owns, watching the rim is the only reliable evidence.
///
/// This WRITES to the wheel, unlike `--hidpp-features`. It only ever sends
/// LED commands: nothing here produces force, and every test turns the
/// lights off again afterwards.
fn led_probe() -> Result<(), Box<dyn std::error::Error>> {
    use logi_wheel_core::hidpp;
    use std::io::Write;
    use std::thread::sleep;
    use std::time::Duration;

    let device = Device::discover()?;
    let Some(if0) = device.hid_dir() else {
        println!("No wheel found.");
        return Ok(());
    };
    println!("wheel: {:?}", device.model());
    println!();
    println!("Each test lights the rev strip for 4 seconds, then turns it off.");
    println!("Watch the wheel. Note which test number lights it, if any.");
    println!("Nothing here produces force feedback.");
    println!();

    let hold = Duration::from_secs(4);
    let mut worked: Vec<&str> = Vec::new();

    // Test 1: the classic lg4ff output report, on the joystick interface.
    print!("TEST 1  classic lg4ff command ... ");
    std::io::stdout().flush().ok();
    match hidpp::open_joystick_node(if0) {
        None => println!("could not open the joystick interface (try sudo)"),
        Some(mut io) => {
            let all_on = hidpp::rev_mask_via_lg4ff(&mut io, 0x1f);
            sleep(hold);
            let _ = hidpp::rev_mask_via_lg4ff(&mut io, 0x00);
            match all_on {
                Ok(()) => {
                    println!("sent");
                    worked.push("1 (classic lg4ff)");
                }
                Err(e) => println!("send failed: {e}"),
            }
        }
    }

    // Test 2: the level-based 0x807A dialect, on the HID++ interface.
    print!("TEST 2  0x807A level dialect  ... ");
    std::io::stdout().flush().ok();
    match hidpp::probe_features(if0) {
        None => println!("no HID++ interface could be opened (try sudo)"),
        Some(rows) => {
            let lightsync = rows.iter().find(|(id, _, _)| *id == 0x807A).and_then(|(_, _, i)| *i);
            match lightsync {
                None => println!("this wheel does not implement 0x807A, so this one cannot work"),
                Some(idx) => match hidpp::find_hidpp_sibling(if0)
                    .and_then(|n| hidpp::RealHidppIo::open(&n).ok())
                {
                    None => println!("could not open the HID++ interface (try sudo)"),
                    Some(mut io) => {
                        let on = hidpp::rev_level_via_lightsync(&mut io, idx, 10);
                        sleep(hold);
                        let _ = hidpp::rev_level_via_lightsync(&mut io, idx, 0);
                        match on {
                            Ok(()) => {
                                println!("sent (feature index 0x{idx:02X})");
                                worked.push("2 (0x807A level dialect)");
                            }
                            Err(e) => println!("send failed: {e}"),
                        }
                    }
                },
            }
        }
    }

    println!();
    if worked.is_empty() {
        println!("Nothing could be sent. Run again with sudo, or report the errors above.");
    } else {
        println!("Sent successfully: {}", worked.join(", "));
        println!("A command being sent does NOT mean the wheel obeyed it.");
        println!("What matters is which test number actually lit the strip.");
    }
    Ok(())
}

/// Everything a bug report needs, in one pasteable block, with the parts
/// that identify a person left out.
///
/// This exists because the alternative advice is "paste your dmesg", and
/// that publishes the wheel's serial number: the driver logs it at probe,
/// and `wheel_serial` sits in sysfs next to the settings worth reading.
/// `wheel_profile_names` is worse, being whatever the owner called their
/// profiles. Neither helps diagnose anything, so neither is collected, and
/// the dmesg command suggested at the end filters the serial line out.
fn report() -> Result<(), Box<dyn std::error::Error>> {
    use std::fmt::Write as _;
    use std::fs;

    /// Read a file, trimmed, or None.
    fn slurp(p: impl AsRef<std::path::Path>) -> Option<String> {
        fs::read_to_string(p).ok().map(|s| s.trim().to_string())
    }

    // Settings whose VALUE is the owner's, not the wheel's. Their presence
    // is worth reporting; their contents are not.
    const WITHHELD: &[&str] = &["wheel_serial", "wheel_profile_names", "wheel_led_slot_name"];

    let mut out = String::new();
    writeln!(out, "## logitech-trueforce diagnostic report")?;
    writeln!(out)?;
    writeln!(out, "app        {}", env!("CARGO_PKG_VERSION"))?;
    writeln!(out, "kernel     {}", slurp("/proc/sys/kernel/osrelease").unwrap_or_default())?;
    writeln!(out, "module     {}",
             slurp("/sys/module/hid_logitech_dd/version")
                 .unwrap_or_else(|| "not loaded".into()))?;
    if let Some(os) = slurp("/etc/os-release") {
        if let Some(line) = os.lines().find(|l| l.starts_with("PRETTY_NAME=")) {
            writeln!(out, "distro     {}", line.trim_start_matches("PRETTY_NAME=").trim_matches('"'))?;
        }
    }

    writeln!(out, "\n### wheels")?;
    let base = std::path::Path::new("/sys/bus/hid/devices");
    let mut found = 0;
    if let Ok(entries) = fs::read_dir(base) {
        let mut names: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
        names.sort();
        for name in names {
            let n = name.to_string_lossy().into_owned();
            // Logitech wheels only; other HID devices are not ours to report.
            if !n.contains("046D:C2") {
                continue;
            }
            found += 1;
            let dir = base.join(&name);
            let drv = fs::read_link(dir.join("driver")).ok()
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "none".into());
            writeln!(out, "\n{n}  driver={drv}")?;
            let mut attrs: Vec<String> = fs::read_dir(&dir).ok()
                .map(|rd| rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|a| a.starts_with("wheel_") || a == "range")
                    .collect())
                .unwrap_or_default();
            attrs.sort();
            for a in attrs {
                if WITHHELD.contains(&a.as_str()) {
                    writeln!(out, "  {a:<26} <withheld: identifies you, not the wheel>")?;
                    continue;
                }
                if let Some(v) = slurp(dir.join(&a)) {
                    let v = v.replace('\n', " | ");
                    let v = if v.len() > 70 { format!("{}...", &v[..70]) } else { v };
                    writeln!(out, "  {a:<26} {v}")?;
                }
            }
        }
    }
    if found == 0 {
        writeln!(out, "  no Logitech wheel bound")?;
    }

    writeln!(out, "\n### simulated TrueForce config")?;
    let cfg = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
        .join("logi-wheel/tf-sim.conf");
    match slurp(&cfg) {
        Some(c) if !c.is_empty() => {
            for line in c.lines().filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty()) {
                writeln!(out, "  {line}")?;
            }
        }
        _ => writeln!(out, "  none (defaults apply)")?,
    }

    writeln!(out, "\n### udev rules installed")?;
    let mut any_rule = false;
    for d in ["/etc/udev/rules.d", "/usr/lib/udev/rules.d", "/lib/udev/rules.d"] {
        if let Ok(rd) = fs::read_dir(d) {
            for e in rd.filter_map(|e| e.ok()) {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.contains("logi") || n.contains("trueforce") {
                    writeln!(out, "  {d}/{n}")?;
                    any_rule = true;
                }
            }
        }
    }
    if !any_rule {
        writeln!(out, "  none found (force feedback and LEDs will need root)")?;
    }

    writeln!(out, "\n### kernel log")?;
    writeln!(out, "  Not readable without root. Add it with:")?;
    writeln!(out)?;
    writeln!(out, "    sudo dmesg | grep -i logitech | grep -v serial")?;
    writeln!(out)?;
    writeln!(out, "  The grep drops the line carrying your wheel's serial number.")?;
    writeln!(out, "  The lines worth having are \"HID++ features\", \"Effect timer\",")?;
    writeln!(out, "  and anything saying failed or error.")?;

    print!("{out}");
    Ok(())
}

#[cfg(test)]
mod report_tests {
    /// The withheld list is the whole point of the report existing rather
    /// than telling people to paste dmesg, so it is worth a test that says
    /// so. Each entry is a value that identifies the owner rather than the
    /// hardware: the wheel's serial number, and the names they gave their
    /// profiles and lighting slots.
    ///
    /// Add to it rather than removing: a field wrongly withheld costs a
    /// round trip in a bug report, a field wrongly published cannot be
    /// taken back.
    #[test]
    fn the_identifying_settings_stay_withheld() {
        const WITHHELD: &[&str] = &["wheel_serial", "wheel_profile_names", "wheel_led_slot_name"];
        let src = include_str!("main.rs");
        for field in WITHHELD {
            assert!(
                src.contains(&format!("\"{field}\"")),
                "{field} dropped out of the report's withheld list",
            );
        }
    }
}
