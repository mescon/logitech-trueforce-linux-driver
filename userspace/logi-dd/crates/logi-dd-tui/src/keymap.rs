//! One source of truth for every key binding: the `?` help overlay and the
//! footer hints both render from the tables `sections` builds, so the two
//! can never drift apart. Each binding says whether the slim footer shows
//! it (the 4-5 most contextual keys); the overlay shows everything.

use crate::app::{App, Focus};
use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::Category;

pub struct Binding {
    pub keys: &'static str,
    pub action: &'static str,
    /// Whether the slim footer line carries this binding too.
    pub footer: bool,
}

const fn b(keys: &'static str, action: &'static str) -> Binding {
    Binding { keys, action, footer: false }
}

const fn bf(keys: &'static str, action: &'static str) -> Binding {
    Binding { keys, action, footer: true }
}

pub struct Section {
    pub title: &'static str,
    pub bindings: Vec<Binding>,
}

/// The global navigation keys, appended to every non-text context.
fn globals() -> Section {
    Section {
        title: "Global",
        bindings: vec![
            b("1-7", "jump to that view"),
            b("Tab", "switch focus: sidebar / content"),
            b("Esc", "close the topmost thing, else back to the sidebar"),
            b("?", "this key list"),
            b("q", "quit"),
        ],
    }
}

/// How much of the keymap is reachable from the current context.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// A pane-level view: the globals apply on top of the context keys.
    View,
    /// A modal editor that owns every key it lists; only `?` stays live
    /// on top, so the globals are left out.
    Modal,
    /// Text entry (or an any-key/confirm state): every key is input,
    /// nothing else is reachable, not even `?`.
    TextEntry,
}

/// The keymap for the app's current state: the topmost active context
/// first (a modal or text editor when one is open, the focused pane
/// otherwise), then the globals where they still apply.
pub fn sections<S: SysfsIo>(app: &App<S>) -> Vec<Section> {
    let (context, scope) = context_section(app);
    match scope {
        Scope::View => vec![context, globals()],
        Scope::Modal | Scope::TextEntry => vec![context],
    }
}

