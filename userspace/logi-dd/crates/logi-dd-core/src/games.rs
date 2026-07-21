//! Per-game force-feedback and TrueForce compatibility registry.
//!
//! Static, std-only reference data the Setup pages render: for each known
//! sim-racing title, whether it runs on Linux, how force feedback reaches
//! it, whether it carries genuine (SDK) TrueForce, whether logi-tf-sim can
//! synthesize TrueForce from its telemetry, and the one-line recommended
//! setup. The authoritative content is the project's game-compatibility
//! dataset; this is a faithful transcription, never a place to claim more
//! support than has actually been established.

use crate::tfsim;

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
    /// [`tfsim::DAEMON_GAME_IDS`]) so a front-end can render its live
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
        simulated_tf: SimTf::PossibleWithParser,
        setup: "Plain force feedback; for simulated TrueForce, enable OutGauge to 127.0.0.1:4444 once a parser lands.",
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
        simulated_tf: SimTf::PossibleWithParser,
        setup: "Enable UDP in config.json (port 20777); run logi-tf-sim (needs a WRC parser).",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "EA Sports F1 (F1 22-25)",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::PossibleWithParser,
        setup: "Enable in-game UDP telemetry (F1 format, port 20777); run logi-tf-sim (needs an F1 parser).",
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
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    games
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn provisional_flag_tracks_soft_confidence() {
        assert!(Confidence::Expected.is_provisional());
        assert!(Confidence::Unknown.is_provisional());
        assert!(!Confidence::Verified.is_provisional());
        assert!(!Confidence::Documented.is_provisional());
    }
}
