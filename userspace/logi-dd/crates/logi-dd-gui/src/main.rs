slint::include_modules!();

mod bridge;
mod testio;
mod viewmodel;
mod worker;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use logi_dd_core::curve::Curve;
use logi_dd_core::evtest;
use logi_dd_core::{lightsync, shaping};
use logi_dd_core::{Category, Color, Mode, Value, REGISTRY};
use slint::Model as _;
use viewmodel::WidgetInput;
use worker::{Request, Response, Worker};

/// The curve editor's in-flight state: which attribute it is editing (so
/// `commit` knows what to write and which category to refresh) and the
/// `Curve` being shaped. Lives on the UI thread only; the worker never sees
/// it until `commit` sends the finished `Curve` as a `WidgetInput::Curve`.
struct CurveEditorState {
    attr: String,
    category: Category,
    curve: Curve,
}

/// The LIGHTSYNC slot editor's in-flight state: the staged strip colors
/// and animation direction for the ACTIVE custom slot (`slot`, 0-based,
/// only kept for the overlay title). Same UI-thread-only lifetime as
/// `CurveEditorState`; the worker never sees the staged parts until
/// `commit` sends them (colors, then direction, then the slot apply
/// trigger). The slot's name and per-slot brightness are NOT staged here:
/// both commit to the device immediately from their own callbacks, and
/// their display state lives in the `rgb-slot-name`/`rgb-slot-brightness`
/// Slint properties.
struct RgbEditorState {
    attr: String,
    category: Category,
    colors: Vec<Color>,
    /// Staged `wheel_led_direction` enum value (0-3); mirror-painting is
    /// active while this is inside-out (2) or outside-in (3).
    direction: u8,
}

/// The slot-text editor's in-flight state: which attribute it is editing
/// (so a write knows what to send and which category to refresh) and the
/// slot names currently shown. Unlike `CurveEditorState`/`RgbEditorState`,
/// nothing here is staged for a final commit: `Kind::SlotText` writes one
/// slot at a time, so each `set-slot-name` call sends its own
/// `WidgetInput::SlotText` immediately. `names` is only kept so the overlay
/// can be redrawn (optimistically) after each apply; lives on the UI thread
/// only, same rules as the other two editor states.
struct SlotTextEditorState {
    attr: String,
    category: Category,
    names: Vec<String>,
}

/// Push `curve`'s current shape to the Slint side: the composed plot line,
/// the draggable control points, and the two deadzone slider values. Called
/// after every curve-editor edit so the overlay's preview stays live.
fn push_curve_editor(app: &App, curve: &Curve) {
    app.set_curve_plot_commands(bridge::curve_plot_commands(curve).into());
    app.set_curve_control_points(bridge::curve_control_points(curve));
    app.set_curve_lower_deadzone(i32::from(curve.lower_deadzone()));
    app.set_curve_upper_deadzone(i32::from(curve.upper_deadzone()));
}

/// Push `colors`' current shape to the Slint side: the swatch list. Called
/// after every RGB-editor edit so the row of swatches stays live.
fn push_rgb_editor(app: &App, colors: &[Color]) {
    app.set_rgb_leds(bridge::rgb_leds_model(colors));
}

/// After swatch `index` was painted in the slot editor, copy it onto its
/// mirror pair when the staged direction is inside-out/outside-in (the
/// wheel plays those as 5 mirrored pairs; see `lightsync::mirrored`).
/// A no-op for the left/right sweeps, and for an index off the 10-LED
/// strip.
fn mirror_staged_swatch(state: &mut RgbEditorState, index: usize) {
    if !lightsync::mirrored(state.direction) || index >= 10 {
        return;
    }
    let pair = lightsync::mirror_index(index);
    if index < state.colors.len() && pair < state.colors.len() {
        state.colors[pair] = state.colors[index];
    }
}

/// Push `names`' current shape to the Slint side: the slot-name row list.
/// Called after every slot-text edit so the fields stay in sync with what
/// was just sent to the worker.
fn push_slot_text_editor(app: &App, names: &[String]) {
    app.set_slot_text_names(bridge::slot_names_model(names));
}

/// Read `wheel_profile_names`'s last-known value out of `known_values`
/// (populated from whichever `Response` last carried that attr; see the
/// worker-response closure in `main`). Returns an empty `Vec` when it has
/// not been read yet (app just started, or this wheel does not expose it):
/// `bridge::apply_profile_choices` still produces a full, usable dropdown
/// from an empty names list.
fn profile_names(known_values: &Arc<Mutex<HashMap<String, Value>>>) -> Vec<String> {
    match known_values.lock().unwrap().get("wheel_profile_names") {
        Some(Value::SlotNames(names)) => names.clone(),
        _ => Vec::new(),
    }
}

/// Read `wheel_led_slot`'s last-known value out of `known_values` (0 when
/// unread), for pairing with a `wheel_led_slot_name` value: the sysfs attr
/// only ever reads the ACTIVE slot's name, so the slot number says which
/// cache entry that name belongs to.
fn led_slot(known_values: &Arc<Mutex<HashMap<String, Value>>>) -> i32 {
    match known_values.lock().unwrap().get("wheel_led_slot") {
        Some(Value::Int(n)) => *n,
        _ => 0,
    }
}

/// Record the active slot's name in the per-slot cache `main` keeps for
/// the effect selector's CUSTOM labels. `slot`/`name` must come from the
/// same read (a whole-category reload, or a rename's reply paired with the
/// slot row the worker re-reads alongside it; see `worker::handle`); the
/// pure mapping (and its only-the-active-slot guarantee) lives in
/// `bridge::record_led_slot_name`.
fn cache_led_slot_name(cache: &Arc<Mutex<Vec<String>>>, slot: i32, name: &str) {
    bridge::record_led_slot_name(&mut cache.lock().unwrap(), slot, name);
}

/// Read `wheel_led_effect`'s last-known raw value out of `known_values`
/// (1 when unread), for the effect selector's labels and for resolving a
/// trailing raw-entry pick (see `lightsync::dropdown_labels`).
fn led_effect(known_values: &Arc<Mutex<HashMap<String, Value>>>) -> u8 {
    match known_values.lock().unwrap().get("wheel_led_effect") {
        Some(Value::Int(n)) => (*n).clamp(0, i32::from(u8::MAX)) as u8,
        _ => 1,
    }
}

/// Replace the persistent rows model's contents with a whole category's
/// worth of fresh rows (`LoadCategory`, `Refresh`, the no-wheel screen's
/// Retry, or a mode switch's follow-up refresh). `app.set_rows` is only
/// ever called once, at startup (`main`), installing a `VecModel` this
/// function and `update_row` mutate in place from then on; that way the
/// `SettingsList`'s repeater never sees the model itself change identity,
/// only its contents, which is what keeps a widget that is not part of
/// this reload (e.g. an open `ComboBox` popup) from being torn down for no
/// reason.
///
/// When the new content is the same length as what is already shown (the
/// common case: the same category re-read after an unrelated refresh),
/// each row is replaced in place via `set_row_data`, which only re-renders
/// rows whose value actually differs. A different length (a category
/// switch) falls back to replacing the whole content in one go: every row
/// is for a different setting there anyway, so there is nothing to
/// preserve.
fn load_rows(
    app: &App,
    rows: &[viewmodel::Row],
    profile_names: &[String],
    led_names: &[String],
    shaping_toggles: shaping::AxisToggles,
) {
    let items = bridge::setting_rows(rows);
    // A no-op for every category but Profiles (see `compose_profiles`'s
    // doc): onboard mode shows the slot picker + rename rows, desktop mode
    // keeps only the Mode row (the computer-side profile store renders
    // below the list instead).
    let items = bridge::compose_profiles(items, profile_names);
    // A no-op for every category but Leds (see `compose_lightsync`'s doc):
    // the LIGHTSYNC page renders three composed rows plus the Edit slot
    // button instead of the registry's raw row-per-attr list.
    let items = bridge::compose_lightsync(items, led_names);
    // A no-op for every category without shaping generators (see
    // `compose_shaping`'s doc): Steering and Pedals get a per-axis shaping
    // toggle row heading each axis block, and each axis shows either its
    // sensitivity row or its curve row, never both.
    let items = bridge::compose_shaping(items, shaping_toggles);
    let model = app.get_rows();
    if model.row_count() == items.len() {
        for (i, mut item) in items.into_iter().enumerate() {
            // A monotonic revision per push: widgets whose own binding was
            // severed by user interaction watch this with a `changed`
            // callback and re-assert their display from the row (see
            // `SettingRow`'s doc in `ui/widgets.slint`).
            item.revision = model.row_data(i).map_or(0, |r| r.revision.wrapping_add(1));
            model.set_row_data(i, item);
        }
        return;
    }
    match model.as_any().downcast_ref::<slint::VecModel<SettingRow>>() {
        Some(vec_model) => vec_model.set_vec(items),
        // Should not happen: `main` installs a `VecModel` before the first
        // response can arrive. Fall back to installing a fresh one rather
        // than silently dropping the reload.
        None => app.set_rows(slint::ModelRc::new(slint::VecModel::from(items))),
    }
}