/// The context's own section plus its scope.
fn context_section<S: SysfsIo>(app: &App<S>) -> (Section, Scope) {
    use Category::*;
    if app.info_popup.is_some() {
        return (
            Section { title: "Info popup", bindings: vec![bf("any key", "close")] },
            Scope::TextEntry,
        );
    }
    if app.curve_edit.is_some() {
        return (
            Section {
                title: "Curve editor",
                bindings: vec![
                    bf("Up/Down", "field"),
                    bf("Left/Right", "adjust"),
                    b("+", "add point"),
                    b("-", "delete point"),
                    bf("Enter", "save"),
                    bf("Esc", "cancel"),
                ],
            },
            Scope::Modal,
        );
    }
    if let Some(picker) = &app.color_picker {
        if picker.hex.is_some() {
            return (
                Section {
                    title: "LED color: hex entry",
                    bindings: vec![
                        bf("0-9 a-f", "type 6 hex digits"),
                        b("Backspace", "erase"),
                        bf("Enter", "apply to this LED"),
                        bf("Esc", "back to the picker"),
                    ],
                },
                Scope::TextEntry,
            );
        }
        return (
            Section {
                title: "Color picker",
                bindings: vec![
                    bf("Tab", "focus LEDs / palette"),
                    bf("arrows", "move (Home/End: first/last LED)"),
                    bf("Enter", "paint the LED"),
                    b("a", "paint all LEDs"),
                    b("p", "paint the LED and its mirror pair"),
                    b("x", "hex entry for the LED"),
                    bf("w", "write to the wheel"),
                    bf("Esc", "cancel"),
                ],
            },
            Scope::Modal,
        );
    }
    if app.effect_edit.is_some() {
        return (
            Section {
                title: "Effect selector",
                bindings: vec![
                    bf("Left/Right", "choose"),
                    bf("Enter", "apply"),
                    bf("Esc", "cancel"),
                ],
            },
            Scope::Modal,
        );
    }
    if app.edit.is_some() {
        return (
            Section {
                title: "Editing",
                bindings: vec![
                    bf("Left/Right", "adjust / pick slot"),
                    bf("type", "text"),
                    b("Backspace", "erase"),
                    bf("Enter", "commit"),
                    bf("Esc", "cancel"),
                ],
            },
            Scope::TextEntry,
        );
    }
    if app.profile_name_edit.is_some() {
        return (text_field("Profile name"), Scope::TextEntry);
    }
    if app.profile_delete_confirm.is_some() {
        return (confirm("Delete profile"), Scope::TextEntry);
    }
    if app.is_setup() {
        if app.sdk_edit.is_some() {
            return (text_field("SDK folder"), Scope::TextEntry);
        }
        if app.tf_intensity_edit.is_some() {
            return (digit_field("TF intensity (0-100)"), Scope::TextEntry);
        }
        if app.tf_pitch_edit.is_some() {
            return (digit_field("TF pitch (10-200)"), Scope::TextEntry);
        }
        if app.tf_sweep_confirm {
            return (confirm("Test sweep"), Scope::TextEntry);
        }
        return (setup_section(app), Scope::View);
    }
    if app.is_info() {
        if app.test.confirm.is_some() {
            return (confirm("Simulation"), Scope::TextEntry);
        }
        if app.focus == Focus::Sidebar {
            return (sidebar(), Scope::View);
        }
        let mut keys = vec![
            bf("Up/Down", "scroll (PgUp/PgDn: page)"),
        ];
        if app.test.sim_running() {
            keys.push(bf("s", "stop the simulation"));
        } else {
            keys.push(bf("f", "force feedback sim"));
            keys.push(bf("t", "TrueForce texture sim"));
        }
        keys.push(b("c", "show serial + versions for copying"));
        keys.push(bf("r", "rescan wheel + input"));
        keys.push(b("d", "toggle desktop/onboard mode"));
        return (Section { title: "Info / Testing", bindings: keys }, Scope::View);
    }
    if app.focus == Focus::Sidebar {
        return (sidebar(), Scope::View);
    }
    // A plain settings view (content focus).
    let mut keys = vec![
        bf("Up/Down", "select a row"),
        bf("Enter", "edit / apply / run"),
        bf("i", "explain the setting"),
    ];
    if app.rows.iter().any(|r| r.attr == crate::app::PROFILE_NEW_ATTR) {
        keys.push(bf("n", "save current as a new profile"));
        keys.push(b("d", "delete profile (mode toggle on the Mode row)"));
    } else {
        keys.push(bf("d", "toggle desktop/onboard mode"));
    }
    if app.has_shaping_toggle() {
        keys.push(b("a", "sensitivity/curve for the row's axis"));
    }
    keys.push(b("r", "refresh"));
    let title = if app.no_wheel {
        "Settings (no wheel)"
    } else {
        match app.category() {
            Ffb => "Force feedback",
            Steering => "Steering",
            Pedals => "Pedals",
            Leds => "LIGHTSYNC",
            Profiles => "Profiles / mode",
            Info => "Info / Testing",
        }
    };
    (Section { title, bindings: keys }, Scope::View)
}

/// The Setup view's context bindings, scoped to the selected section and
/// whether its inner state is entered.
fn setup_section<S: SysfsIo>(app: &App<S>) -> Section {
    use crate::app::SetupSection;
    if app.focus == Focus::Sidebar {
        return sidebar();
    }
    let mut keys: Vec<Binding> = Vec::new();
    if app.setup_inside {
        match app.setup_section() {
            SetupSection::Games => {
                keys.push(bf("Up/Down", "select a game"));
                keys.push(bf("i", "install the SDK shim"));
                keys.push(bf("u", "remove the SDK shim"));
                keys.push(bf("g", "toggle simulated TF for the game"));
                keys.push(bf("Esc/Left", "back to the sections"));
            }
            SetupSection::SimTf => {
                keys.push(bf("m", "master on/off"));
                keys.push(bf("e", "intensity"));
                keys.push(bf("p", "pitch"));
                keys.push(bf("d", "start/stop the daemon"));
                keys.push(bf("t", "play a test sweep"));
                keys.push(b("Esc/Left", "back to the sections"));
            }
            _ => {}
        }
    } else {
        keys.push(bf("Up/Down", "select a section"));
        keys.push(bf("Enter/Right", "open the section"));
        if app.setup_section() == SetupSection::Sdk {
            keys.push(bf("s", "edit the SDK folder"));
        }
    }
    if app.tf_sweep_active() {
        keys.push(bf("s", "stop the sweep"));
    }
    keys.push(b("r", "rescan games + daemon"));
    keys.push(b("PgUp/PgDn", "scroll"));
    Section { title: "Setup", bindings: keys }
}

