//! Per-game force-feedback and TrueForce compatibility registry.
//!
//! Static, std-only reference data the Setup pages render: for each known
//! sim-racing title, whether it runs on Linux, how force feedback reaches
//! it, whether it carries genuine (SDK) TrueForce, whether logi-tf-sim can
//! synthesize TrueForce from its telemetry, and the one-line recommended
//! setup. The authoritative content is the project's game-compatibility
//! dataset; this is a faithful transcription, never a place to claim more
//! support than has actually been established.

/// Whether a title runs on Linux, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Linux {
    /// Ships a native Linux build.
    Native,
    /// A Windows build run through Proton or Wine.
    Proton,
    /// Not playable on Linux (anti-cheat, storefront, or console-only).
    Unsupported,
}

impl Linux {
    pub fn label(self) -> &'static str {
        match self {
            Linux::Native => "Native Linux",
            Linux::Proton => "Proton",
            Linux::Unsupported => "Not on Linux",
        }
    }
}

/// How force feedback is delivered to the wheel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ffb {
    /// Standard Linux force feedback: the wheel is an ordinary force
    /// feedback device and it works out of the box.
    NativeEvdev,
    /// The game drives feedback through the older Windows DirectInput path
    /// inside Proton; it needs the logi-ffb proxy (or HIDRAW turned off).
    DirectInput,
    /// The game itself calls Logitech's TrueForce SDK; the shim feeds those
    /// calls to the wheel.
    TrueForceShim,
}

impl Ffb {
    pub fn label(self) -> &'static str {
        match self {
            Ffb::NativeEvdev => "Native FFB",
            Ffb::DirectInput => "logi-ffb",
            Ffb::TrueForceShim => "TrueForce shim",
        }
    }
}

/// A yes / no / expected support answer (used for native TrueForce).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Yes,
    No,
    /// Marketed or likely, but not confirmed on this driver.
    Expected,
}

impl Support {
    pub fn label(self) -> &'static str {
        match self {
            Support::Yes => "Yes",
            Support::No => "No",
            Support::Expected => "Expected",
        }
    }
}

/// Whether logi-tf-sim can synthesize TrueForce from the title's telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimTf {
    /// Works today; carries the daemon's per-game id (one of
    /// [`crate::tfsim::DAEMON_GAME_IDS`]) so a front-end can render its live
    /// per-game toggle.
    LiveNow(&'static str),
    /// A telemetry source exists, but the daemon needs a new parser first.
    PossibleWithParser,
    /// No usable telemetry (or none documented).
    No,
    /// Not applicable: the title already delivers real TrueForce, so there
    /// is nothing to synthesize.
    NotApplicableNative,
}

impl SimTf {
    /// The static cell text for a non-live title. A [`SimTf::LiveNow`] title
    /// renders its own toggle instead, so "Live" is only its fallback label.
    pub fn label(self) -> &'static str {
        match self {
            SimTf::LiveNow(_) => "Live",
            SimTf::PossibleWithParser => "possible (needs a parser)",
            SimTf::No => "no",
            SimTf::NotApplicableNative => "n/a (native)",
        }
    }

    /// The daemon game id for a live title, else `None`.
    pub fn live_id(self) -> Option<&'static str> {
        match self {
            SimTf::LiveNow(id) => Some(id),
            _ => None,
        }
    }
}

/// How firmly a row's information is established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// Confirmed end-to-end by this project.
    Verified,
    /// Documented by the vendor or a reliable community source.
    Documented,
    /// Expected to work, not confirmed.
    Expected,
    /// Genuinely unknown.
    Unknown,
}

impl Confidence {
    pub fn label(self) -> &'static str {
        match self {
            Confidence::Verified => "verified",
            Confidence::Documented => "documented",
            Confidence::Expected => "expected",
            Confidence::Unknown => "unknown",
        }
    }

    /// Whether a title should carry the "not verified on this driver yet"
    /// marker: true for the softer `Expected` / `Unknown` rows.
    pub fn is_provisional(self) -> bool {
        matches!(self, Confidence::Expected | Confidence::Unknown)
    }
}