/// Update just the row named by `row.attr` in place (a successful or
/// failed single-field edit), without touching any other row's widget.
/// `error` is the edit's failure message, or `None` on success; either way
/// `row` itself already reflects a fresh read (see `Response::RowUpdated`'s
/// doc comment), so this never has to guess at what reverting looks like.
///
/// An attr with no row in the model (the LIGHTSYNC slot-scoped attrs the
/// composed page hides) is a no-op: its value still lands in
/// `known_values` via the response handler, which is all the composed
/// page reads. `led_slot`/`led_names` feed the effect selector's rewrite
/// when the updated row is `wheel_led_effect` on the composed page (where
/// the model row carries the selector, not the raw 1-9 value).
fn update_row(
    app: &App,
    row: &viewmodel::Row,
    error: Option<&str>,
    profile_names: &[String],
    led_slot: i32,
    led_names: &[String],
) {
    let model = app.get_rows();
    let Some(index) = (0..model.row_count()).find(|&i| model.row_data(i).is_some_and(|r| r.attr == row.attr))
    else {
        return;
    };
    let mut sr = bridge::to_setting_row_with_error(row, error);
    if row.attr == "wheel_profile" {
        bridge::apply_profile_choices(&mut sr, profile_names);
    }
    if row.attr == "wheel_led_effect"
        && model.row_data(index).is_some_and(|r| r.kind == bridge::KIND_LIGHT_EFFECT)
    {
        bridge::apply_lightsync_effect(&mut sr, led_slot, led_names);
    }
    // Same per-push revision bump as `load_rows`; this is what makes an
    // error-revert (fresh read equal to the pre-edit value) still reach a
    // widget whose binding the user's own input severed.
    sr.revision = model.row_data(index).map_or(0, |r| r.revision.wrapping_add(1));
    model.set_row_data(index, sr);
    // A fresh effect value also decides whether the sibling Edit slot
    // button is live (it only edits the ACTIVE custom slot).
    if row.attr == "wheel_led_effect" {
        if let Some(bidx) = (0..model.row_count())
            .find(|&i| model.row_data(i).is_some_and(|r| r.attr == bridge::LIGHT_EDIT_SLOT_ATTR))
        {
            if let Some(mut button) = model.row_data(bidx) {
                button.bool_value = matches!(row.value, Some(Value::Int(5)));
                button.revision = button.revision.wrapping_add(1);
                model.set_row_data(bidx, button);
            }
        }
    }
}

/// Rebuild the composed effect selector's labels from the per-slot name
/// cache. Called when a slot rename settles: the rename's `RowUpdated`
/// only carries the (hidden) name row, but the selector renders that name
/// in its CUSTOM entry, same pattern as `refresh_profile_row`. Selection
/// and everything else on the row stay as they are.
fn refresh_light_effect_row(app: &App, led_names: &[String], effect: u8) {
    let model = app.get_rows();
    let Some(index) = (0..model.row_count()).find(|&i| {
        model.row_data(i).is_some_and(|r| r.attr == "wheel_led_effect" && r.kind == bridge::KIND_LIGHT_EFFECT)
    }) else {
        return;
    };
    let Some(mut sr) = model.row_data(index) else { return };
    sr.choices =
        slint::ModelRc::new(slint::VecModel::from(bridge::lightsync_choice_labels(led_names, effect)));
    sr.revision = sr.revision.wrapping_add(1);
    model.set_row_data(index, sr);
}

/// Rebuild the `wheel_profile` row's dropdown labels from `names`
/// (`wheel_profile_names`'s freshest value). Called when a slot rename's
/// `RowUpdated` lands: that response only updates the names row itself, but
/// the sibling profile dropdown renders those same names and would otherwise
/// keep the old ones until the next whole-category reload. Only the labels
/// are replaced: the model row's `int_value` is already the shifted
/// dropdown index (`apply_profile_choices` ran when the row was composed),
/// so re-applying the full rewrite here would shift it twice.
fn refresh_profile_row(app: &App, names: &[String]) {
    let model = app.get_rows();
    let Some(index) =
        (0..model.row_count()).find(|&i| model.row_data(i).is_some_and(|r| r.attr == "wheel_profile"))
    else {
        return;
    };
    let Some(mut sr) = model.row_data(index) else { return };
    sr.choices = slint::ModelRc::new(slint::VecModel::from(bridge::profile_choice_labels(names)));
    sr.revision = sr.revision.wrapping_add(1);
    model.set_row_data(index, sr);
}

/// Push the LIGHTSYNC preview strip's state to the Slint side: the ACTIVE
/// slot's stored colors and its direction, from the same last-known values
/// the slot editor seeds from. A mirrored direction collapses the strip
/// into its played pairs (left half wins) so the preview shows what the
/// wheel plays, same rule as the slot editor. Called on every Leds reload
/// and whenever a colors/direction row update lands.
fn push_led_preview(app: &App, known_values: &Arc<Mutex<HashMap<String, Value>>>) {
    let (mut colors, direction) = {
        let kv = known_values.lock().unwrap();
        let colors = match kv.get("wheel_led_colors") {
            Some(Value::Rgb(cs)) => cs.clone(),
            _ => bridge::default_rgb("wheel_led_colors"),
        };
        let direction = match kv.get("wheel_led_direction") {
            Some(Value::Enum(d)) => *d,
            _ => 0,
        };
        (colors, direction)
    };
    if lightsync::mirrored(direction) {
        lightsync::mirror_left_half(&mut colors);
    }
    app.set_lightsync_leds(bridge::rgb_leds_model(&colors));
    app.set_lightsync_direction(i32::from(direction));
}

/// Read the `Category`/`Mode` out of one of the `Arc<Mutex<_>>` cells below.
/// Both are `Copy`, so the lock never needs to outlive this call.
fn get<T: Copy>(cell: &Arc<Mutex<T>>) -> T {
    *cell.lock().unwrap()
}

fn set<T>(cell: &Arc<Mutex<T>>, value: T) {
    *cell.lock().unwrap() = value;
}

/// Open `url` in the user's default browser via `xdg-open`, detached. Best
/// effort: a spawn failure (no `xdg-open` on a minimal system) is ignored
/// rather than taking the app down, since this is a convenience link.
fn open_in_browser(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// The exact launch-options string the Setup page's "Copy" button copies.
const FFB_LAUNCH_OPTIONS: &str = "logi-ffb %command%";

/// Copy `text` to the clipboard, best-effort: try `wl-copy` (Wayland), then
/// `xclip -selection clipboard` (X11). Ignores every failure (no clipboard
/// tool installed, no display server, ...) since the Setup page's launch-
/// options field is itself selectable as a fallback. Meant to be called off
/// the UI thread (`std::thread::spawn`) since a missing/hanging clipboard
/// tool should never stall the window.
fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};

    if Command::new("wl-copy").arg(text).status().is_ok_and(|s| s.success()) {
        return;
    }
    if let Ok(mut child) = Command::new("xclip").args(["-selection", "clipboard"]).stdin(Stdio::piped()).spawn() {
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

/// The SDK folder field's prefill: `$LOGITECH_TRUEFORCE_SDK_DIR` when set,
/// else `~/.local/share/logitech-trueforce/sdk` (the installer script's
/// own default). Only a starting point for the editable field; the
/// concrete outcome next to it always comes from the full resolution
/// (`sdk_status`), which checks this and every other candidate.
fn sdk_field_prefill() -> String {
    if let Some(dir) = std::env::var_os("LOGITECH_TRUEFORCE_SDK_DIR") {
        if !dir.is_empty() {
            return dir.to_string_lossy().into_owned();
        }
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    home.join(".local/share/logitech-trueforce/sdk").to_string_lossy().into_owned()
}

/// The SDK folder status line: resolve the directory exactly like the
/// installer does (`steam::resolve_sdk_dir`: the field while it validates,
/// then `$LOGITECH_TRUEFORCE_SDK_DIR`, the repo `sdk/` next to the
/// resolved installer, the XDG default) and report the concrete outcome:
/// the resolved path (shown green, and passed explicitly to installs so
/// status and install can never diverge), or a red not-found line saying
/// where the DLLs come from.
fn sdk_status(field: &str, installer: Option<&std::path::Path>) -> (Option<std::path::PathBuf>, String) {
    match logi_dd_core::steam::resolve_sdk_dir(field, installer) {
        Some(dir) => {
            let message = format!("SDK DLLs: found at {}", dir.display());
            (Some(dir), message)
        }
        None => (
            None,
            "SDK DLLs: not found - copy them from a Windows G HUB install; see the README".to_string(),
        ),
    }
}

/// Rescan the installed Proton games off the UI thread (the Steam
/// libraries can live on slow external drives) and push the result into
/// `setup-games` via `slint::invoke_from_event_loop`, the same
/// worker-thread-to-UI pattern `Worker::spawn`'s response closure uses.
/// Runs at startup, on the Rescan button, and after every install/remove
/// so the per-row shim status reflects what just happened.
fn scan_games(app_weak: slint::Weak<App>) {
    std::thread::spawn(move || {
        let games = match std::env::var_os("HOME") {
            Some(home) => {
                let roots = logi_dd_core::steam::library_roots(std::path::Path::new(&home));
                logi_dd_core::steam::installed_games(&roots)
            }
            None => Vec::new(),
        };
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = app_weak.upgrade() else { return };
            let items: Vec<SetupGame> = games
                .iter()
                .map(|g| SetupGame {
                    name: g.name.as_str().into(),
                    prefix: g.prefix.to_string_lossy().as_ref().into(),
                    installed: g.shim_installed,
                })
                .collect();
            app.set_setup_games(slint::ModelRc::new(slint::VecModel::from(items)));
            app.set_setup_games_scanned(true);
        });
    });
}

