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
        // An optional test number re-runs just that one. Watching a rim
        // through eighteen tests to check a single number again is a poor
        // use of the one instrument this question has: the person looking.
        let only = std::env::args()
            .skip_while(|a| a != "--led-probe")
            .nth(1)
            .and_then(|a| a.parse::<u32>().ok());
        return led_probe(only);
    }
    if std::env::args().any(|a| a == "--launch-plan") {
        if std::env::args().any(|a| a == "--list") {
            return launch_plan_list();
        }
        let args: Vec<String> = std::env::args().collect();
        let named = args
            .iter()
            .position(|a| a == "--game")
            .and_then(|i| args.get(i + 1))
            .cloned();
        let appid = std::env::args()
            .skip_while(|a| a != "--launch-plan")
            .nth(1)
            .and_then(|a| a.parse::<u32>().ok());
        let wheel_arg = args
            .iter()
            .position(|a| a == "--wheel")
            .and_then(|i| args.get(i + 1))
            .cloned();
        return launch_plan(appid, named, wheel_arg);
    }
    if std::env::args().any(|a| a == "--report") {
        print!("{}", logi_wheel_core::diagnostics::report());
        return Ok(());
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

/// Manage the next attached wheel, wrapping round.
///
/// Rediscovers rather than trusting a remembered list: wheels get unplugged
/// mid-session, and switching to one that has gone is worse than staying
/// put. With a single wheel this says so rather than appearing to do
/// nothing.
fn next_wheel(app: &mut App<RealSysfs>) {
    let mut all = Device::discover_all();
    if all.len() < 2 {
        app.status = "only one wheel attached".to_string();
        return;
    }
    let current = app.device.sysfs_key();
    let at = all.iter().position(|d| d.sysfs_key() == current).unwrap_or(0);
    let next = (at + 1) % all.len();
    let label = all[next]
        .info()
        .ok()
        .map(|i| i.name)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            logi_wheel_core::device::short_model_label(all[next].model()).to_string()
        });
    app.device = all.remove(next);
    app.status = format!("managing {label}");
    // Everything downstream is per-wheel: the cached values, the rows the
    // views build from them, and the evdev node the monitor reads.
    app.reload();
    app.rescan_input();
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
        // Worked out here rather than in `App`, because the checks read the
        // real /sys and `App` is also driven by a fake one under test.
        // Cleared by `adopt_device`, so a wheel that goes away again gets a
        // fresh answer rather than the previous one.
        if app.no_wheel && app.diagnosis.is_empty() {
            app.diagnosis = logi_wheel_core::diagnose::diagnose();
        }
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
                Ok(Event::Key(k)) if k.kind == event::KeyEventKind::Press => {
                    app.on_key(k.code);
                    if std::mem::take(&mut app.next_wheel_requested) {
                        next_wheel(&mut app);
                    }
                }
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
    // Every attached wheel, not just the first one sysfs happened to
    // yield. A diagnostic that quietly describes one wheel on a two-wheel
    // rig is worse than one that refuses to run: the output looks complete.
    let wheels = Device::discover_all();
    if wheels.is_empty() {
        return Err(Box::new(logi_wheel_core::Error::NoWheel));
    }
    let models: Vec<_> = wheels.iter().map(|d| d.model()).collect();
    let labels = logi_wheel_core::device::short_labels(&models);
    for (i, device) in wheels.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("wheel: {}", labels.get(i).cloned().unwrap_or_else(|| format!("{:?}", device.model())));
        report_hidpp_features(device);
    }
    Ok(())
}