/// What a game needs to get the best out of the wheel: the single
/// enablement action the Setup page's "Your games" list offers for it.
/// Derived from a title's [`Ffb`] and [`SimTf`] (see
/// [`GameCompat::setup_action`]) so both front-ends classify a game the
/// same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupAction {
    /// A native-TrueForce sim (ACC, AC EVO): install the TrueForce shim so
    /// the game's own TrueForce reaches the wheel.
    InstallShim,
    /// An older DirectInput game (Le Mans Ultimate, rFactor 2): launch it
    /// with the logi-ffb helper so it gets force feedback at all.
    UseLogiFfb,
    /// A game logi-tf-sim can drive from telemetry today: offer its
    /// per-game simulated-TrueForce switch.
    SimulatedTrueForce,
    /// Plain force feedback works with nothing to install.
    WorksOutOfBox,
}

/// One title's compatibility facts.
#[derive(Debug, Clone, Copy)]
pub struct GameCompat {
    pub name: &'static str,
    pub linux: Linux,
    pub ffb: Ffb,
    pub native_trueforce: Support,
    pub simulated_tf: SimTf,
    /// One-line, plain-English recommended action.
    pub setup: &'static str,
    pub confidence: Confidence,
}

impl GameCompat {
    /// The "Force feedback" cell: how feedback reaches the title, or a plain
    /// "Not on Linux" for titles that do not run here at all (their stored
    /// [`Ffb`] then only describes the Windows or console situation and is
    /// never surfaced).
    pub fn ffb_cell(&self) -> &'static str {
        match self.linux {
            Linux::Unsupported => "Not on Linux",
            _ => self.ffb.label(),
        }
    }

    /// The single enablement action the Setup page offers for this title
    /// (see [`SetupAction`]). A native-TrueForce sim wants the shim; a
    /// title logi-tf-sim can drive today wants its simulated-TrueForce
    /// switch; a DirectInput title wants the logi-ffb helper; everything
    /// else already works with plain force feedback.
    pub fn setup_action(&self) -> SetupAction {
        if self.ffb == Ffb::TrueForceShim {
            SetupAction::InstallShim
        } else if self.simulated_tf.live_id().is_some() {
            SetupAction::SimulatedTrueForce
        } else if self.ffb == Ffb::DirectInput {
            SetupAction::UseLogiFfb
        } else {
            SetupAction::WorksOutOfBox
        }
    }
}

/// Normalize a game title for fuzzy matching: lower-cased, trademark marks
/// removed, any parenthetical suffix (e.g. "(early access)", "(original)")
/// dropped, and every run of non-alphanumeric characters (spaces, dots,
/// dashes, colons, ...) collapsed to a single space. Steam's display name,
/// the registry name, and a launcher slug (e.g. a Lutris file stem like
/// "dirt-rally-2-0") all pass through this before they are compared, so
/// "Assetto Corsa EVO" matches "Assetto Corsa EVO (early access)" and
/// "dirt rally 2 0" matches "DiRT Rally 2.0".
fn normalize_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut depth: i32 = 0;
    for ch in title.chars() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            '\u{2122}' | '\u{00ae}' | '\u{00a9}' => {} // (TM) (R) (C)
            _ if depth > 0 => {}
            _ if ch.is_alphanumeric() => out.extend(ch.to_lowercase()),
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Best-effort match of an installed Steam game's display name to a
/// registry entry, for the Setup page's "Your games" list. An exact
/// normalized-name match ([`normalize_title`]) wins; the four EA Sports F1
/// season titles (F1 22-25) fall back to the single "EA Sports F1" row,
/// mirroring the family handling in `tfsim::game_id_for_title`. Returns
/// `None` when nothing matches confidently, so an unknown game is shown as
/// "no special setup needed" rather than mislabeled.
pub fn match_title(steam_name: &str) -> Option<&'static GameCompat> {
    let target = normalize_title(steam_name);
    if target.is_empty() {
        return None;
    }
    if let Some(g) = GAMES.iter().find(|g| normalize_title(g.name) == target) {
        return Some(g);
    }
    let f1 = matches!(target.as_str(), "f1 22" | "f1 23" | "f1 24" | "f1 25");
    if f1 {
        return GAMES.iter().find(|g| g.name == "EA Sports F1 (F1 22-25)");
    }
    None
}