/// Run the TrueForce SDK shim installer with `args` (a per-game install,
/// `--prefix <pfx>` plus `--sdk-dir <dir>` when the folder validates, or
/// an `--uninstall-prefix <pfx>` remove) off the UI thread, then push its combined stdout+stderr plus
/// exit status back to `setup-shim-output` (and clear
/// `setup-shim-running`) via `slint::invoke_from_event_loop`, followed by
/// a games rescan so the row's shim status updates. `binary` is `None`
/// when `helpers::installer_path` found nothing at startup (neither on
/// `PATH` nor in a checkout above the executable); that is reported
/// immediately, without spawning anything (the installer is never
/// re-resolved mid-run, so a binary installed after startup needs an app
/// restart to be picked up, same as the status line next to the buttons).
fn run_shim_command(app_weak: slint::Weak<App>, binary: Option<String>, args: Vec<String>) {
    let Some(bin) = binary else {
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = app_weak.upgrade() else { return };
            app.set_setup_shim_output("Installer not found (PATH or the repo's tools/ directory).".into());
            app.set_setup_shim_running(false);
        });
        return;
    };
    std::thread::spawn(move || {
        let text = match std::process::Command::new(&bin).args(&args).output() {
            Ok(out) => {
                let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
                combined.push_str(&String::from_utf8_lossy(&out.stderr));
                combined.push_str(&format!("\n[exit status: {}]", out.status));
                combined
            }
            Err(e) => format!("Failed to run {bin}: {e}"),
        };
        let rescan_weak = app_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = app_weak.upgrade() else { return };
            app.set_setup_shim_output(text.into());
            app.set_setup_shim_running(false);
        });
        scan_games(rescan_weak);
    });
}

/// Send SIGTERM to `pid`: the clean stop for both the logi-tf-sim daemon
/// and a running `--sweep` (its handler sends the wheel stop packets on
/// the way out, which a hard kill would skip).
fn send_sigterm(pid: i32) {
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
}

/// Re-probe whether the logi-tf-sim daemon is running (a /proc comm scan)
/// off the UI thread, pushing the outcome into `setup-tf-running`.
/// `expect_running` is the state a just-issued Start/Stop is settling
/// towards: the probe polls (up to ~1.5 s) until the scan agrees, so a
/// daemon that takes a moment to appear or to handle its SIGTERM does not
/// leave a stale status behind; `None` probes once, immediately.
fn refresh_tf_daemon(app_weak: slint::Weak<App>, expect_running: Option<bool>) {
    std::thread::spawn(move || {
        let mut running = logi_dd_core::tfsim::daemon_pid().is_some();
        if let Some(expected) = expect_running {
            for _ in 0..10 {
                if running == expected {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
                running = logi_dd_core::tfsim::daemon_pid().is_some();
            }
        }
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = app_weak.upgrade() {
                app.set_setup_tf_running(running);
            }
        });
    });
}

/// Reload tf-sim.conf and rebuild the compatibility table's rows: a
/// per-game edit must show up on BOTH titles sharing an id (AMS2 and
/// Project CARS 2 are one daemon id), so the whole model is rebuilt from
/// the file rather than patching the edited row.
fn refresh_compat_rows(app: &App) {
    let cfg = logi_dd_core::tfsim::Config::load();
    app.set_setup_compat_rows(slint::ModelRc::new(slint::VecModel::from(bridge::compat_rows(&cfg))));
}

/// Surface a tf-sim.conf write failure in the Simulated TrueForce panel's
/// output area (successes stay quiet; the controls already show the value).
fn report_tf_write(app: &App, result: Result<(), logi_dd_core::Error>) {
    if let Err(e) = result {
        app.set_setup_tf_output(format!("tf-sim.conf: {e}").into());
    }
}

/// Run `logi-tf-sim --sweep` (about 6 s of real haptics; only ever fired
/// from the consent dialog) off the UI thread. The child's pid is parked
/// in `pid_cell` so the Stop button can SIGTERM it early; either way the
/// wait completes here, which pushes the combined output + exit status
/// into `setup-tf-output` and resets the run state exactly once, same
/// single-cleanup-site discipline as the Test page's simulations.
fn run_tf_sweep(
    app_weak: slint::Weak<App>,
    binary: String,
    pid_cell: Arc<Mutex<Option<i32>>>,
) {
    std::thread::spawn(move || {
        let spawned = std::process::Command::new(&binary)
            .arg("--sweep")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();
        let text = match spawned {
            Ok(child) => {
                *pid_cell.lock().unwrap() = Some(child.id() as i32);
                let waited = child.wait_with_output();
                *pid_cell.lock().unwrap() = None;
                match waited {
                    Ok(out) => {
                        let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
                        combined.push_str(&String::from_utf8_lossy(&out.stderr));
                        combined.push_str(&format!("\n[exit status: {}]", out.status));
                        combined
                    }
                    Err(e) => format!("Failed to wait for {binary}: {e}"),
                }
            }
            Err(e) => format!("Failed to run {binary}: {e}"),
        };
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = app_weak.upgrade() else { return };
            app.set_setup_tf_output(text.into());
            app.set_setup_tf_sweep_running(false);
        });
    });
}

/// Dev-only render aid: `LOGI_DD_TEST_OVERLAYS=<degrees>` forces every
/// callout overlay visible and the wheel image to that steering angle, so
/// the overlay positions and the rotation can be screenshot-verified
/// against the layout PNG without pressing 19 physical buttons. Inert
/// unless the variable is set; read once.
fn overlay_debug() -> Option<f32> {
    static FLAG: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *FLAG.get_or_init(|| {
        std::env::var("LOGI_DD_TEST_OVERLAYS").ok().map(|v| v.trim().parse().unwrap_or(0.0))
    })
}

/// Push one reader-thread [`testio::Snapshot`] into the Test page's
/// properties. Runs on the UI thread (the reader's callback hops here via
/// `slint::invoke_from_event_loop` first). Degrees are derived from the
/// raw 0..65535 steering axis and the `test-range` property the monitor
/// start seeded from `wheel_range`.
fn apply_test_snapshot(app: &App, snap: &testio::Snapshot) {
    let range = app.get_test_range().max(1) as u32;
    let debug = overlay_debug();
    let deg = debug
        .unwrap_or_else(|| evtest::steering_degrees(snap.steering_raw, 0, evtest::AXIS_MAX, range));
    app.set_test_degrees(deg);
    app.set_test_degrees_text(format!("{deg:+.1} deg").into());
    app.set_test_hat(evtest::hat_label(snap.hat.0, snap.hat.1).into());
    let buttons: Vec<TestButton> = evtest::WHEEL_BUTTONS
        .iter()
        .zip(&snap.buttons)
        .map(|((code, label), pressed)| TestButton {
            code: i32::from(*code),
            label: (*label).into(),
            pressed: *pressed,
        })
        .collect();
    app.set_test_buttons(slint::ModelRc::new(slint::VecModel::from(buttons)));
    // The layout image's callout tints: a box lights while any of its
    // buttons is held (`snap.buttons` is parallel to `WHEEL_BUTTONS`), the
    // D-pad box while the hat is off center.
    let held = |code: u16| {
        evtest::WHEEL_BUTTONS
            .iter()
            .position(|(c, _)| *c == code)
            .is_some_and(|i| snap.buttons.get(i).copied().unwrap_or(false))
    };
    let callouts: Vec<TestCallout> = evtest::CALLOUT_BOXES
        .iter()
        .map(|b| TestCallout {
            cx: b.cx,
            cy: b.cy,
            w: b.w,
            h: b.h,
            pressed: debug.is_some() || evtest::callout_lit(b, snap.hat, held),
        })
        .collect();
    app.set_test_callouts(slint::ModelRc::new(slint::VecModel::from(callouts)));
    let axes: Vec<TestAxis> = [("Throttle", 0), ("Brake", 1), ("Clutch", 2), ("Handbrake", 3)]
        .iter()
        .map(|(label, i)| TestAxis { label: (*label).into(), value: snap.axes[*i] })
        .collect();
    app.set_test_axes(slint::ModelRc::new(slint::VecModel::from(axes)));
}

/// Stop (and join) the Test page's reader thread, if one is running.
/// Cheap when none is: just a mutex lock. Called when navigating off the
/// Test page, before every re-discovery, and at app exit.
fn stop_test_monitor(reader_cell: &Arc<Mutex<Option<testio::Reader>>>) {
    if let Some(reader) = reader_cell.lock().unwrap().take() {
        reader.stop();
    }
}

/// (Re-)discover the wheel's evdev node and start the reader thread for
/// the Test page. Discovery and the one-off `wheel_range` read run off
/// the UI thread (same pattern as `scan_games`); the result lands back
/// via `slint::invoke_from_event_loop`, which also stores the running
/// reader in `reader_cell` and the found device in `device_cell` (what
/// the sim buttons play against). Called at page-open and on Rescan.
fn start_test_monitor(
    app_weak: slint::Weak<App>,
    reader_cell: Arc<Mutex<Option<testio::Reader>>>,
    device_cell: Arc<Mutex<Option<evtest::WheelInput>>>,
) {
    stop_test_monitor(&reader_cell);
    std::thread::spawn(move || {
        let found = evtest::discover_wheel_input();
        // The configured rotation range, read once through the same sysfs
        // plumbing the settings pages use; 900 when unreadable.
        let range = logi_dd_core::Device::discover()
            .ok()
            .and_then(|d| d.read("wheel_range").ok())
            .and_then(|v| match v {
                Value::Int(n) => u32::try_from(n).ok(),
                _ => None,
            })
            .unwrap_or(900);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = app_weak.upgrade() else { return };
            app.set_test_scanned(true);
            app.set_test_range(range as i32);
            app.set_test_sim_error("".into());
            let Some(wheel) = found else {
                *device_cell.lock().unwrap() = None;
                app.set_test_available(false);
                app.set_test_device_name("".into());
                return;
            };
            app.set_test_device_name(wheel.name.as_str().into());
            let snap_weak = app.as_weak();
            let gone_weak = app.as_weak();
            match testio::Reader::start(
                &wheel.event_path,
                move |snap| {
                    // Reader thread -> UI thread, at most ~30 Hz (the
                    // reader throttles before calling this).
                    let weak = snap_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = weak.upgrade() {
                            apply_test_snapshot(&app, &snap);
                        }
                    });
                },
                move || {
                    // The wheel disappeared mid-session: back to the
                    // empty state (the dead reader handle is reaped by
                    // the next start/stop).
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = gone_weak.upgrade() {
                            app.set_test_available(false);
                            app.set_test_device_name("".into());
                        }
                    });
                },
            ) {
                Ok(reader) => {
                    *reader_cell.lock().unwrap() = Some(reader);
                    *device_cell.lock().unwrap() = Some(wheel);
                    app.set_test_available(true);
                }
                Err(e) => {
                    // Found but not openable (most likely permissions):
                    // stay in the empty state with the reason shown.
                    *device_cell.lock().unwrap() = None;
                    app.set_test_available(false);
                    app.set_test_sim_error(
                        format!("Cannot open {}: {e} (read access to /dev/input is required)", wheel.event_path)
                            .into(),
                    );
                }
            }
        });
    });
}