fn report_hidpp_features(device: &Device<logi_wheel_core::sysfs::RealSysfs>) {
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
            // What the wheel says it has, rather than what we thought to
            // ask about. Anything unnamed here is a feature this project
            // has never documented, which on a wheel whose LEDs nobody can
            // drive is the first place to look.
            if let Some(all) = device.hidpp_all_features() {
                let extra: Vec<String> = all
                    .iter()
                    .filter(|(_, _, name)| name.is_none())
                    .map(|(i, id, _)| format!("0x{id:04X}@0x{i:02X}"))
                    .collect();
                println!();
                println!("The wheel lists {} features in total.", all.len());
                if extra.is_empty() {
                    println!("None of them are undocumented here.");
                } else {
                    println!("Undocumented by this project: {}", extra.join(" "));
                }
            }
        }
    }
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
/// What a game needs on the wheel that is attached, as `key=value` lines
/// for `logi-launch` to act on.
///
/// The knowledge lives in the registry, which is tested and already drives
/// the apps' Setup page. A launch wrapper reimplementing any of it in shell
/// would be a second copy to drift, and the per-wheel half is exactly what
/// went wrong when the Setup page described the wrong wheel.
fn launch_plan(
    appid: Option<u32>,
    named: Option<String>,
    wheel_arg: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    use logi_wheel_core::games;

    // The advice is per wheel, so it needs the wheel actually attached.
    // With none, say so and claim nothing: a wrapper that guesses here
    // would set PROTON_ENABLE_HIDRAW on a G923 and cost its owner force
    // feedback.
    //
    // With SEVERAL, and of different kinds, the honest answer is that we
    // do not know which one the game will use. The game chooses that in
    // its own settings and never tells us. Taking the first one discovery
    // returns is a coin toss, and on a rig with a direct-drive wheel and a
    // G923 the losing side of that toss sets PROTON_ENABLE_HIDRAW on the
    // G923 and costs it force feedback. So the wheel can be named, and
    // when it is not and the answer is ambiguous, the harmful half is
    // withheld rather than guessed.
    let wheels = logi_wheel_core::Device::discover_all();
    if wheels.is_empty() {
        println!("wheel=none");
        return Ok(());
    }
    let mut kinds: Vec<bool> = wheels.iter().map(|d| d.wheel_caps().sdk_trueforce).collect();
    kinds.dedup();
    let forced = match wheel_arg.as_deref() {
        Some("dd") | Some("direct-drive") | Some("rs50") | Some("gpro") => Some(true),
        Some("g923") | Some("classic") => Some(false),
        Some(other) => {
            eprintln!("unknown --wheel {other:?}; use dd or g923");
            None
        }
        None => None,
    };
    let ambiguous = forced.is_none() && kinds.len() > 1;
    let caps = games::WheelCaps {
        sdk_trueforce: forced.unwrap_or_else(|| wheels[0].wheel_caps().sdk_trueforce),
    };
    if ambiguous {
        println!("wheel=mixed");
        println!("note=several kinds of wheel attached and the game picks one, not us");
        println!("note=name it with --wheel dd or --wheel g923 to get the full recipe");
    } else {
        println!("wheel={}", if caps.sdk_trueforce { "direct-drive" } else { "classic" });
    }

    // An unknown title still gets the daemon; see `LaunchPlan::unknown`.
    let unknown = || {
        println!("game=unknown");
        for line in games::LaunchPlan::unknown().lines() {
            println!("{line}");
        }
    };
    // A name the user gave wins over the appid. That is the whole point of
    // it: the appid may be missing (a non-Steam shortcut, whose id is
    // generated locally), or belong to a delisted title someone reinstalled
    // from a backup.
    let game = if let Some(want) = named.as_deref() {
        match games::compat_for_slug(want) {
            Ok(g) => Some(g),
            Err(candidates) => {
                if candidates.is_empty() {
                    eprintln!("no game matches {want:?}. Try --launch-plan --list");
                } else {
                    eprintln!("{want:?} matches several games: {}", candidates.join(", "));
                }
                unknown();
                return Ok(());
            }
        }
    } else {
        appid.and_then(games::compat_for_appid)
    };
    let Some(game) = game else {
        unknown();
        return Ok(());
    };
    println!("game={}", game.name);

    // Everything below this point is decided in logi-wheel-core, so the
    // wrapper's recipe and the GUI's description of it cannot disagree.
    // They did before: only this printer knew which wheel was attached.
    let mut plan = games::LaunchPlan::for_game(game, caps, ambiguous);
    // Name the wheel in PROTON_ENABLE_HIDRAW rather than saying "1", which
    // Proton reads as "every HID device on this machine" (issue #60). With
    // several wheels attached, scope to the one whose recipe this is.
    // Only a real wheel's id: logi-ffb's virtual wheel is a HID device too,
    // and while the proxy is running discovery can return it. Scoping the
    // variable to that would point Proton at a device the game's TrueForce
    // cannot reach, and the failure would be silent.
    // The id test is part of choosing the wheel, not a check applied after
    // choosing one. Applied afterwards, a first candidate that fails it (the
    // logi-ffb virtual wheel while the proxy runs, or any device whose
    // directory name is not in BUS:VID:PID.SEQ shape) yielded None for the
    // whole chain, and the wrapper fell back to the bare `1` this exists to
    // avoid. Keep looking instead.
    plan.hidraw_scope = wheels
        .iter()
        .filter(|d| d.wheel_caps().sdk_trueforce == caps.sdk_trueforce)
        .filter_map(|d| d.product_id())
        .find(|pid| {
            logi_wheel_core::device::DD_PIDS.contains(pid)
                || logi_wheel_core::device::G923_PIDS.contains(pid)
        })
        .map(|pid| format!("0x046D/0x{pid:04X}"));
    for line in plan.lines() {
        println!("{line}");
    }
    Ok(())
}