/// Every title the project has compatibility information about. Transcribed
/// from the game-compatibility dataset in its curated order; front-ends
/// display them sorted (see [`sorted_by_name`]).
pub const GAMES: &[GameCompat] = &[
    GameCompat {
        name: "Assetto Corsa Competizione",
        linux: Linux::Proton,
        ffb: Ffb::TrueForceShim,
        native_trueforce: Support::Yes,
        simulated_tf: SimTf::NotApplicableNative,
        setup: "Install the TrueForce shim; set PROTON_ENABLE_HIDRAW=1; turn Steam Input off.",
        confidence: Confidence::Verified,
    },
    GameCompat {
        name: "Assetto Corsa EVO (early access)",
        linux: Linux::Proton,
        ffb: Ffb::TrueForceShim,
        native_trueforce: Support::Yes,
        simulated_tf: SimTf::NotApplicableNative,
        setup: "Install the TrueForce shim; set PROTON_ENABLE_HIDRAW=1; turn Steam Input off.",
        confidence: Confidence::Verified,
    },
    GameCompat {
        name: "Assetto Corsa (original)",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::No,
        setup: "Plain force feedback; no shim; turn Steam Input off.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "Automobilista 2",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::LiveNow("ams2-pcars2"),
        setup: "Enable UDP telemetry (Project CARS 2 format) to 127.0.0.1; run logi-tf-sim; Steam Input off.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "Project CARS 2",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::LiveNow("ams2-pcars2"),
        setup: "Enable UDP telemetry (Project CARS 2 format); run logi-tf-sim.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "rFactor 2",
        linux: Linux::Proton,
        ffb: Ffb::DirectInput,
        native_trueforce: Support::No,
        simulated_tf: SimTf::PossibleWithParser,
        setup: "Set PROTON_ENABLE_HIDRAW=0, or launch with logi-ffb %command%; Steam Input off.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "Le Mans Ultimate",
        linux: Linux::Proton,
        ffb: Ffb::DirectInput,
        native_trueforce: Support::No,
        simulated_tf: SimTf::PossibleWithParser,
        setup: "Set PROTON_ENABLE_HIDRAW=0, or launch with logi-ffb %command%; Steam Input off.",
        confidence: Confidence::Verified,
    },
    GameCompat {
        name: "iRacing",
        linux: Linux::Proton,
        ffb: Ffb::DirectInput,
        native_trueforce: Support::No,
        simulated_tf: SimTf::PossibleWithParser,
        setup: "Now Linux-playable; set PROTON_ENABLE_HIDRAW=0 or launch with logi-ffb %command%; Steam Input off.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "RaceRoom Racing Experience",
        linux: Linux::Proton,
        ffb: Ffb::DirectInput,
        native_trueforce: Support::No,
        simulated_tf: SimTf::PossibleWithParser,
        setup: "Set PROTON_ENABLE_HIDRAW=0, or launch with logi-ffb %command%; Steam Input off.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "BeamNG.drive",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::Expected,
        simulated_tf: SimTf::LiveNow("beamng"),
        setup: "Plain force feedback; for simulated TrueForce, enable OutGauge to 127.0.0.1:4444 and run logi-tf-sim.",
        confidence: Confidence::Expected,
    },
    GameCompat {
        name: "DiRT Rally 2.0",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::Expected,
        simulated_tf: SimTf::LiveNow("dirt-rally-2"),
        setup: "Enable in-game UDP telemetry (Codemasters, port 20777); run logi-tf-sim; Steam Input off.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "DiRT 4",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::LiveNow("codemasters"),
        setup: "Enable UDP telemetry (Codemasters format); run logi-tf-sim.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "EA Sports WRC",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::LiveNow("ea-wrc"),
        setup: "Add the logi-tf-sim WRC packet to config.json (UDP to 127.0.0.1:20777); run logi-tf-sim.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "EA Sports F1 (F1 22-25)",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::LiveNow("f1"),
        setup: "Enable in-game UDP telemetry (F1 format, port 20777); run logi-tf-sim.",
        confidence: Confidence::Expected,
    },
    GameCompat {
        name: "Richard Burns Rally",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::PossibleWithParser,
        setup: "Plain force feedback; a telemetry plugin can feed logi-tf-sim.",
        confidence: Confidence::Expected,
    },
    GameCompat {
        name: "Wreckfest",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::No,
        setup: "Plain force feedback; turn Steam Input off.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "Assetto Corsa Rally (early access)",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::No,
        setup: "Plain force feedback; watch for telemetry as it matures.",
        confidence: Confidence::Unknown,
    },
    GameCompat {
        name: "Euro Truck Simulator 2",
        linux: Linux::Native,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::PossibleWithParser,
        setup: "Plain force feedback on the native Linux build; no shim.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "American Truck Simulator",
        linux: Linux::Native,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::PossibleWithParser,
        setup: "Plain force feedback on the native Linux build; no shim.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "KartKraft",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::PossibleWithParser,
        setup: "Plain force feedback; a telemetry parser could be added later.",
        confidence: Confidence::Unknown,
    },
    GameCompat {
        name: "CarX Drift Racing Online",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::No,
        setup: "Plain force feedback; turn Steam Input off.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "GRID (2019)",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::Expected,
        simulated_tf: SimTf::PossibleWithParser,
        setup: "Plain force feedback; the TrueForce shim is worth trying; Steam Input off.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "GRID Legends",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::PossibleWithParser,
        setup: "Plain force feedback; turn Steam Input off.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "Forza Motorsport (2023)",
        linux: Linux::Unsupported,
        ffb: Ffb::TrueForceShim,
        native_trueforce: Support::Yes,
        simulated_tf: SimTf::No,
        setup: "Not on Linux (anti-cheat / storefront); TrueForce here is a Windows-only story.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "Forza Horizon 5",
        linux: Linux::Unsupported,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::No,
        setup: "Not reliably on Linux (anti-cheat).",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "Gran Turismo 7",
        linux: Linux::Unsupported,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::No,
        setup: "PlayStation 5 only; not a Linux target.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "Dakar Desert Rally",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::No,
        setup: "Plain force feedback.",
        confidence: Confidence::Unknown,
    },
    GameCompat {
        name: "Rennsport",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::No,
        setup: "Anti-cheat may block some builds; verify per release.",
        confidence: Confidence::Unknown,
    },
];