fn sidebar() -> Section {
    Section {
        title: "Sidebar",
        bindings: vec![
            bf("Up/Down", "choose a view (loads live)"),
            bf("Enter/Right", "into the view's content"),
        ],
    }
}

fn text_field(title: &'static str) -> Section {
    Section {
        title,
        bindings: vec![
            bf("type", "text"),
            b("Backspace", "erase"),
            bf("Enter", "save"),
            bf("Esc", "cancel"),
        ],
    }
}

fn digit_field(title: &'static str) -> Section {
    Section {
        title,
        bindings: vec![
            bf("digits", "value"),
            b("Backspace", "erase"),
            bf("Enter", "save"),
            bf("Esc", "cancel"),
        ],
    }
}

fn confirm(title: &'static str) -> Section {
    Section {
        title,
        bindings: vec![bf("y", "confirm"), bf("any other key", "cancel")],
    }
}

/// The slim footer line: the context's footer-flagged bindings (the 4-5
/// most contextual keys) plus the pointer to the full overlay. Text-entry
/// contexts leave the pointer off: `?` is input there.
pub fn footer<S: SysfsIo>(app: &App<S>) -> String {
    let (_, scope) = context_section(app);
    let mut parts: Vec<String> = sections(app)
        .iter()
        .flat_map(|s| s.bindings.iter())
        .filter(|b| b.footer)
        .map(|b| format!("{} {}", b.keys, b.action_short()))
        .collect();
    if scope != Scope::TextEntry {
        parts.push("? keys".to_string());
    }
    parts.join("   ")
}

impl Binding {
    /// The footer's compact wording: everything up to the first
    /// parenthesis, so the overlay can elaborate without bloating the
    /// footer line.
    fn action_short(&self) -> &str {
        self.action.split(" (").next().unwrap_or(self.action).trim_end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use logi_dd_core::sysfs::FakeSysfs;

    fn app() -> App<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_strength", "62");
        fs.set("wheel_range", "900");
        fs.set("wheel_mode", "desktop");
        App::new(logi_dd_core::Device::with_io(fs))
    }

    /// Every footer entry must come from the same table the overlay
    /// renders: the one-source-of-truth guarantee.
    fn footer_subset_of_overlay(a: &App<FakeSysfs>) {
        let overlay = sections(a);
        let f = footer(a);
        for part in f.split("   ") {
            if part == "? keys" {
                continue;
            }
            let keys = part.split(' ').next().unwrap();
            assert!(
                overlay.iter().any(|s| s.bindings.iter().any(|b| b.keys == keys)),
                "footer key '{keys}' missing from the overlay: {f}"
            );
        }
    }

    #[test]
    fn footer_always_renders_from_the_overlay_table() {
        use crossterm::event::KeyCode;
        let mut a = app();
        footer_subset_of_overlay(&a); // sidebar focus
        a.focus = Focus::Content;
        footer_subset_of_overlay(&a); // settings content
        a.on_key(KeyCode::Enter); // inline editor
        assert!(a.edit.is_some());
        footer_subset_of_overlay(&a);
        a.on_key(KeyCode::Esc);
        for idx in 0..=crate::app::SETUP_INDEX {
            a.set_cat(idx);
            a.games_scanned = true;
            footer_subset_of_overlay(&a);
        }
    }

    #[test]
    fn footer_stays_slim_and_points_at_the_overlay() {
        let mut a = app();
        a.focus = Focus::Content;
        let f = footer(&a);
        let hints = f.split("   ").count();
        assert!(hints <= 6, "at most 5 keys + the pointer: {f}");
        assert!(f.ends_with("? keys"), "footer points at the overlay: {f}");
    }

    #[test]
    fn text_entry_contexts_stand_alone_without_the_globals() {
        use crossterm::event::KeyCode;
        let mut a = app();
        a.focus = Focus::Content;
        a.profile_name_edit = Some(String::new());
        let s = sections(&a);
        assert_eq!(s.len(), 1, "typing states show only their own keys");
        assert!(!footer(&a).contains("? keys"), "? is input while typing");
        a.profile_name_edit = None;
        a.on_key(KeyCode::Char('?'));
        assert!(a.help, "? opens the overlay outside text entry");
        a.on_key(KeyCode::Char('q'));
        assert!(!a.help, "any key closes it");
        assert!(!a.quit, "the closing key does nothing else");
    }
}