/// Every title the registry knows, with the name to pass to `--game` and
/// the launch line for it. Printed rather than kept only in the app so a
/// wiki page, an issue reply or a script can be built from the same source
/// as the app's own advice.
fn launch_plan_list() -> Result<(), Box<dyn std::error::Error>> {
    use logi_wheel_core::games::{self, Ffb, SimTf};
    let caps = match logi_wheel_core::Device::discover() {
        Ok(d) => d.wheel_caps(),
        Err(_) => games::WheelCaps::assumed(),
    };
    println!("# for a {} wheel", if caps.sdk_trueforce { "direct-drive" } else { "classic" });
    println!("{:<28} {:<9} {:<11} title", "--game", "appid", "settings");
    for g in games::GAMES {
        let slug = games::slug_for(g.name);
        let appid = games::appid_for(g.name).map(|a| a.to_string()).unwrap_or_else(|| "-".into());
        let mut bits: Vec<String> = Vec::new();
        match g.launch_options(caps) {
            Some(games::LAUNCH_HIDRAW) => bits.push("hidraw=1".into()),
            Some(games::LAUNCH_LOGI_FFB) => bits.push("ffb=proxy".into()),
            _ => {}
        }
        if g.ffb == Ffb::DirectInput && g.launch_options(caps).is_none() {
            bits.push("hidraw=0".into());
        }
        if g.setup_action(caps) == games::SetupAction::InstallShim {
            bits.push("tfsim=0".into());
        } else if let SimTf::LiveNow(id) = g.simulated_tf {
            bits.push("tfsim=1".into());
            if matches!(id, "acc" | "ac-evo" | "assetto" | "iracing" | "raceroom" | "rf2" | "lmu") {
                bits.push(format!("relay={id}"));
            }
        }
        let settings = if bits.is_empty() { "-".to_string() } else { bits.join(" ") };
        println!("{slug:<28} {appid:<9} {settings:<11} {}", g.name);
    }
    println!();
    println!("Use: logi-launch --game <name> %command%");
    println!("Or set your own in ~/.config/logi-wheel/games.conf, keyed by appid.");
    Ok(())
}