/// The playing simulation's cancel flag, `None` while no sim runs. The
/// Stop button sets the flag through this cell; the runner thread (which
/// holds its own clone) wakes within its poll tick, stops + erases the
/// effect through its single cleanup site, then clears the cell.
type SimCancelCell = Arc<Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>>;

/// Run one confirmed force simulation against the discovered wheel, off
/// the UI thread, pushing completion (and any error) back into the Test
/// page's properties. A missing device is a silent no-op: the buttons
/// are disabled without a wheel, so this only races an unplug.
fn run_test_sim(
    app_weak: slint::Weak<App>,
    device_cell: &Arc<Mutex<Option<evtest::WheelInput>>>,
    cancel_cell: &SimCancelCell,
    kind: testio::SimKind,
) {
    let Some(app) = app_weak.upgrade() else { return };
    let Some(wheel) = device_cell.lock().unwrap().clone() else { return };
    if app.get_test_sim_running() {
        return;
    }
    app.set_test_sim_running(true);
    app.set_test_sim_error("".into());
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    *cancel_cell.lock().unwrap() = Some(cancel.clone());
    let cancel_cell = cancel_cell.clone();
    let weak = app.as_weak();
    std::thread::spawn(move || {
        // Completion and cancellation both come back through here (the
        // cancel flag only shortens the wait inside run_simulation, which
        // cleans up on every path), so the UI state resets exactly once.
        let result = testio::run_simulation(&wheel.event_path, kind, &cancel);
        *cancel_cell.lock().unwrap() = None;
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = weak.upgrade() else { return };
            app.set_test_sim_running(false);
            if let Err(e) = result {
                app.set_test_sim_error(e.into());
            }
        });
    });
}