/// Every title, sorted case-insensitively by name: the friendly order for a
/// lookup table (a user hunting one game reads alphabetically).
pub fn sorted_by_name() -> Vec<&'static GameCompat> {
    let mut games: Vec<&'static GameCompat> = GAMES.iter().collect();
    games.sort_by_key(|g| g.name.to_lowercase());
    games
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tfsim;
    use std::collections::BTreeSet;

    #[test]
    fn registry_is_non_empty() {
        assert!(!GAMES.is_empty());
    }

    #[test]
    fn names_are_unique() {
        let names: BTreeSet<&str> = GAMES.iter().map(|g| g.name).collect();
        assert_eq!(names.len(), GAMES.len(), "duplicate game name in GAMES");
    }

    #[test]
    fn every_live_id_is_a_real_daemon_id() {
        for g in GAMES {
            if let Some(id) = g.simulated_tf.live_id() {
                assert!(
                    tfsim::DAEMON_GAME_IDS.contains(&id),
                    "{} claims live sim TF via unknown daemon id {id:?}",
                    g.name
                );
            }
        }
    }

    #[test]
    fn live_titles_match_the_daemons_real_parsers() {
        let live: BTreeSet<(&str, &str)> = GAMES
            .iter()
            .filter_map(|g| g.simulated_tf.live_id().map(|id| (g.name, id)))
            .collect();
        let expected: BTreeSet<(&str, &str)> = [
            ("Automobilista 2", "ams2-pcars2"),
            ("Project CARS 2", "ams2-pcars2"),
            ("DiRT Rally 2.0", "dirt-rally-2"),
            ("DiRT 4", "codemasters"),
            ("BeamNG.drive", "beamng"),
            ("EA Sports F1 (F1 22-25)", "f1"),
            ("EA Sports WRC", "ea-wrc"),
        ]
        .into_iter()
        .collect();
        assert_eq!(live, expected);
    }

    #[test]
    fn enums_render_expected_short_labels() {
        assert_eq!(Linux::Native.label(), "Native Linux");
        assert_eq!(Linux::Proton.label(), "Proton");
        assert_eq!(Linux::Unsupported.label(), "Not on Linux");

        assert_eq!(Ffb::NativeEvdev.label(), "Native FFB");
        assert_eq!(Ffb::DirectInput.label(), "logi-ffb");
        assert_eq!(Ffb::TrueForceShim.label(), "TrueForce shim");

        assert_eq!(Support::Yes.label(), "Yes");
        assert_eq!(Support::No.label(), "No");
        assert_eq!(Support::Expected.label(), "Expected");

        assert_eq!(SimTf::LiveNow("x").label(), "Live");
        assert_eq!(SimTf::PossibleWithParser.label(), "possible (needs a parser)");
        assert_eq!(SimTf::No.label(), "no");
        assert_eq!(SimTf::NotApplicableNative.label(), "n/a (native)");

        assert_eq!(Confidence::Verified.label(), "verified");
        assert_eq!(Confidence::Unknown.label(), "unknown");
    }

    #[test]
    fn unsupported_titles_report_not_on_linux_for_ffb() {
        for g in GAMES.iter().filter(|g| g.linux == Linux::Unsupported) {
            assert_eq!(g.ffb_cell(), "Not on Linux");
        }
    }

    #[test]
    fn sorted_is_alphabetical_and_complete() {
        let sorted = sorted_by_name();
        assert_eq!(sorted.len(), GAMES.len());
        for pair in sorted.windows(2) {
            assert!(pair[0].name.to_lowercase() <= pair[1].name.to_lowercase());
        }
    }

    #[test]
    fn match_title_handles_exact_parenthetical_and_trademark_names() {
        // Steam's plain display names match the registry's fuller names.
        assert_eq!(
            match_title("Assetto Corsa Competizione").map(|g| g.name),
            Some("Assetto Corsa Competizione")
        );
        // The parenthetical registry suffix is dropped for the compare.
        assert_eq!(
            match_title("Assetto Corsa EVO").map(|g| g.name),
            Some("Assetto Corsa EVO (early access)")
        );
        assert_eq!(
            match_title("Assetto Corsa").map(|g| g.name),
            Some("Assetto Corsa (original)")
        );
        // Trademark marks and casing do not block a match.
        assert_eq!(match_title("EA SPORTS\u{2122} WRC").map(|g| g.name), Some("EA Sports WRC"));
        assert_eq!(match_title("DiRT Rally 2.0").map(|g| g.name), Some("DiRT Rally 2.0"));
    }

    #[test]
    fn match_title_maps_f1_season_titles_to_the_family_row() {
        for title in ["F1 22", "F1 23", "F1 24", "F1 25"] {
            assert_eq!(
                match_title(title).map(|g| g.name),
                Some("EA Sports F1 (F1 22-25)"),
                "{title} should ride the EA Sports F1 row"
            );
        }
    }

    #[test]
    fn normalize_title_is_punctuation_insensitive() {
        // A launcher slug like a Lutris file stem ("dirt-rally-2-0",
        // hyphens replaced with spaces) must normalize the same as the
        // registry's punctuated name.
        assert_eq!(normalize_title("DiRT Rally 2.0"), normalize_title("dirt rally 2 0"));
    }

    #[test]
    fn match_title_returns_none_for_unknown_games() {
        assert!(match_title("TEKKEN 8").is_none());
        assert!(match_title("").is_none());
        assert!(match_title("   ").is_none());
    }

    #[test]
    fn setup_action_classifies_each_ffb_and_sim_combination() {
        let action = |name: &str| GAMES.iter().find(|g| g.name == name).unwrap().setup_action();
        // Native-TrueForce sims want the shim.
        assert_eq!(action("Assetto Corsa Competizione"), SetupAction::InstallShim);
        // Live simulated-TF titles want their per-game switch, even when
        // their base force feedback is native evdev.
        assert_eq!(action("Automobilista 2"), SetupAction::SimulatedTrueForce);
        assert_eq!(action("DiRT Rally 2.0"), SetupAction::SimulatedTrueForce);
        // DirectInput titles want logi-ffb.
        assert_eq!(action("Le Mans Ultimate"), SetupAction::UseLogiFfb);
        // Plain native force feedback needs nothing.
        assert_eq!(action("Wreckfest"), SetupAction::WorksOutOfBox);
    }

    #[test]
    fn provisional_flag_tracks_soft_confidence() {
        assert!(Confidence::Expected.is_provisional());
        assert!(Confidence::Unknown.is_provisional());
        assert!(!Confidence::Verified.is_provisional());
        assert!(!Confidence::Documented.is_provisional());
    }
}