fn led_probe(only: Option<u32>) -> Result<(), Box<dyn std::error::Error>> {
    use logi_wheel_core::hidpp;
    use std::io::Write;
    use std::thread::sleep;
    use std::time::Duration;

    // Every attached wheel, each with every interface. Probing only the
    // first wheel found would leave the second one untested while the
    // output looked complete, which is the failure this whole command
    // exists to prevent.
    let wheels = Device::discover_all();
    if wheels.is_empty() {
        println!("No wheel found.");
        return Ok(());
    }
    let models: Vec<_> = wheels.iter().map(|d| d.model()).collect();
    let labels = logi_wheel_core::device::short_labels(&models);

    // Every interface, every dialect, numbered. The old version tried two
    // fixed guesses: the classic command on interface 0 and the 0x807A level
    // dialect on whichever interface declared HID++ report ids. On a wheel
    // laid out differently (the Xbox G923 has no 0xFF00 interface at all)
    // the second one silently had no target, so a dialect that had never
    // been sent got recorded as one the wheel ignores. Sweeping removes the
    // guess: a remote tester runs one command and reports one number.
    println!();
    println!("Each test lights the rev strip for 4 seconds, then turns it off,");
    println!("with a 2 second gap between tests. LEDs only: nothing here");
    println!("produces force feedback and the wheel will not move.");
    println!();
    println!("Watch the rim and note WHICH TEST NUMBER lights it.");
    println!();

    let hold = Duration::from_secs(4);
    let gap = Duration::from_secs(2);
    let mut n = 0;
    let mut sent = Vec::new();
    let mut first = true;

    for (wi, device) in wheels.iter().enumerate() {
        let Some(if0) = device.hid_dir() else { continue };
        let nodes = hidpp::all_wheel_nodes(if0);
        if nodes.is_empty() {
            continue;
        }
        let label = labels.get(wi).cloned().unwrap_or_else(|| format!("{:?}", device.model()));
        println!("== {label} ==");
        for (hid_dir, node) in &nodes {
        let kind = hidpp::descriptor_kind(hid_dir);

        // Dialect A: the classic lg4ff output report.
        n += 1;
        // Skip only THIS test, never the rest of the node's: a `continue`
        // here would jump past the level tests too, so `--led-probe 12`
        // would silently never run test 12.
        let run_classic = only.map_or(true, |o| o == n);
        if run_classic {
        if !first {
            sleep(gap);
        }
        first = false;
        print!("TEST {n}  {} [{kind}]  classic lg4ff ... ", node.display());
        std::io::stdout().flush().ok();
        if kind.contains("HID++") {
            // Report id 0xF8 means nothing on a HID++ interface. The write
            // is refused AND the refusal stalls the endpoint for several
            // seconds, so the next test on this node fails too and reads as
            // a result about that test rather than fallout from this one.
            // That produced one wrong conclusion already.
            println!("skipped, the classic command is not for a HID++ interface");
        } else {
            match hidpp::RealHidppIo::open(node) {
                Err(e) => println!("cannot open ({e})"),
                Ok(mut io) => {
                    let on = hidpp::rev_mask_via_lg4ff(&mut io, 0x1f);
                    sleep(hold);
                    let _ = hidpp::rev_mask_via_lg4ff(&mut io, 0x00);
                    match on {
                        Ok(()) => {
                            println!("sent");
                            sent.push(n);
                        }
                        Err(e) => println!("refused ({e})"),
                    }
                }
            }
        }
        }

        // Dialect B: the level-based 0x807A sequence, but only where this
        // interface actually answers HID++. Asking each interface rather
        // than assuming which one speaks it is the whole point.
        // Once per strip size. The level command states how many LEDs the
        // strip has, and a wheel told the wrong number lights nothing: the
        // direct-drive wheels want 10, a G923 wants 5, and every level
        // command this project sent before said 10 (issue #27).
        for leds in [hidpp::LEDS_DIRECT_DRIVE, hidpp::LEDS_G923] {
            n += 1;
            if only.is_some_and(|o| o != n) {
                continue;
            }
            if only.is_none() {
                sleep(gap);
            }
            print!("TEST {n}  {} [{kind}]  0x807A level, {leds} LEDs ... ", node.display());
            std::io::stdout().flush().ok();
            match hidpp::RealHidppIo::open(node) {
                Err(e) => println!("cannot open ({e})"),
                Ok(mut io) => match hidpp::resolve_feature_index(&mut io, 0x807A) {
                    None => println!("this interface does not answer HID++ for 0x807A"),
                    Some(idx) => {
                        let on = hidpp::rev_level_via_lightsync(&mut io, idx, leds, leds);
                        sleep(hold);
                        let _ = hidpp::rev_level_via_lightsync(&mut io, idx, 0, leds);
                        match on {
                            Ok(()) => {
                                println!("sent (feature index 0x{idx:02X})");
                                sent.push(n);
                            }
                            Err(e) => println!("refused ({e})"),
                        }
                    }
                },
            }
        }
    }
        println!();
    }

    println!();
    if sent.is_empty() {
        println!("Nothing could be sent at all. Try again with sudo.");
    } else {
        let list = sent.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ");
        println!("Reached the wheel: TEST {list}.");
        println!("That only means the bytes were accepted, NOT that the wheel obeyed.");
    }
    println!();
    println!("WHICH TEST NUMBER LIT THE STRIP? Reply with the number, or 'none'.");
    println!();
    println!("Please paste this whole output with your answer. The numbers depend");
    println!("on how many interfaces your wheel has, so the same number means");
    println!("different things on different wheels: on its own it cannot be read.");
    Ok(())
}