fn main() -> Result<(), slint::PlatformError> {
    let app = App::new()?;
    // The sidebar labels are the real device categories plus a trailing
    // "Setup" row; kept out of `bridge::category_labels_model` (and its own
    // test) since that function's contract is "one label per
    // `Category::ALL`, in `Category::ALL`'s order" and Setup is not a
    // device category.
    let mut labels: Vec<slint::SharedString> = bridge::category_labels_model().iter().collect();
    labels.push("Setup".into());
    app.set_category_labels(slint::ModelRc::new(slint::VecModel::from(labels)));
    let setup_index = Category::ALL.len() as i32;
    app.set_setup_index(setup_index);
    // The Info category carries the live input monitor (the old Test page);
    // selecting it starts the evdev reader, leaving it stops it.
    let info_index = bridge::index_of(Category::Info);
    app.set_info_index(info_index);
    // The Profiles category swaps in the computer-side profile store while
    // the wheel is in desktop mode (see `show-computer-profiles`).
    app.set_profiles_index(bridge::index_of(Category::Profiles));
    // The Leds category carries the LIGHTSYNC preview strip above its
    // composed settings list.
    app.set_leds_index(bridge::index_of(Category::Leds));
    app.set_project_url(logi_dd_core::PROJECT_URL.into());
    app.on_open_url(|url| open_in_browser(&url));

    // Setup page: helper presence, resolved once at startup: `PATH` first,
    // then the repo-checkout fallbacks (`logi-ffb` next to this executable,
    // the installer in a `tools/` directory above it); see
    // `logi_dd_core::helpers`. The status lines show the resolved path.
    let ffb_binary = logi_dd_core::helpers::ffb_path();
    app.set_setup_ffb_found(ffb_binary.is_some());
    app.set_setup_ffb_path(
        ffb_binary.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default().into(),
    );
    // The installer's resolved path is kept both ways: the string feeds the
    // status line and the shim runs, the `PathBuf` anchors the SDK
    // resolution's repo-`sdk/` candidate (see `sdk_status`).
    let installer_path: Option<std::path::PathBuf> = logi_dd_core::helpers::installer_path();
    let shim_binary = installer_path.as_ref().map(|p| p.to_string_lossy().into_owned());
    app.set_setup_shim_found(shim_binary.is_some());
    app.set_setup_shim_path(shim_binary.clone().unwrap_or_default().into());
    // Setup page: the SDK folder field's prefill + resolved status, and the
    // installed-games scan (off the UI thread). The field text lives in a
    // shared cell so every per-game install run re-resolves from the
    // freshest value.
    let sdk_dir = Arc::new(Mutex::new(sdk_field_prefill()));
    {
        let dir = sdk_dir.lock().unwrap().clone();
        let (resolved, message) = sdk_status(&dir, installer_path.as_deref());
        app.set_setup_sdk_dir(dir.into());
        app.set_setup_sdk_valid(resolved.is_some());
        app.set_setup_sdk_status(message.into());
    }
    scan_games(app.as_weak());
    // Setup page: Simulated TrueForce. The binary resolves once at startup
    // (same rule as logi-ffb); tf-sim.conf is read fresh for the controls
    // and the compatibility table, and the daemon's run state is probed
    // off the UI thread.
    let tf_binary: Option<String> =
        logi_dd_core::helpers::tf_sim_path().map(|p| p.to_string_lossy().into_owned());
    app.set_setup_tf_found(tf_binary.is_some());
    app.set_setup_tf_path(tf_binary.clone().unwrap_or_default().into());
    {
        let cfg = logi_dd_core::tfsim::Config::load();
        app.set_setup_tf_enabled(cfg.enabled);
        app.set_setup_tf_intensity(i32::from(cfg.intensity));
        app.set_setup_tf_pitch(i32::from(cfg.pitch_pct));
        app.set_setup_compat_rows(slint::ModelRc::new(slint::VecModel::from(
            bridge::compat_rows(&cfg),
        )));
    }
    refresh_tf_daemon(app.as_weak(), None);
    // Installed once, here, and never replaced: `load_rows`/`update_row`
    // mutate this same `VecModel`'s contents for the rest of the app's
    // life (see `load_rows`'s doc comment for why that matters).
    app.set_rows(slint::ModelRc::new(slint::VecModel::<SettingRow>::default()));

    // UI-side state the worker's responses need to be checked against, or
    // the mode toggle needs to compute its target: which category is on
    // screen, and which mode the header last reported. Both start at a
    // reasonable default and get corrected by the worker's first replies.
    // `Arc<Mutex<_>>`, not `Rc<Cell<_>>`, because the worker's response
    // callback runs on the worker thread and must be `Send`.
    let current_category = Arc::new(Mutex::new(Category::ALL[0]));
    let current_mode = Arc::new(Mutex::new(Mode::Desktop));
    app.set_category_label(get(&current_category).label().into());
    app.set_selected_category(bridge::index_of(get(&current_category)));

    // The current category's row values, keyed by attr, refreshed on every
    // `Response::Rows`. The curve editor needs this to seed a `Curve` from
    // the attribute's live value when a row is activated: the worker only
    // hands back `Vec<Row>` inside that one response, so nothing else in
    // this file holds onto it once `bridge::rows_model` has built the
    // Slint-facing rows.
    let known_values: Arc<Mutex<HashMap<String, Value>>> = Arc::new(Mutex::new(HashMap::new()));
    // Per-slot LIGHTSYNC name cache for the effect selector's CUSTOM
    // labels. `wheel_led_slot_name` only ever reads the ACTIVE slot's
    // name, so the full set cannot be read in one go without slot-churning
    // writes; instead every name that flows past (paired with the slot it
    // belonged to at read time) is remembered here, and unseen slots show
    // the plain "CUSTOM N" fallback.
    let led_slot_names: Arc<Mutex<Vec<String>>> =
        Arc::new(Mutex::new(vec![String::new(); lightsync::CUSTOM_SLOTS]));
    // The per-axis shaping view toggles: pure per-session view state (never
    // persisted, never a sysfs write). Read when composing rows; flipped by
    // the synthetic `shaping::toggle_attr` rows' Switches, which
    // `edit-switch` intercepts below.
    let shaping_toggles = Arc::new(Mutex::new(shaping::AxisToggles::default()));
    // The current category's last full (unfiltered) row list, kept so the
    // shaping toggle can re-compose the page locally, without a worker
    // round trip: the visible model only holds the filtered rows, so
    // flipping the filter needs the originals back.
    let last_rows: Arc<Mutex<Vec<viewmodel::Row>>> = Arc::new(Mutex::new(Vec::new()));
    // The curve editor's in-flight state, `None` while the overlay is
    // closed. UI-thread only (see `CurveEditorState`'s own doc comment).
    let curve_editor: Arc<Mutex<Option<CurveEditorState>>> = Arc::new(Mutex::new(None));
    // The RGB strip editor's in-flight state, same lifetime/thread rules as
    // `curve_editor` (see `RgbEditorState`'s own doc comment).
    let rgb_editor: Arc<Mutex<Option<RgbEditorState>>> = Arc::new(Mutex::new(None));
    // The slot-text editor's in-flight state, same lifetime/thread rules as
    // `curve_editor` and `rgb_editor` (see `SlotTextEditorState`'s own doc
    // comment).
    let slot_text_editor: Arc<Mutex<Option<SlotTextEditorState>>> = Arc::new(Mutex::new(None));

    let worker = {
        let app_weak = app.as_weak();
        let current_category = current_category.clone();
        let current_mode = current_mode.clone();
        let known_values = known_values.clone();
        let led_slot_names = led_slot_names.clone();
        let slot_text_editor = slot_text_editor.clone();
        let shaping_toggles = shaping_toggles.clone();
        let last_rows = last_rows.clone();
        Worker::spawn(move |response| {
            let app_weak = app_weak.clone();
            let current_category = current_category.clone();
            let current_mode = current_mode.clone();
            let known_values = known_values.clone();
            let led_slot_names = led_slot_names.clone();
            let slot_text_editor = slot_text_editor.clone();
            let shaping_toggles = shaping_toggles.clone();
            let last_rows = last_rows.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = app_weak.upgrade() else { return };
                match response {
                    Response::Rows { category, rows } => {
                        // A category switch (or a slow edit reply racing a
                        // later switch) can make this response stale;
                        // only the category currently on screen matters.
                        if category != get(&current_category) {
                            return;
                        }
                        {
                            let mut kv = known_values.lock().unwrap();
                            for row in &rows {
                                if let Some(v) = &row.value {
                                    kv.insert(row.attr.to_string(), v.clone());
                                }
                            }
                        }
                        // The Info page renders the identity rows as its
                        // own header (the rest of the page is the live
                        // input monitor), so push them into their string
                        // properties instead of a settings list.
                        if category == Category::Info {
                            let text = |attr: &str| {
                                rows.iter()
                                    .find(|r| r.attr == attr)
                                    .and_then(|r| match &r.value {
                                        Some(Value::Text(s)) => Some(s.clone()),
                                        _ => None,
                                    })
                                    .unwrap_or_default()
                            };
                            app.set_info_serial(text("wheel_serial").into());
                            app.set_info_firmware(text("wheel_firmware").into());
                        }
                        // A Leds reload reads the active slot and its name
                        // together, which is the only safe pairing for the
                        // per-slot name cache (see `led_slot_names`).
                        let slot = rows.iter().find(|r| r.attr == "wheel_led_slot").and_then(|r| {
                            match &r.value {
                                Some(Value::Int(n)) => Some(*n),
                                _ => None,
                            }
                        });
                        let name = rows.iter().find(|r| r.attr == "wheel_led_slot_name").and_then(|r| {
                            match &r.value {
                                Some(Value::Text(s)) => Some(s.clone()),
                                _ => None,
                            }
                        });
                        if let (Some(slot), Some(name)) = (slot, name) {
                            cache_led_slot_name(&led_slot_names, slot, &name);
                        }
                        // The preview strip renders the slot-scoped values
                        // the composed page hides; refresh it from the
                        // just-updated cache alongside the rows.
                        if category == Category::Leds {
                            push_led_preview(&app, &known_values);
                        }
                        let led_names = led_slot_names.lock().unwrap().clone();
                        // Remember the full list before it is filtered so
                        // the shaping toggle can re-compose locally (see
                        // `last_rows`'s doc comment above).
                        *last_rows.lock().unwrap() = rows;
                        let rows = last_rows.lock().unwrap();
                        load_rows(
                            &app,
                            &rows,
                            &profile_names(&known_values),
                            &led_names,
                            get(&shaping_toggles),
                        );
                    }
                    Response::RowUpdated { category, row, error } => {
                        // Same staleness guard as `Rows`: a reply for a
                        // category the user has since navigated away from
                        // should not touch what is on screen now.
                        if category != get(&current_category) {
                            return;
                        }
                        if let Some(v) = &row.value {
                            known_values.lock().unwrap().insert(row.attr.to_string(), v.clone());
                        }
                        // A LIGHTSYNC rename's reply carries the device's
                        // authoritative (uppercased or reverted) name for
                        // the active slot: remember it for the effect
                        // selector's CUSTOM label and push it back into
                        // the slot editor overlay's name field. The slot
                        // itself settled before any rename could be typed,
                        // so pairing with the last-known slot is safe.
                        if row.attr == "wheel_led_slot_name" {
                            if let Some(Value::Text(name)) = &row.value {
                                cache_led_slot_name(&led_slot_names, led_slot(&known_values), name);
                                app.set_rgb_slot_name(name.as_str().into());
                            }
                            let led_names = led_slot_names.lock().unwrap().clone();
                            refresh_light_effect_row(&app, &led_names, led_effect(&known_values));
                        }
                        // Same push-back for the slot brightness slider in
                        // the overlay (a rejected write must visibly
                        // revert, same rule as the settings sliders).
                        if row.attr == "wheel_led_slot_brightness" {
                            if let Some(Value::Percent(p)) = &row.value {
                                app.set_rgb_slot_brightness(i32::from(*p));
                            }
                        }
                        // A colors or direction edit (the slot editor's
                        // commit) also redraws the preview strip; the
                        // composed page has no row for either attr, so
                        // `update_row` below cannot do it.
                        if matches!(row.attr, "wheel_led_colors" | "wheel_led_direction") {
                            push_led_preview(&app, &known_values);
                        }
                        let led_names = led_slot_names.lock().unwrap().clone();
                        update_row(
                            &app,
                            &row,
                            error.as_deref(),
                            &profile_names(&known_values),
                            led_slot(&known_values),
                            &led_names,
                        );
                        if row.attr == "wheel_profile_names" {
                            // A slot rename also changes the labels the
                            // sibling profile dropdown shows.
                            refresh_profile_row(&app, &profile_names(&known_values));
                        }
                        // If the slot-text overlay is open on this attr,
                        // push the device's authoritative names back into
                        // it (a rejected rename must visibly revert; a
                        // successful one shows the wheel's uppercased form)
                        // and surface the write error inside the overlay
                        // instead of only on the settings row hidden behind
                        // it.
                        let mut guard = slot_text_editor.lock().unwrap();
                        if let Some(state) = guard.as_mut() {
                            if state.attr == row.attr {
                                if let Some(Value::SlotNames(names)) = &row.value {
                                    state.names = names.clone();
                                }
                                push_slot_text_editor(&app, &state.names);
                                app.set_slot_text_error(error.as_deref().unwrap_or("").into());
                            }
                        }
                        drop(guard);
                        // Keep the unfiltered cache fresh too, so a shaping
                        // toggle right after an edit re-composes from this
                        // row's new value, not the last whole-category read.
                        if let Some(entry) =
                            last_rows.lock().unwrap().iter_mut().find(|r| r.attr == row.attr)
                        {
                            *entry = row;
                        }
                    }
                    Response::Info(info) => {
                        set(&current_mode, info.mode);
                        app.set_no_wheel(false);
                        app.set_no_wheel_message("".into());
                        app.set_mode_onboard(matches!(info.mode, Mode::Onboard));
                    }
                    Response::NoWheel(message) => {
                        app.set_no_wheel(true);
                        app.set_no_wheel_message(message.into());
                        // No sysfs identity to show; the Info page falls
                        // back to its "-" placeholders.
                        app.set_info_serial("".into());
                        app.set_info_firmware("".into());
                        // An in-flight LIGHTSYNC try answered by NoWheel
                        // (the wheel vanished) must re-enable its button.
                        app.set_lightsync_try_running(false);
                    }
                    Response::Profiles { names, status, error } => {
                        let items: Vec<slint::SharedString> =
                            names.iter().map(|n| slint::SharedString::from(n.as_str())).collect();
                        app.set_computer_profiles(slint::ModelRc::new(slint::VecModel::from(items)));
                        app.set_profiles_status(status.into());
                        app.set_profiles_status_error(error);
                    }
                    Response::LedTryDone { error } => {
                        // The try finished (held and restored); re-enable
                        // the button and surface any failure next to it.
                        app.set_lightsync_try_running(false);
                        app.set_lightsync_try_error(error.unwrap_or_default().into());
                    }
                }
            });
        })
    };

    // The Test page's reader thread (`None` while the page is closed or no
    // wheel was found), the evdev node the sim buttons play against, and
    // the playing sim's cancel flag (see `SimCancelCell`).
    let test_reader: Arc<Mutex<Option<testio::Reader>>> = Arc::new(Mutex::new(None));
    let test_device: Arc<Mutex<Option<evtest::WheelInput>>> = Arc::new(Mutex::new(None));
    let test_sim_cancel: SimCancelCell = Arc::new(Mutex::new(None));

    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        let app_weak = app.as_weak();
        let test_reader = test_reader.clone();
        let test_device = test_device.clone();
        app.on_select_category(move |index| {
            // The trailing "Setup" row: show that page and stop, without
            // asking the worker for a category (there is none). Switching
            // back to a real category below still reloads it via the usual
            // `LoadCategory` request, so nothing needs to force a refresh
            // when leaving the page.
            if index == setup_index {
                stop_test_monitor(&test_reader);
                if let Some(app) = app_weak.upgrade() {
                    app.set_selected_category(index);
                }
                return;
            }
            let cat = bridge::category_at(index);
            // The Info category hosts the live input monitor: entering it
            // starts the evdev reader (independent of the sysfs worker
            // request below, which fetches the identity rows); leaving it
            // stops the reader so no fd stays open behind other pages.
            if cat == Category::Info {
                start_test_monitor(app_weak.clone(), test_reader.clone(), test_device.clone());
            } else {
                stop_test_monitor(&test_reader);
            }
            set(&current_category, cat);
            if let Some(app) = app_weak.upgrade() {
                app.set_selected_category(bridge::index_of(cat));
                app.set_category_label(cat.label().into());
            }
            worker.request(Request::LoadCategory(cat));
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_retry_discover(move || {
            worker.request(Request::Discover);
            worker.request(Request::LoadCategory(get(&current_category)));
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_refresh(move || {
            worker.request(Request::Refresh(get(&current_category)));
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        let current_mode = current_mode.clone();
        app.on_toggle_mode(move || {
            let target = match get(&current_mode) {
                Mode::Desktop => Mode::Onboard,
                Mode::Onboard => Mode::Desktop,
            };
            worker.request(Request::SetMode(target));
            worker.request(Request::Refresh(get(&current_category)));
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_edit_slider(move |attr, value| {
            worker.request(Request::Edit {
                category: get(&current_category),
                attr: attr.to_string(),
                input: WidgetInput::Slider(i64::from(value)),
            });
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_edit_choice(move |attr, index| {
            worker.request(Request::Edit {
                category: get(&current_category),
                attr: attr.to_string(),
                input: WidgetInput::Choice(index.max(0) as usize),
            });
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        let shaping_toggles = shaping_toggles.clone();
        let last_rows = last_rows.clone();
        let known_values = known_values.clone();
        let led_slot_names = led_slot_names.clone();
        let app_weak = app.as_weak();
        app.on_edit_switch(move |attr, value| {
            // A per-axis shaping row is view state, not a device attribute:
            // flip that axis's flag and re-compose the current rows from
            // the unfiltered cache, without a worker round trip.
            if let Some(axis) = shaping::toggle_axis(&attr) {
                let toggles = {
                    let mut guard = shaping_toggles.lock().unwrap();
                    guard.set(axis, value);
                    *guard
                };
                let Some(app) = app_weak.upgrade() else { return };
                let led_names = led_slot_names.lock().unwrap().clone();
                let rows = last_rows.lock().unwrap();
                load_rows(&app, &rows, &profile_names(&known_values), &led_names, toggles);
                return;
            }
            worker.request(Request::Edit {
                category: get(&current_category),
                attr: attr.to_string(),
                input: WidgetInput::Switch(value),
            });
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_edit_text(move |attr, text| {
            worker.request(Request::Edit {
                category: get(&current_category),
                attr: attr.to_string(),
                input: WidgetInput::Text(text.to_string()),
            });
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_trigger(move |attr| {
            worker.request(Request::Edit {
                category: get(&current_category),
                attr: attr.to_string(),
                input: WidgetInput::Trigger,
            });
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_edit_pair(move |attr, is_lower, value| {
            // Only the touched half travels; the worker reads the other
            // half fresh from the device at write time (see
            // `ViewModel::edit`), so a quick lower-then-upper edit cannot
            // clobber the first edit with a stale row snapshot.
            let v = value.clamp(0, 255) as u8;
            worker.request(Request::Edit {
                category: get(&current_category),
                attr: attr.to_string(),
                input: if is_lower { WidgetInput::PairLower(v) } else { WidgetInput::PairUpper(v) },
            });
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        let known_values = known_values.clone();
        app.on_edit_light_effect(move |index| {
            let category = get(&current_category);
            // The current raw effect only matters for the trailing raw
            // entry (present while the device reports an effect outside
            // 1-5): picking it re-writes that same value.
            match lightsync::index_selection(index.max(0) as usize, led_effect(&known_values)) {
                lightsync::Selection::Effect(e) => {
                    worker.request(Request::Edit {
                        category,
                        attr: "wheel_led_effect".to_string(),
                        input: WidgetInput::Slider(i64::from(e)),
                    });
                }
                // A CUSTOM entry is two writes: point the wheel at the
                // slot, then switch to the custom effect (the driver
                // re-applies the slot's stored config on that
                // transition). The worker runs queued edits in order,
                // and each reply's RowUpdated re-syncs the selector via
                // the revision mechanism.
                lightsync::Selection::Custom(slot) => {
                    worker.request(Request::Edit {
                        category,
                        attr: "wheel_led_slot".to_string(),
                        input: WidgetInput::Slider(i64::from(slot)),
                    });
                    worker.request(Request::Edit {
                        category,
                        attr: "wheel_led_effect".to_string(),
                        input: WidgetInput::Slider(5),
                    });
                    // The slot switch re-targets every slot-scoped attr
                    // (name, colors, direction, brightness) at the new
                    // slot; re-read the category so `known_values` (which
                    // seeds the slot editor) and the name cache pick up
                    // the new slot's state instead of the old slot's.
                    worker.request(Request::Refresh(category));
                }
            }
        });
    }

    {
        // The LIGHTSYNC preview's "Try on wheel": apply the currently
        // selected effect/slot (i.e. the device's last-known state, which
        // the selector commits immediately; re-applying it makes the wheel
        // visibly play it) for ~5 s, then restore. Runs on the worker
        // thread; only LED state is written.
        let worker = worker.clone();
        let known_values = known_values.clone();
        let app_weak = app.as_weak();
        app.on_lightsync_try(move || {
            let Some(app) = app_weak.upgrade() else { return };
            if app.get_lightsync_try_running() {
                return;
            }
            app.set_lightsync_try_running(true);
            app.set_lightsync_try_error("".into());
            let effect = led_effect(&known_values).clamp(1, 9);
            let slot = led_slot(&known_values).clamp(0, lightsync::CUSTOM_SLOTS as i32 - 1) as u8;
            worker.request(Request::LedTry { effect, slot });
        });
    }

    {
        let known_values = known_values.clone();
        let curve_editor = curve_editor.clone();
        let current_category = current_category.clone();
        let app_weak = app.as_weak();
        app.on_edit_curve(move |attr| {
            let Some(app) = app_weak.upgrade() else { return };
            let attr = attr.to_string();
            let value = known_values.lock().unwrap().get(&attr).cloned().unwrap_or(Value::Curve(vec![]));
            let curve = Curve::from_value(&attr, &value);
            push_curve_editor(&app, &curve);
            let label = REGISTRY.iter().find(|s| s.attr == attr).map(|s| s.label).unwrap_or(attr.as_str());
            app.set_curve_label(label.into());
            // The plot's x-axis caption, named for the axis this attribute
            // shapes (the y-axis is always "Output %").
            let x_label = match attr.as_str() {
                "wheel_response_curve" => "Steering input %",
                "wheel_throttle_curve" => "Throttle input %",
                "wheel_brake_curve" => "Brake input %",
                "wheel_clutch_curve" => "Clutch input %",
                "wheel_handbrake_curve" => "Handbrake input %",
                _ => "Input %",
            };
            app.set_curve_x_label(x_label.into());
            app.set_curve_editor_open(true);
            *curve_editor.lock().unwrap() = Some(CurveEditorState { attr, category: get(&current_category), curve });
        });
    }
    {
        let curve_editor = curve_editor.clone();
        let app_weak = app.as_weak();
        app.on_curve_move_point(move |index, x, y| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = curve_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            bridge::apply_move_point(&mut state.curve, index.max(0) as usize, x, y);
            push_curve_editor(&app, &state.curve);
        });
    }
    {
        let curve_editor = curve_editor.clone();
        app.on_curve_grab_point(move |x, y, aspect| {
            let guard = curve_editor.lock().unwrap();
            let Some(state) = guard.as_ref() else { return -1 };
            let fracs = bridge::control_point_fracs(&state.curve);
            let last = fracs.len().saturating_sub(1);
            // Distances are measured in height-fraction units, with the x
            // component scaled by the plot's aspect ratio, so the grab
            // radius covers the same number of pixels in both directions
            // (in plain fraction space a wide plot made the horizontal
            // reach several times the vertical one). 0.065 of the plot
            // height is ~14px on the default 220px-tall plot, matching the
            // drawn handle size.
            let aspect = if aspect > 0.0 { aspect } else { 1.0 };
            let mut best = -1_i32;
            let mut best_d = 0.065_f32;
            for (i, (px, py)) in fracs.into_iter().enumerate() {
                // The endpoints are pinned (Curve::move_point never moves
                // them), so grabbing one could only swallow the click;
                // skipping them lets clicks near the plot's ends fall
                // through to add-point instead.
                if i == 0 || i == last {
                    continue;
                }
                let d = (((px - x) * aspect).powi(2) + (py - y).powi(2)).sqrt();
                if d < best_d {
                    best_d = d;
                    best = i as i32;
                }
            }
            best
        });
    }
    {
        let curve_editor = curve_editor.clone();
        let app_weak = app.as_weak();
        app.on_curve_add_point(move |x| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = curve_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            bridge::apply_add_point(&mut state.curve, x);
            push_curve_editor(&app, &state.curve);
        });
    }
    {
        let curve_editor = curve_editor.clone();
        let app_weak = app.as_weak();
        app.on_curve_remove_point(move |index| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = curve_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            state.curve.remove_point(index.max(0) as usize);
            push_curve_editor(&app, &state.curve);
        });
    }
    {
        let curve_editor = curve_editor.clone();
        let app_weak = app.as_weak();
        app.on_curve_set_lower_deadzone(move |v| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = curve_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            bridge::apply_lower_deadzone(&mut state.curve, v);
            push_curve_editor(&app, &state.curve);
        });
    }
    {
        let curve_editor = curve_editor.clone();
        let app_weak = app.as_weak();
        app.on_curve_set_upper_deadzone(move |v| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = curve_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            bridge::apply_upper_deadzone(&mut state.curve, v);
            push_curve_editor(&app, &state.curve);
        });
    }
    {
        let worker = worker.clone();
        let curve_editor = curve_editor.clone();
        let app_weak = app.as_weak();
        app.on_curve_commit(move || {
            if let Some(state) = curve_editor.lock().unwrap().take() {
                worker.request(Request::Edit {
                    category: state.category,
                    attr: state.attr,
                    input: WidgetInput::Curve(state.curve),
                });
            }
            if let Some(app) = app_weak.upgrade() {
                app.set_curve_editor_open(false);
            }
        });
    }
    {
        let curve_editor = curve_editor.clone();
        let app_weak = app.as_weak();
        app.on_curve_cancel(move || {
            *curve_editor.lock().unwrap() = None;
            if let Some(app) = app_weak.upgrade() {
                app.set_curve_editor_open(false);
            }
        });
    }

    {
        let known_values = known_values.clone();
        let rgb_editor = rgb_editor.clone();
        let current_category = current_category.clone();
        let app_weak = app.as_weak();
        // Opens the LIGHTSYNC slot editor. The `attr` the button reports is
        // ignored (it is the synthetic Edit slot row's, or a legacy
        // `Kind::RgbStrip` row's): the editor always targets the ACTIVE
        // custom slot, seeded from the slot-scoped attrs' last-known
        // values.
        app.on_edit_rgb(move |_attr| {
            let Some(app) = app_weak.upgrade() else { return };
            let attr = "wheel_led_colors".to_string();
            let (mut colors, direction, slot, name, slot_brightness) = {
                let kv = known_values.lock().unwrap();
                let colors = match kv.get(&attr) {
                    Some(Value::Rgb(cs)) => cs.clone(),
                    _ => bridge::default_rgb(&attr),
                };
                let direction = match kv.get("wheel_led_direction") {
                    Some(Value::Enum(d)) => *d,
                    _ => 0,
                };
                let slot = match kv.get("wheel_led_slot") {
                    Some(Value::Int(n)) => (*n).clamp(0, lightsync::CUSTOM_SLOTS as i32 - 1),
                    _ => 0,
                };
                let name = match kv.get("wheel_led_slot_name") {
                    Some(Value::Text(s)) => s.clone(),
                    _ => String::new(),
                };
                let slot_brightness = match kv.get("wheel_led_slot_brightness") {
                    Some(Value::Percent(p)) => i32::from(*p),
                    _ => 100,
                };
                (colors, direction, slot, name, slot_brightness)
            };
            // A mirrored direction shows (and will write) paired colors,
            // left half winning, so the preview matches what the wheel
            // plays; see `lightsync::mirror_left_half`.
            if lightsync::mirrored(direction) {
                lightsync::mirror_left_half(&mut colors);
            }
            push_rgb_editor(&app, &colors);
            app.set_rgb_selected_hex("".into());
            app.set_rgb_label(format!("CUSTOM {}", slot + 1).into());
            app.set_rgb_slot_name(name.into());
            app.set_rgb_slot_brightness(slot_brightness);
            app.set_rgb_direction(i32::from(direction));
            app.set_rgb_editor_open(true);
            *rgb_editor.lock().unwrap() =
                Some(RgbEditorState { attr, category: get(&current_category), colors, direction });
        });
    }
    {
        let rgb_editor = rgb_editor.clone();
        let app_weak = app.as_weak();
        app.on_rgb_select_led(move |i| {
            let Some(app) = app_weak.upgrade() else { return };
            let guard = rgb_editor.lock().unwrap();
            let Some(state) = guard.as_ref() else { return };
            if let Some(c) = state.colors.get(i.max(0) as usize) {
                app.set_rgb_selected_hex(c.to_hex().into());
            }
        });
    }
    {
        let rgb_editor = rgb_editor.clone();
        let app_weak = app.as_weak();
        app.on_rgb_set_color(move |i, r, g, b| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = rgb_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            let index = i.max(0) as usize;
            bridge::apply_set_color(&mut state.colors, index, r, g, b);
            mirror_staged_swatch(state, index);
            push_rgb_editor(&app, &state.colors);
            if let Some(c) = state.colors.get(index) {
                app.set_rgb_selected_hex(c.to_hex().into());
            }
        });
    }
    {
        let rgb_editor = rgb_editor.clone();
        let app_weak = app.as_weak();
        app.on_rgb_set_hex(move |i, hex| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = rgb_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            let index = i.max(0) as usize;
            if bridge::apply_set_hex(&mut state.colors, index, &hex).is_ok() {
                mirror_staged_swatch(state, index);
                push_rgb_editor(&app, &state.colors);
                if let Some(c) = state.colors.get(index) {
                    app.set_rgb_selected_hex(c.to_hex().into());
                }
            }
        });
    }
    {
        let rgb_editor = rgb_editor.clone();
        let app_weak = app.as_weak();
        app.on_rgb_apply_to_all(move |r, g, b| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = rgb_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            bridge::apply_to_all(&mut state.colors, r, g, b);
            push_rgb_editor(&app, &state.colors);
            app.set_rgb_selected_hex(
                Color { r: r.clamp(0, 255) as u8, g: g.clamp(0, 255) as u8, b: b.clamp(0, 255) as u8 }
                    .to_hex()
                    .into(),
            );
        });
    }
    {
        let rgb_editor = rgb_editor.clone();
        let app_weak = app.as_weak();
        app.on_rgb_set_direction(move |d| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = rgb_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            state.direction = d.clamp(0, 3) as u8;
            app.set_rgb_direction(i32::from(state.direction));
            // Switching INTO a mirrored direction makes the pairs real
            // immediately (left half wins), so the preview shows what the
            // wheel will play rather than a half-truth until commit.
            if lightsync::mirrored(state.direction) {
                lightsync::mirror_left_half(&mut state.colors);
                push_rgb_editor(&app, &state.colors);
            }
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_rgb_set_name(move |name| {
            // The wheel takes at most 8 chars; trim the excess here so the
            // write cannot fail on length alone (the device's re-read
            // pushes its canonical, uppercased form back into the field).
            let name: String = name.chars().take(8).collect();
            worker.request(Request::Edit {
                category: get(&current_category),
                attr: "wheel_led_slot_name".to_string(),
                input: WidgetInput::Text(name),
            });
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_rgb_set_slot_brightness(move |v| {
            worker.request(Request::Edit {
                category: get(&current_category),
                attr: "wheel_led_slot_brightness".to_string(),
                input: WidgetInput::Slider(i64::from(v)),
            });
        });
    }
    {
        let worker = worker.clone();
        let rgb_editor = rgb_editor.clone();
        let app_weak = app.as_weak();
        app.on_rgb_commit(move || {
            // Colors, then direction, then the apply trigger: the driver
            // commits the active slot's staged config to the wheel on the
            // trigger, so it must run last. The name and slot brightness
            // already committed individually from their own callbacks.
            if let Some(state) = rgb_editor.lock().unwrap().take() {
                worker.request(Request::Edit {
                    category: state.category,
                    attr: state.attr,
                    input: WidgetInput::Rgb(state.colors),
                });
                worker.request(Request::Edit {
                    category: state.category,
                    attr: "wheel_led_direction".to_string(),
                    input: WidgetInput::Choice(usize::from(state.direction)),
                });
                worker.request(Request::Edit {
                    category: state.category,
                    attr: "wheel_led_apply".to_string(),
                    input: WidgetInput::Trigger,
                });
            }
            if let Some(app) = app_weak.upgrade() {
                app.set_rgb_editor_open(false);
            }
        });
    }
    {
        let rgb_editor = rgb_editor.clone();
        let app_weak = app.as_weak();
        app.on_rgb_cancel(move || {
            *rgb_editor.lock().unwrap() = None;
            if let Some(app) = app_weak.upgrade() {
                app.set_rgb_editor_open(false);
            }
        });
    }

    {
        let known_values = known_values.clone();
        let slot_text_editor = slot_text_editor.clone();
        let current_category = current_category.clone();
        let app_weak = app.as_weak();
        app.on_edit_slot_text(move |attr| {
            let Some(app) = app_weak.upgrade() else { return };
            let attr = attr.to_string();
            let names = match known_values.lock().unwrap().get(&attr) {
                Some(Value::SlotNames(ns)) => ns.clone(),
                _ => bridge::default_slot_names(&attr),
            };
            push_slot_text_editor(&app, &names);
            app.set_slot_text_error("".into());
            app.set_slot_text_max_len(bridge::slot_text_max_len(&attr));
            let label = REGISTRY.iter().find(|s| s.attr == attr).map(|s| s.label).unwrap_or(attr.as_str());
            app.set_slot_text_label(label.into());
            app.set_slot_text_editor_open(true);
            *slot_text_editor.lock().unwrap() =
                Some(SlotTextEditorState { attr, category: get(&current_category), names });
        });
    }
    {
        let worker = worker.clone();
        let slot_text_editor = slot_text_editor.clone();
        let app_weak = app.as_weak();
        app.on_slot_text_set_name(move |slot, name| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = slot_text_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            let slot = slot.max(0) as u8;
            bridge::apply_set_slot_name(&mut state.names, slot, &name);
            push_slot_text_editor(&app, &state.names);
            // A fresh edit clears the previous attempt's error; the reply's
            // RowUpdated re-sets it if this one fails too.
            app.set_slot_text_error("".into());
            // Kind::SlotText writes one slot at a time and reads back the
            // whole list, so this is the same "send an Edit, let the
            // category's next Rows response refresh everything" pattern the
            // other immediate-apply widgets (slider/choice/switch/text/
            // trigger) use, not the curve/RGB overlays' staged commit. The
            // reply's RowUpdated then pushes the device's re-read names
            // back into this overlay (see the worker-response closure), so
            // the optimistic push above is only a placeholder until the
            // authoritative one lands.
            worker.request(Request::Edit {
                category: state.category,
                attr: state.attr.clone(),
                input: WidgetInput::SlotText { slot, text: name.to_string() },
            });
        });
    }
    {
        let slot_text_editor = slot_text_editor.clone();
        let app_weak = app.as_weak();
        app.on_slot_text_close(move || {
            *slot_text_editor.lock().unwrap() = None;
            if let Some(app) = app_weak.upgrade() {
                app.set_slot_text_editor_open(false);
            }
        });
    }

    // Profiles page (desktop mode): the computer-side profile store. All
    // three run on the worker thread (file + sysfs I/O), which replies with
    // `Response::Profiles` (fresh list + status line); an apply also
    // triggers the Rows/Info refresh the settings need.
    {
        let worker = worker.clone();
        app.on_profile_save(move |name| {
            worker.request(Request::ProfileSave(name.to_string()));
        });
    }
    {
        let worker = worker.clone();
        app.on_profile_apply(move |name| {
            worker.request(Request::ProfileApply(name.to_string()));
        });
    }
    {
        let worker = worker.clone();
        app.on_profile_delete(move |name| {
            worker.request(Request::ProfileDelete(name.to_string()));
        });
    }

    // Setup page: the clipboard copy and the shim installer both shell out,
    // so both run off the UI thread (a plain `std::thread::spawn`, not the
    // category worker) rather than risk a slow `--all-steam` Proton-prefix
    // scan freezing the window.
    app.on_setup_copy_launch(move || {
        std::thread::spawn(|| copy_to_clipboard(FFB_LAUNCH_OPTIONS));
    });
    // The Info page's Serial/Firmware Copy buttons: same best-effort
    // clipboard mechanism, same off-the-UI-thread rule. The firmware
    // value keeps the driver's own base/motor line break, so it lands in
    // the clipboard as two lines.
    app.on_info_copy(move |text| {
        let text = text.to_string();
        std::thread::spawn(move || copy_to_clipboard(&text));
    });
    {
        let sdk_dir = sdk_dir.clone();
        let installer_path = installer_path.clone();
        let app_weak = app.as_weak();
        app.on_setup_sdk_dir_edited(move |text| {
            let text = text.to_string();
            let (resolved, message) = sdk_status(&text, installer_path.as_deref());
            *sdk_dir.lock().unwrap() = text;
            let Some(app) = app_weak.upgrade() else { return };
            app.set_setup_sdk_valid(resolved.is_some());
            app.set_setup_sdk_status(message.into());
        });
    }
    {
        let sdk_dir = sdk_dir.clone();
        let installer_path = installer_path.clone();
        let shim_binary = shim_binary.clone();
        let app_weak = app.as_weak();
        app.on_setup_install_game(move |prefix| {
            let Some(app) = app_weak.upgrade() else { return };
            app.set_setup_shim_output("Running...".into());
            app.set_setup_shim_running(true);
            let dir = sdk_dir.lock().unwrap().clone();
            // The install passes the RESOLVED dir explicitly, i.e. exactly
            // the directory the status line reports; nothing resolved
            // omits the flag and the installer's own error guidance runs.
            let resolved = logi_dd_core::steam::resolve_sdk_dir(&dir, installer_path.as_deref());
            let args = logi_dd_core::steam::shim_install_args(&prefix, resolved.as_deref());
            run_shim_command(app_weak.clone(), shim_binary.clone(), args);
        });
    }
    {
        let shim_binary = shim_binary.clone();
        let app_weak = app.as_weak();
        app.on_setup_remove_game(move |prefix| {
            let Some(app) = app_weak.upgrade() else { return };
            app.set_setup_shim_output("Running...".into());
            app.set_setup_shim_running(true);
            let args = vec!["--uninstall-prefix".to_string(), prefix.to_string()];
            run_shim_command(app_weak.clone(), shim_binary.clone(), args);
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_setup_rescan_games(move || scan_games(app_weak.clone()));
    }

    // Setup page: Simulated TrueForce. Every conf edit writes exactly one
    // key (logi_dd_core::tfsim's writers preserve everything else in the
    // file); the daemon and the sweep are plain child processes, stopped
    // with SIGTERM so their handlers send the wheel clean stop packets.
    {
        let app_weak = app.as_weak();
        app.on_setup_tf_set_enabled(move |v| {
            let Some(app) = app_weak.upgrade() else { return };
            report_tf_write(
                &app,
                logi_dd_core::tfsim::set_enabled_in(&logi_dd_core::tfsim::default_path(), v),
            );
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_setup_tf_set_intensity(move |v| {
            let Some(app) = app_weak.upgrade() else { return };
            report_tf_write(
                &app,
                logi_dd_core::tfsim::set_intensity_in(
                    &logi_dd_core::tfsim::default_path(),
                    v.clamp(0, 100) as u8,
                ),
            );
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_setup_tf_set_pitch(move |v| {
            let Some(app) = app_weak.upgrade() else { return };
            report_tf_write(
                &app,
                logi_dd_core::tfsim::set_pitch_in(
                    &logi_dd_core::tfsim::default_path(),
                    v.clamp(10, 200) as u8,
                ),
            );
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_setup_sim_game_enabled(move |id, v| {
            let Some(app) = app_weak.upgrade() else { return };
            report_tf_write(
                &app,
                logi_dd_core::tfsim::set_game_enabled_in(&logi_dd_core::tfsim::default_path(), &id, v),
            );
            // Both titles sharing the id must show the fresh value.
            refresh_compat_rows(&app);
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_setup_sim_game_intensity(move |id, v| {
            let Some(app) = app_weak.upgrade() else { return };
            report_tf_write(
                &app,
                logi_dd_core::tfsim::set_game_intensity_in(
                    &logi_dd_core::tfsim::default_path(),
                    &id,
                    v.clamp(0, 100) as u8,
                ),
            );
            refresh_compat_rows(&app);
        });
    }
    {
        let app_weak = app.as_weak();
        let tf_binary = tf_binary.clone();
        app.on_setup_tf_start_daemon(move || {
            // The button is disabled while unresolved, so this only races
            // a stale click; detached, like every helper spawn.
            let Some(bin) = tf_binary.clone() else { return };
            if let Ok(mut child) = std::process::Command::new(bin)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                // Reap the daemon whenever it exits: an un-reaped child
                // stays a zombie in our process table for the app's whole
                // lifetime (one parked thread is the cheapest cure).
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            refresh_tf_daemon(app_weak.clone(), Some(true));
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_setup_tf_stop_daemon(move || {
            if let Some(pid) = logi_dd_core::tfsim::daemon_pid() {
                send_sigterm(pid);
            }
            refresh_tf_daemon(app_weak.clone(), Some(false));
        });
    }
    // The running sweep's pid, `None` while no sweep plays; the Stop
    // button reads it, the runner thread owns its lifecycle.
    let tf_sweep_pid: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
    {
        let app_weak = app.as_weak();
        let tf_binary = tf_binary.clone();
        let tf_sweep_pid = tf_sweep_pid.clone();
        // Only ever fired from the consent dialog's Continue button.
        app.on_setup_tf_sweep(move || {
            let Some(app) = app_weak.upgrade() else { return };
            let Some(bin) = tf_binary.clone() else { return };
            if app.get_setup_tf_sweep_running() {
                return;
            }
            app.set_setup_tf_sweep_running(true);
            app.set_setup_tf_output("Running logi-tf-sim --sweep...".into());
            run_tf_sweep(app_weak.clone(), bin, tf_sweep_pid.clone());
        });
    }
    {
        let tf_sweep_pid = tf_sweep_pid.clone();
        app.on_setup_tf_sweep_stop(move || {
            // A stray click after the sweep finished finds the cell empty
            // and does nothing.
            if let Some(pid) = *tf_sweep_pid.lock().unwrap() {
                send_sigterm(pid);
            }
        });
    }

    // Test page: Rescan re-runs discovery (restarting the reader), and the
    // two sim callbacks only ever fire from the page's confirm dialog.
    {
        let app_weak = app.as_weak();
        let test_reader = test_reader.clone();
        let test_device = test_device.clone();
        app.on_test_rescan(move || {
            start_test_monitor(app_weak.clone(), test_reader.clone(), test_device.clone());
        });
    }
    {
        let app_weak = app.as_weak();
        let test_device = test_device.clone();
        let test_sim_cancel = test_sim_cancel.clone();
        app.on_test_sim_constant(move || {
            run_test_sim(app_weak.clone(), &test_device, &test_sim_cancel, testio::SimKind::ConstantForce);
        });
    }
    {
        let app_weak = app.as_weak();
        let test_device = test_device.clone();
        let test_sim_cancel = test_sim_cancel.clone();
        app.on_test_sim_texture(move || {
            run_test_sim(app_weak.clone(), &test_device, &test_sim_cancel, testio::SimKind::Texture);
        });
    }
    {
        // The Stop button: flag the runner thread, which stops + erases
        // the effect within its poll tick and then resets the UI state
        // itself (its completion path). A stray click after the runner
        // already finished finds the cell empty and does nothing.
        let test_sim_cancel = test_sim_cancel.clone();
        app.on_test_sim_stop(move || {
            if let Some(cancel) = test_sim_cancel.lock().unwrap().as_ref() {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
    }

    worker.request(Request::LoadCategory(get(&current_category)));

    let outcome = app.run();
    // The reader thread must not outlive the window (it holds an open fd
    // and a Weak<App> that would just go stale).
    stop_test_monitor(&test_reader);
    outcome
}
