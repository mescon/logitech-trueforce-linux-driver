//! Per-game force-feedback and TrueForce compatibility registry.
//!
//! Static, std-only reference data the Setup pages render: for each known
//! sim-racing title, whether it runs on Linux, how force feedback reaches
//! it, whether it carries genuine (SDK) TrueForce, whether logi-tf-sim can
//! synthesize TrueForce from its telemetry, and the one-line recommended
//! setup. The authoritative content is the project's game-compatibility
//! dataset; this is a faithful transcription, never a place to claim more
//! support than has actually been established.
//!
//! A row states what a title needs; what the *user* needs also depends on
//! which wheel is attached, so the recipe accessors take a [`WheelCaps`].

use crate::device::WheelModel;

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
    ///
    /// Read this as "on a wheel that can receive it". These titles hand
    /// their TrueForce to Logitech's SDK, which only the direct-drive wheels
    /// answer, so on a G923 the game is producing TrueForce that never
    /// arrives. Where this project can synthesize a substitute, the title is
    /// [`SimTf::LiveNow`] instead, even though its TrueForce is real.
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

/// Steam appids for the titles whose setup advice depends on knowing which
/// game is installed.
///
/// This exists because `tools/setup.sh` kept its own hand-written copy of the
/// same knowledge and it drifted twice: first as one undifferentiated list
/// that told DirectInput sims to set `PROTON_ENABLE_HIDRAW=1` (the setting
/// that stops force feedback reaching them), and then, after that was split,
/// as a DirectInput list carrying two of the four titles. The registry knows
/// which game needs what; the appid is the only piece it was missing, so it
/// lives here now and the shell lists are checked against it by a test.
///
/// Only titles that run on Linux and whose `Ffb` implies a launch-option
/// check are listed. A title nobody can install on Linux needs no appid, and
/// inventing one would be inventing a fact.
pub const STEAM_APPIDS: &[(&str, u32)] = &[
    // Every id here was resolved against Steam's own store API and confirmed
    // by the name it returns, not from memory. A wrong id is worse than a
    // missing one: it applies another game's recipe to a real title, and the
    // owner has no way to tell that is what happened.
    //
    // Delisted titles (Project CARS 2, DiRT 4, F1 22 and 23, GRID 2019) do
    // not appear in store search any more but still resolve through
    // appdetails, and people still own and play them.
    //
    // A title may appear more than once: the F1 row covers four seasons, and
    // the lookup is by id, so each season maps to the same recipe.
    ("Assetto Corsa Competizione", 805550),
    ("Assetto Corsa EVO (early access)", 3058630),
    ("Assetto Corsa (original)", 244210),
    ("Assetto Corsa Rally (early access)", 3917090),
    ("Automobilista 2", 1066890),
    ("Project CARS 2", 378860),
    ("rFactor 2", 365960),
    ("Le Mans Ultimate", 2399420),
    ("iRacing", 266410),
    ("RaceRoom Racing Experience", 211500),
    ("BeamNG.drive", 284160),
    ("DiRT Rally 2.0", 690790),
    ("DiRT 4", 421020),
    ("EA Sports WRC", 1849250),
    ("EA Sports F1 (F1 22-25)", 1692250),
    ("EA Sports F1 (F1 22-25)", 2108330),
    ("EA Sports F1 (F1 22-25)", 2488620),
    ("EA Sports F1 (F1 22-25)", 3059520),
    ("Wreckfest", 228380),
    ("Euro Truck Simulator 2", 227300),
    ("American Truck Simulator", 270880),
    ("KartKraft", 406350),
    ("CarX Drift Racing Online", 635260),
    ("GRID (2019)", 703860),
    ("GRID Legends", 1307710),
    ("Forza Motorsport (2023)", 2440510),
    ("Forza Horizon 5", 1551360),
    ("Dakar Desert Rally", 1839940),
    ("Rennsport", 2077750),
    // Deliberately absent, because they are not on Steam at all: Richard
    // Burns Rally, Gran Turismo 7, TOCA Race Driver 3, Need for Speed Shift.
];

/// The appid for a registry entry, when one is recorded.
pub fn appid_for(name: &str) -> Option<u32> {
    STEAM_APPIDS.iter().find(|(n, _)| *n == name).map(|(_, id)| *id)
}

/// Steam appids of the installed-title groups the shell tooling checks
/// launch options for, as (sdk_sims, directinput_sims).
///
/// Derived from the registry rather than listed, so a title changing its
/// `Ffb` moves between the groups on its own.
pub fn launch_option_appid_groups() -> (Vec<u32>, Vec<u32>) {
    let mut sdk = Vec::new();
    let mut dinput = Vec::new();
    for g in GAMES.iter().filter(|g| g.linux != Linux::Unsupported) {
        let Some(id) = appid_for(g.name) else { continue };
        match g.ffb {
            Ffb::TrueForceShim => sdk.push(id),
            Ffb::DirectInput => dinput.push(id),
            Ffb::NativeEvdev => {}
        }
    }
    sdk.sort_unstable();
    dinput.sort_unstable();
    (sdk, dinput)
}

/// The Steam launch options a DirectInput title needs: the logi-ffb
/// helper wraps the game and gives it force feedback at all.
pub const LAUNCH_LOGI_FFB: &str = "logi-ffb %command%";

/// The Steam launch options an SDK-TrueForce title needs on a wheel
/// that answers the SDK, so Proton exposes the raw HID device the
/// staged SDK DLLs drive.
pub const LAUNCH_HIDRAW: &str = "PROTON_ENABLE_HIDRAW=1 %command%";

/// The wheel-side half of a setup recipe.
///
/// A recipe is not a property of the game alone, and treating it as one was
/// a real bug: every front-end resolved Assetto Corsa Competizione to
/// "install the shim, set `PROTON_ENABLE_HIDRAW=1`" no matter what was
/// plugged in. On a G923 that advice is not merely useless, it costs the
/// owner force feedback, because that wheel does not answer the TrueForce
/// SDK and the variable diverts the game to raw HID reports it cannot drive
/// feedback through.
///
/// So the smallest honest unit of support is the (game, wheel) pair. This
/// carries the wheel half of it, as capabilities rather than a model name,
/// so adding a wheel means teaching [`WheelCaps::of`] about it and nothing
/// in the registry has to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WheelCaps {
    /// Whether the wheel answers Logitech's TrueForce SDK, so a game's own
    /// TrueForce can reach it through the shim. True on the direct-drive
    /// family (RS50, G PRO), false on the G923, whose force feedback is the
    /// older classic protocol and whose SDK path has never worked.
    pub sdk_trueforce: bool,
}

impl WheelCaps {
    /// The capabilities of `model`.
    pub fn of(model: WheelModel) -> Self {
        WheelCaps {
            sdk_trueforce: match model {
                WheelModel::Rs50 | WheelModel::GPro => true,
                WheelModel::G923 => false,
                // A wheel is attached and we could not name it from either
                // its product id or its input name. The two ways of being
                // wrong here do not cost the same: recommending the SDK
                // path to a wheel that cannot take it loses that owner
                // force feedback, while withholding it from one that could
                // loses only an enhancement they can still turn on by
                // hand. So an unidentified wheel gets the answer that
                // cannot make things worse.
                WheelModel::Unknown => false,
            },
        }
    }

    /// What to assume with no wheel detected: the direct-drive family, the
    /// wheels this driver was written for. A front-end with nothing plugged
    /// in is describing the general case, not advising a specific owner.
    pub const fn assumed() -> Self {
        WheelCaps { sdk_trueforce: true }
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

    /// The single enablement action the Setup page offers for this title
    /// (see [`SetupAction`]). A native-TrueForce sim wants the shim; a
    /// title logi-tf-sim can drive today wants its simulated-TrueForce
    /// switch; a DirectInput title wants the logi-ffb helper; everything
    /// else already works with plain force feedback.
    pub fn setup_action(&self, caps: WheelCaps) -> SetupAction {
        if self.ffb == Ffb::TrueForceShim {
            // The shim is only worth installing on a wheel that answers the
            // SDK. Everywhere else this title still has ordinary force
            // feedback, so it degrades to "nothing to do" rather than to an
            // action that cannot help.
            if caps.sdk_trueforce {
                SetupAction::InstallShim
            } else {
                SetupAction::WorksOutOfBox
            }
        } else if self.ffb == Ffb::DirectInput {
            // Ahead of simulated TrueForce deliberately, because the two are
            // not alternatives: logi-ffb is what gives a DirectInput title
            // any force feedback at all, while simulated TrueForce adds
            // haptics on top of feedback that already works. Ordered the
            // other way, a title that gained a telemetry decoder silently
            // lost its launch options, and its owner was told to enable an
            // engine note for a wheel that was not being driven at all.
            // Simulated TrueForce for these titles is not lost: it has its
            // own section in the generated doc and its own switch in the
            // app's Setup page.
            SetupAction::UseLogiFfb
        } else if self.simulated_tf.live_id().is_some() {
            SetupAction::SimulatedTrueForce
        } else {
            SetupAction::WorksOutOfBox
        }
    }

    /// The one-line recommended setup for this title on `caps`' wheel.
    ///
    /// The stored [`setup`](GameCompat::setup) line describes the
    /// direct-drive case. A wheel with no SDK TrueForce needs its own line
    /// for the SDK titles, and the difference is not a detail: it is the
    /// one case where following the direct-drive advice makes things worse
    /// rather than merely not better.
    pub fn setup_line(&self, caps: WheelCaps) -> &'static str {
        // A title nobody can run on Linux gets its own line whatever the
        // wheel. Without this, an SDK title that happens to be unplayable
        // here still handed a G923 owner install steps for it, which reads
        // as though it were only the wheel standing in the way.
        if self.linux == Linux::Unsupported {
            return self.setup;
        }
        if self.ffb == Ffb::TrueForceShim && !caps.sdk_trueforce {
            // Worded as "not available on this wheel" rather than "this
            // wheel has no SDK TrueForce", because it also covers the
            // unidentified wheel, about which we know only that we cannot
            // deliver it.
            "Leave PROTON_ENABLE_HIDRAW unset: on this wheel it costs you \
             force feedback. For haptics, turn this game on under Simulated \
             TrueForce and run logi-tf-relay in its prefix (see \
             docs/SHARED_MEMORY_RELAY.md); that route is confirmed working \
             on a G923. Installing the shim WITH --proxy aims to carry the \
             game's own TrueForce instead, which would be better, but no \
             one has yet got it to load. Steam Input off."
        } else {
            self.setup
        }
    }

    /// The Steam launch options this title wants on `caps`' wheel, or
    /// `None` when it needs none.
    ///
    /// Returned rather than written: this is the string a front-end shows
    /// and offers to copy. Steam keeps launch options in `localconfig.vdf`
    /// and rewrites that file wholesale when it exits, so editing it under
    /// a running Steam loses the edit, and editing it badly loses whatever
    /// else the user had set there.
    pub fn launch_options(&self, caps: WheelCaps) -> Option<&'static str> {
        match self.setup_action(caps) {
            SetupAction::InstallShim => Some(LAUNCH_HIDRAW),
            SetupAction::UseLogiFfb => Some(LAUNCH_LOGI_FFB),
            SetupAction::SimulatedTrueForce | SetupAction::WorksOutOfBox => None,
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
pub(crate) fn normalize_title(title: &str) -> String {
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
/// A short, typeable name for a title: lowercase, punctuation folded to
/// dashes, any parenthetical dropped. `DiRT Rally 2.0` becomes
/// `dirt-rally-2-0`, `Assetto Corsa EVO (early access)` becomes
/// `assetto-corsa-evo`.
pub fn slug_for(name: &str) -> String {
    let base = name.split('(').next().unwrap_or(name);
    let mut out = String::new();
    let mut dash = false;
    for c in base.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// The registry row for a slug the user typed, matched exactly first and
/// then as a unique prefix.
///
/// Exists because an appid is not always available: a game bought outside
/// Steam, added as a non-Steam shortcut (whose appid is generated locally
/// and means nothing to us), or delisted and reinstalled from a backup.
/// `Err` carries the candidates when a prefix matched more than one, since
/// silently picking one of `dirt-4` and `dirt-rally-2-0` would apply the
/// wrong recipe.
pub fn compat_for_slug(want: &str) -> Result<&'static GameCompat, Vec<String>> {
    let want = want.trim().to_ascii_lowercase();
    if let Some(g) = GAMES.iter().find(|g| slug_for(g.name) == want) {
        return Ok(g);
    }
    let hits: Vec<&'static GameCompat> =
        GAMES.iter().filter(|g| slug_for(g.name).starts_with(&want)).collect();
    match hits.len() {
        1 => Ok(hits[0]),
        _ => Err(hits.iter().map(|g| slug_for(g.name)).collect()),
    }
}

/// The registry row for a Steam appid, for the launch wrapper.
///
/// `match_title` goes by name, which a wrapper does not have: Steam gives
/// it `SteamAppId` and nothing else. Looking the name up first and matching
/// on that would add a second place for the mapping to drift.
pub fn compat_for_appid(appid: u32) -> Option<&'static GameCompat> {
    let name = STEAM_APPIDS.iter().find(|(_, id)| *id == appid).map(|(n, _)| *n)?;
    GAMES.iter().find(|g| g.name == name)
}

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
        // Native TrueForce, and on a direct-drive wheel that is the
        // route to use: install the shim and this is redundant. It is here
        // for the G923, which cannot receive the SDK's TrueForce at all, so
        // a synthesized engine note is the difference between haptics and
        // silence. Competizione publishes the same shared memory as Assetto
        // Corsa, byte for byte, so this needed no new decoder.
        simulated_tf: SimTf::LiveNow("acc"),
        setup: "Install the TrueForce shim; set PROTON_ENABLE_HIDRAW=1; turn Steam Input off.",
        confidence: Confidence::Verified,
    },
    GameCompat {
        name: "Assetto Corsa EVO (early access)",
        linux: Linux::Proton,
        ffb: Ffb::TrueForceShim,
        native_trueforce: Support::Yes,
        // Native TrueForce, and on a direct-drive wheel that is the
        // route to use. This is here for the G923, which cannot receive the
        // SDK's TrueForce at all. EVO renamed its sections and moved the
        // redline into the physics block, so unlike Competizione it needed
        // its own decoder rather than Assetto Corsa's unchanged.
        simulated_tf: SimTf::LiveNow("ac-evo"),
        setup: "Install the TrueForce shim; set PROTON_ENABLE_HIDRAW=1; turn Steam Input off.",
        confidence: Confidence::Verified,
    },
    GameCompat {
        name: "Assetto Corsa (original)",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::LiveNow("assetto"),
        setup: "Plain force feedback; no shim; turn Steam Input off. \
Simulated TrueForce needs logi-tf-relay in the prefix (see \
docs/SHARED_MEMORY_RELAY.md); nothing to switch on in the game.",
        // Documented: the decoder reads only the head of Kunos' physics
        // block, which has not moved since AC 1.0, and checks the static
        // block's layout in-band before trusting the redline. Those exact
        // offsets were confirmed against a live Competizione session on
        // 2026-08-06, which publishes the same two sections with the same
        // layout: the UTF-16 guard passed on real bytes and maxRpm at 412
        // read a genuine redline. This title itself has not been run, which
        // is the only reason this is not Verified.
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
        simulated_tf: SimTf::LiveNow("rf2"),
        setup: "Set PROTON_ENABLE_HIDRAW=0, or launch with logi-ffb %command%; \
Steam Input off. Simulated TrueForce needs the community \
rF2SharedMemoryMapPlugin plus logi-tf-relay in the prefix (see \
docs/SHARED_MEMORY_RELAY.md).",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "Le Mans Ultimate",
        linux: Linux::Proton,
        ffb: Ffb::DirectInput,
        native_trueforce: Support::No,
        simulated_tf: SimTf::LiveNow("lmu"),
        setup: "Set PROTON_ENABLE_HIDRAW=0, or launch with logi-ffb %command%; \
Steam Input off. Simulated TrueForce needs the community \
rF2SharedMemoryMapPlugin plus logi-tf-relay in the prefix (see \
docs/SHARED_MEMORY_RELAY.md).",
        confidence: Confidence::Verified,
    },
    GameCompat {
        name: "iRacing",
        linux: Linux::Proton,
        ffb: Ffb::DirectInput,
        // Real on Windows: first-party G Hub/iRacing captures show its
        // native TrueForce on 0x8123 (docs/PROTOCOL_SPECIFICATION.md,
        // issue #20 lineage). Expected, not Yes: nobody has shown the SDK
        // loading under Proton, and its working FFB here is DirectInput
        // through logi-ffb, which hidraw would kill. The ffb field
        // therefore stays DirectInput on purpose.
        native_trueforce: Support::Expected,
        simulated_tf: SimTf::LiveNow("iracing"),
        setup: "Now Linux-playable; set PROTON_ENABLE_HIDRAW=0 or launch with \
logi-ffb %command%; Steam Input off. Simulated TrueForce needs logi-tf-relay \
in the prefix (see docs/SHARED_MEMORY_RELAY.md).",
        // Expected: the decoder reads iRacing's own self-describing variable
        // table rather than guessed offsets, and is tested against the
        // documented layout, but nobody has yet confirmed the game even
        // publishes its shared memory under Proton. Listed as live so the
        // per-game switch exists for whoever tries it first.
        confidence: Confidence::Expected,
    },
    GameCompat {
        name: "RaceRoom Racing Experience",
        linux: Linux::Proton,
        ffb: Ffb::DirectInput,
        native_trueforce: Support::No,
        simulated_tf: SimTf::LiveNow("raceroom"),
        setup: "Set PROTON_ENABLE_HIDRAW=0, or launch with logi-ffb %command%; \
Steam Input off. Simulated TrueForce needs logi-tf-relay in the prefix (see \
docs/SHARED_MEMORY_RELAY.md); nothing to switch on in the game.",
        // Expected: the decoder is written against KW Studios' own published
        // `r3e.h`, whose major version it checks in-band before reading, but
        // nobody has yet confirmed the game publishes `$R3E` under Proton.
        confidence: Confidence::Expected,
    },
    GameCompat {
        name: "BeamNG.drive",
        // Ships a native Linux build; a native build never goes through
        // Proton, so the SDK/hidraw questions do not arise on the primary
        // route. Its Windows TrueForce is real (the TRUEFORCE_PROTOCOL
        // captures are BeamNG captures) but unconfirmed on this driver,
        // hence Expected below.
        linux: Linux::Native,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::Expected,
        simulated_tf: SimTf::LiveNow("beamng"),
        setup: "Plain force feedback on the native Linux build (Proton also \
works); for simulated TrueForce, enable OutGauge to 127.0.0.1:4444 and run \
logi-tf-sim.",
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
        simulated_tf: SimTf::LiveNow("ets2"),
        setup: "Plain force feedback on the native Linux build; no shim. For \
simulated TrueForce, install the logi-tf-scs plugin (see docs/SCS_PLUGIN.md).",
        // Expected, not Verified: the plugin is built against the official
        // SDK headers with its struct layouts pinned by tests, but nobody
        // has yet loaded it into a running game and felt the result.
        confidence: Confidence::Expected,
    },
    GameCompat {
        name: "American Truck Simulator",
        linux: Linux::Native,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::LiveNow("ats"),
        setup: "Plain force feedback on the native Linux build; no shim. For \
simulated TrueForce, install the logi-tf-scs plugin (see docs/SCS_PLUGIN.md).",
        // See the Euro Truck Simulator 2 entry: same plugin, same caveat.
        confidence: Confidence::Expected,
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
        simulated_tf: SimTf::LiveNow("codemasters"),
        setup: "Enable UDP telemetry (Codemasters format); run logi-tf-sim. \
Steam Input off. Whether its TrueForce reaches the wheel through Logitech's \
SDK is unconfirmed, so nothing here installs one; reports welcome.",
        confidence: Confidence::Documented,
    },
    GameCompat {
        name: "GRID Legends",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::LiveNow("codemasters"),
        setup: "Enable UDP telemetry (Codemasters format); run logi-tf-sim; \
turn Steam Input off.",
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
        // Community-sourced, not first-party tested: ProtonDB rates FH5
        // Platinum and it has been Steam Deck Verified since 2022, online
        // play included, which contradicts the old "not reliably on Linux
        // (anti-cheat)" row. Plan-safe either way: the recipe is a
        // plain-FFB no-op (no hidraw, no shim, no relay).
        name: "Forza Horizon 5",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::No,
        setup: "Plain force feedback.",
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
        GameCompat {
        name: "TOCA Race Driver 3",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::No,
        setup: "Plain force feedback.",
        confidence: Confidence::Documented,
    },
        GameCompat {
        name: "Need for Speed: Shift",
        linux: Linux::Proton,
        ffb: Ffb::NativeEvdev,
        native_trueforce: Support::No,
        simulated_tf: SimTf::No,
        setup: "Plain force feedback.",
        confidence: Confidence::Documented,
    },
];

/// Every title, sorted case-insensitively by name: the friendly order for a
/// lookup table (a user hunting one game reads alphabetically).
pub fn sorted_by_name() -> Vec<&'static GameCompat> {
    let mut games: Vec<&'static GameCompat> = GAMES.iter().collect();
    games.sort_by_key(|g| g.name.to_lowercase());
    games
}

/// What `logi-launch` will actually do for one (game, wheel) pair.
///
/// The recipe used to exist twice: once as the `--launch-plan` printer that
/// `logi-launch` parses, and once as the static `setup` sentence the Setup
/// page shows. They could not agree, because only the first knew which
/// wheel was attached, so the page told a G923 owner to set
/// `PROTON_ENABLE_HIDRAW=1` for Assetto Corsa Competizione while the
/// wrapper correctly refused to. This is the single answer both now read.
///
/// It is deliberately data rather than printed lines: the terminal app
/// renders it as `key=value` for the wrapper to parse, and the GUI renders
/// it as a sentence for a person to read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchPlan {
    /// `Some(true)` sets `PROTON_ENABLE_HIDRAW=1`, `Some(false)` sets it to
    /// `0` explicitly, `None` leaves it alone. Never guessed: on a wheel
    /// that cannot take it, setting it costs the owner force feedback.
    pub hidraw: Option<bool>,
    /// Launch the game through the `logi-ffb` proxy for DirectInput force
    /// feedback.
    pub ffb_proxy: bool,
    /// Run the `logi-tf-sim` daemon for this title.
    pub tfsim: bool,
    /// Which relay decoder belongs in the game's prefix, if any.
    pub relay: Option<&'static str>,
    /// False for a title that does not run on Linux, which gets no recipe
    /// at all rather than an untested one.
    pub supported: bool,
    /// Why the plan is what it is, in the order it should be shown.
    pub notes: Vec<String>,
    /// Enable the driver's kernel texture merge for this session: the
    /// engine-note texture is rendered on the wheel itself, mixed into the
    /// game's own TrueForce stream. `logi-launch` responds by staging the
    /// dinput8 escape proxy into the game's directory (it relays the RPM
    /// telemetry the merge is driven by), starting `logi-rpm-bridge`, and
    /// writing 1 to `wheel_tf_merge`, undoing all of it when the game
    /// exits. Only granted where the native TrueForce path itself is: on a
    /// wheel that cannot receive the SDK stream there is nothing to merge
    /// into.
    pub texture_merge: bool,
    /// The `PROTON_ENABLE_HIDRAW` value to use when [`Self::hidraw`] is on:
    /// `0xVID/0xPID` naming the attached wheel.
    ///
    /// Proton matches that variable as a substring against each device's own
    /// `0x%04X/0x%04X` (`dlls/winebus.sys/main.c`). The bare value `1`
    /// short-circuits the test and hands EVERY HID device to the game, so a
    /// keyboard, a headset and any other controller all become raw HID
    /// alongside the wheel. Naming the wheel is what the pattern form is
    /// for, and it is what issue #60 ran into.
    ///
    /// `None` when no wheel is attached to name, in which case `1` is used
    /// and the old blunt behaviour applies.
    pub hidraw_scope: Option<String>,
}

impl LaunchPlan {
    /// The plan for a title the registry does not know.
    ///
    /// Still runs the daemon. Only the shared-memory sims are keyed by
    /// appid; the UDP ones need nothing but the daemon listening, and it
    /// idles when nothing is streaming. Withholding it because a game is
    /// absent from a table would leave exactly those titles unserved for
    /// no gain.
    pub fn unknown() -> Self {
        LaunchPlan { tfsim: true, supported: true, ..Default::default() }
    }

    /// The plan for `game` on a wheel with `caps`.
    ///
    /// `ambiguous` is true when several kinds of wheel are attached and
    /// none was named. The game picks which one it uses, in its own
    /// settings, and never tells us. In that state the harmful half of the
    /// recipe is withheld rather than guessed, because guessing wrong sets
    /// `PROTON_ENABLE_HIDRAW` on a G923 and costs it force feedback.
    pub fn for_game(game: &GameCompat, caps: WheelCaps, ambiguous: bool) -> Self {
        let mut plan = LaunchPlan { supported: true, ..Default::default() };

        if game.linux == Linux::Unsupported {
            plan.supported = false;
            plan.notes.push("this title does not run on Linux".into());
            return plan;
        }

        match game.launch_options(caps) {
            Some(LAUNCH_HIDRAW) if ambiguous => plan
                .notes
                .push("this game wants PROTON_ENABLE_HIDRAW on a direct-drive wheel;".into()),
            Some(LAUNCH_HIDRAW) => plan.hidraw = Some(true),
            Some(LAUNCH_LOGI_FFB) => plan.ffb_proxy = true,
            _ => {}
        }
        // DirectInput without the proxy needs HIDRAW off, not merely unset.
        if game.ffb == Ffb::DirectInput && game.launch_options(caps).is_none() {
            plan.hidraw = Some(false);
        }

        // A title whose own TrueForce reaches this wheel must NOT also get
        // the simulated kind. The daemon treats an unlisted game as
        // enabled, so running it for ACC or AC EVO on a direct-drive wheel
        // would layer a synthesised engine note over the real haptics the
        // game is already sending.
        if game.setup_action(caps) == SetupAction::InstallShim {
            plan.notes.push("simulated TrueForce stays off, so it does not double the real thing".into());
            // The kernel texture merge rides the native stream, so it is
            // granted exactly as widely as hidraw: withheld on an ambiguous
            // rig for the same reason, and gated per title because the
            // escape proxy's RPM relay is only validated for AC EVO.
            if plan.hidraw == Some(true)
                && game.simulated_tf.live_id() == Some("ac-evo")
            {
                plan.texture_merge = true;
            }
            return plan;
        }

        if let SimTf::LiveNow(id) = game.simulated_tf {
            plan.tfsim = true;
            plan.relay = matches!(id, "acc" | "ac-evo" | "assetto" | "iracing" | "raceroom" | "rf2" | "lmu")
                .then_some(id);
        }
        plan
    }

    /// The plan as the `key=value` lines `logi-launch` parses.
    ///
    /// Order and spelling are load-bearing: the wrapper reads them with
    /// `sed -n "s/^key=//p"`, so a renamed key silently stops being found
    /// rather than failing loudly.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.supported {
            out.push("supported=0".into());
            out.push("tfsim=0".into());
            out.push("relay=none".into());
            out.extend(self.notes.iter().map(|n| format!("note={n}")));
            return out;
        }
        for note in &self.notes {
            // A note explaining a withheld hidraw belongs beside it, above
            // the telemetry half, which is where it was printed before.
            if note.contains("PROTON_ENABLE_HIDRAW") {
                out.push(format!("note={note}"));
            }
        }
        match self.hidraw {
            Some(true) => out.push(format!(
                "hidraw={}",
                self.hidraw_scope.as_deref().unwrap_or("1")
            )),
            Some(false) => out.push("hidraw=0".into()),
            None => {}
        }
        if self.texture_merge {
            out.push("texture=merge".into());
        }
        if self.ffb_proxy {
            out.push("ffb=proxy".into());
        }
        out.push(format!("tfsim={}", u8::from(self.tfsim)));
        out.push(format!("relay={}", self.relay.unwrap_or("none")));
        for note in &self.notes {
            if !note.contains("PROTON_ENABLE_HIDRAW") {
                out.push(format!("note={note}"));
            }
        }
        out
    }

    /// The plan as a sentence, for the Setup page.
    ///
    /// Written as what the wrapper WILL DO rather than what the user should
    /// do, because with `logi-launch` in the launch options they no longer
    /// do any of it. The page used to show a static instruction to set
    /// variables by hand next to a launch line that sets them for you,
    /// which reads as two conflicting recipes.
    pub fn describe(&self) -> String {
        if !self.supported {
            return "This title does not run on Linux, so there is nothing to set up.".into();
        }
        let mut parts: Vec<String> = Vec::new();
        match self.hidraw {
            Some(true) => parts.push(format!(
                "sets PROTON_ENABLE_HIDRAW={} so the game's own TrueForce reaches the wheel",
                self.hidraw_scope.as_deref().unwrap_or("1")
            )),
            Some(false) => parts.push("sets PROTON_ENABLE_HIDRAW=0, which this game needs for force feedback".into()),
            None => {}
        }
        if self.texture_merge {
            parts.push(
                "merges the engine-note texture into the game's own TrueForce on the wheel".into(),
            );
        }
        if self.ffb_proxy {
            parts.push("runs the game through logi-ffb for force feedback".into());
        }
        if self.tfsim {
            parts.push("starts logi-tf-sim for simulated TrueForce".into());
        }
        if let Some(relay) = self.relay {
            parts.push(format!("puts the {relay} telemetry relay in the game's prefix"));
        }

        let mut text = if parts.is_empty() {
            "On your wheel, logi-launch sets nothing extra for this game.".to_string()
        } else {
            format!("On your wheel, logi-launch {}.", join_clauses(&parts))
        };
        for note in &self.notes {
            text.push(' ');
            let note = note.trim_end_matches(';');
            text.push_str(&format!("{}{}", capitalise(note), if note.ends_with('.') { "" } else { "." }));
        }
        text
    }
}

/// "a", "a and b", "a, b and c" - an Oxford-comma-free list, because these
/// clauses are read aloud in a sentence rather than scanned as a table.
fn join_clauses(parts: &[String]) -> String {
    match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        _ => format!("{} and {}", parts[..parts.len() - 1].join(", "), parts[parts.len() - 1]),
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tfsim;

    const DD: WheelCaps = WheelCaps { sdk_trueforce: true };
    const G923: WheelCaps = WheelCaps { sdk_trueforce: false };

    fn acc() -> &'static GameCompat {
        compat_for_appid(805550).expect("Assetto Corsa Competizione is in the registry")
    }

    /// The two wheels must get DIFFERENT recipes for the same game, which
    /// is the whole reason the Setup page shows this line: the launch
    /// options are now identical for every game and every wheel, so the
    /// page would otherwise imply the same thing happens on both.
    #[test]
    fn the_same_game_plans_differently_per_wheel() {
        let dd = LaunchPlan::for_game(acc(), DD, false);
        let classic = LaunchPlan::for_game(acc(), G923, false);
        assert_ne!(dd, classic, "ACC must not plan identically on both wheels");

        // Direct drive: the game's own TrueForce, and NOT the simulated
        // kind on top of it.
        assert_eq!(dd.hidraw, Some(true));
        assert!(!dd.tfsim, "simulated TrueForce would double the real thing");

        // A G923 must never be told to set this: it costs that wheel force
        // feedback, and the owner has no way to tell that is what happened.
        assert_ne!(classic.hidraw, Some(true), "PROTON_ENABLE_HIDRAW on a G923 kills its force feedback");
        assert!(classic.tfsim, "the G923's only TrueForce here is the simulated kind");
    }

    /// With several kinds of wheel attached and none named, the harmful
    /// half is withheld rather than guessed.
    #[test]
    fn an_ambiguous_rig_is_never_told_to_set_hidraw() {
        let plan = LaunchPlan::for_game(acc(), DD, true);
        assert_eq!(plan.hidraw, None, "guessing here costs a G923 its force feedback");
        assert!(
            plan.notes.iter().any(|n| n.contains("PROTON_ENABLE_HIDRAW")),
            "withholding it silently is worse than withholding it out loud"
        );
    }

    /// `logi-launch` parses these with `sed -n "s/^key=//p"`, so a renamed
    /// or reordered key stops being found instead of failing loudly.
    #[test]
    fn the_wrapper_keys_are_present_and_parseable() {
        for (game, caps) in [(acc(), DD), (acc(), G923)] {
            let lines = LaunchPlan::for_game(game, caps, false).lines();
            for key in ["tfsim=", "relay="] {
                assert!(
                    lines.iter().any(|l| l.starts_with(key)),
                    "{key:?} missing from {lines:?}; logi-launch reads it by exact prefix"
                );
            }
            for line in &lines {
                assert!(line.contains('='), "{line:?} is not a key=value line");
                assert!(!line.contains('\n'), "{line:?} spans lines; the wrapper reads one per line");
            }
        }
    }

    /// **Native TrueForce always wins where it can arrive.** A game with its
    /// own TrueForce, on a wheel that can receive it, must never also be
    /// given the simulated kind: that would lay a synthesised engine note
    /// over the haptics the game's developers authored, and the two would
    /// play at once.
    ///
    /// The converse is equally required. On a wheel that cannot receive it,
    /// the same title must get the simulated kind, or that owner is left
    /// with nothing rather than with the best available.
    #[test]
    fn native_trueforce_is_always_preferred_where_it_can_arrive() {
        for game in GAMES.iter() {
            if game.linux == Linux::Unsupported {
                continue;
            }
            for caps in [DD, G923] {
                let plan = LaunchPlan::for_game(game, caps, false);
                let native_here = game.setup_action(caps) == SetupAction::InstallShim;
                if native_here {
                    assert!(
                        !plan.tfsim,
                        "{}: native TrueForce reaches this wheel, so the simulated kind \
                         must stay off or both play at once",
                        game.name
                    );
                    assert_eq!(plan.relay, None, "{}: no relay is needed when native works", game.name);
                } else if game.simulated_tf.live_id().is_some() {
                    assert!(
                        plan.tfsim,
                        "{}: native TrueForce cannot reach this wheel and a decoder exists, \
                         so the simulated kind is what that owner should get",
                        game.name
                    );
                }
            }
        }
    }

    /// The kernel texture merge goes exactly where the native TrueForce
    /// path goes, and only for the title whose RPM relay is validated.
    #[test]
    fn only_ac_evo_on_a_direct_drive_wheel_gets_the_texture_merge() {
        let evo = compat_for_appid(3058630).expect("Assetto Corsa EVO is in the registry");

        let dd = LaunchPlan::for_game(evo, DD, false);
        assert!(dd.texture_merge, "AC EVO on a direct-drive wheel is the shipping merge case");
        assert!(
            dd.lines().contains(&"texture=merge".to_string()),
            "logi-launch reads this by exact prefix; {:?}",
            dd.lines()
        );

        // No native stream on a G923, so nothing to merge into.
        let classic = LaunchPlan::for_game(evo, G923, false);
        assert!(!classic.texture_merge, "a G923 has no native stream to merge into");

        // An ambiguous rig is withheld hidraw, and the merge follows it.
        let ambiguous = LaunchPlan::for_game(evo, DD, true);
        assert!(!ambiguous.texture_merge, "the merge must follow a withheld hidraw");

        // ACC's RPM relay through the escape proxy is unvalidated, so it
        // must not inherit AC EVO's recipe by accident.
        let acc_dd = LaunchPlan::for_game(acc(), DD, false);
        assert!(!acc_dd.texture_merge, "only AC EVO's RPM relay is validated");
    }

    /// An unknown title still gets the daemon: the UDP sims need nothing
    /// but a listener, and it idles when nothing streams.
    #[test]
    fn an_unknown_game_still_gets_the_daemon() {
        let plan = LaunchPlan::unknown();
        assert!(plan.tfsim);
        assert_eq!(plan.relay, None);
        assert!(plan.lines().contains(&"tfsim=1".to_string()));
    }

    /// A title that does not run on Linux gets no recipe rather than an
    /// untested one.
    #[test]
    fn an_unsupported_title_is_described_as_such_and_gets_no_recipe() {
        let Some(game) = GAMES.iter().find(|g| g.linux == Linux::Unsupported) else {
            return;
        };
        let plan = LaunchPlan::for_game(game, DD, false);
        assert!(!plan.supported);
        assert!(!plan.tfsim);
        assert!(plan.lines().contains(&"supported=0".to_string()));
        assert!(plan.describe().contains("does not run on Linux"));
    }

    /// The sentence has to read as one, and must never be empty: a blank
    /// line under the launch options tells the reader nothing about why it
    /// is blank.
    #[test]
    fn every_game_describes_itself_readably_on_both_wheels() {
        for game in GAMES.iter() {
            for caps in [DD, G923] {
                let text = LaunchPlan::for_game(game, caps, false).describe();
                assert!(!text.trim().is_empty(), "{} described as nothing", game.name);
                assert!(text.ends_with('.'), "{} : {text:?} does not end a sentence", game.name);
                assert!(!text.contains(" ,"), "{} : {text:?} has a stray comma", game.name);
                assert!(!text.contains("  "), "{} : {text:?} has a doubled space", game.name);
                assert!(!text.contains(".."), "{} : {text:?} has a doubled full stop", game.name);
            }
        }
    }
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
            // Native UDP telemetry: the daemon parses the game's own
            // broadcast directly.
            ("Automobilista 2", "ams2-pcars2"),
            ("Project CARS 2", "ams2-pcars2"),
            ("DiRT Rally 2.0", "dirt-rally-2"),
            ("DiRT 4", "codemasters"),
            // Same EGO engine and same packed float-array output as DiRT 4,
            // so the existing Codemasters parser reads them unchanged: its
            // length gate accepts the whole 64..=70 float range and both
            // titles land inside it. These two sat marked "needs a parser"
            // for a while purely because nobody carried the fact across
            // from the parser, which is what this allowlist is here to stop.
            ("GRID (2019)", "codemasters"),
            ("GRID Legends", "codemasters"),
            ("BeamNG.drive", "beamng"),
            ("EA Sports F1 (F1 22-25)", "f1"),
            ("EA Sports WRC", "ea-wrc"),
            // Plugin sources: no UDP of their own, so a plugin inside the
            // game speaks the relay format instead. Live in the same sense,
            // and gated by the same per-game switches.
            ("Euro Truck Simulator 2", "ets2"),
            ("American Truck Simulator", "ats"),
            // Shared memory, read by the in-prefix relay. Each of these three
            // earned a decoder a different way: iRacing's telemetry is
            // self-describing, RaceRoom's layout is published by KW Studios
            // themselves and version-stamped in-band, and Assetto Corsa's
            // needed fields sit in the part of its struct that has not moved
            // since 1.0, with an in-band check on the one risky offset.
            // rFactor 2 and Le Mans Ultimate have none of those properties
            // and still wait for a captured fixture.
            ("iRacing", "iracing"),
            ("RaceRoom Racing Experience", "raceroom"),
            ("Assetto Corsa (original)", "assetto"),
            // Same sections and same layout as Assetto Corsa, so the same
            // decoder; listed separately because it is a separate game to
            // anyone setting an intensity.
            ("Assetto Corsa Competizione", "acc"),
            // Same family, but not the same bytes: EVO renamed every section
            // and dropped the static block's redline, so it reads
            // `currentMaxRpm` out of the physics block instead.
            ("Assetto Corsa EVO (early access)", "ac-evo"),
            // The rF2 family's layout comes from a community plugin rather
            // than a vendor, which is why it was written last. What makes it
            // decodable is that the format says whether a read was clean:
            // version counters around each buffer catch a torn read, and the
            // player's car is found by matching an id across two
            // independently written buffers, which a wrong layout fails.
            ("rFactor 2", "rf2"),
            ("Le Mans Ultimate", "lmu"),
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

    /// Every appid must name a title the registry actually has. A typo
    /// here does not fail to resolve, it resolves to nothing and the
    /// launch wrapper silently treats a supported game as unknown.
    #[test]
    fn every_appid_names_a_game_in_the_registry() {
        for (name, appid) in STEAM_APPIDS {
            assert!(
                GAMES.iter().any(|g| g.name == *name),
                "appid {appid} is mapped to {name:?}, which is not a title in GAMES"
            );
            assert!(
                compat_for_appid(*appid).is_some(),
                "appid {appid} ({name}) does not resolve through compat_for_appid"
            );
        }
    }

    /// No appid may appear twice with different titles. The same title
    /// under several ids is fine and deliberate (the F1 seasons); the same
    /// id under two titles means one of them is wrong.
    #[test]
    fn no_appid_maps_to_two_different_games() {
        for (i, (name_a, id_a)) in STEAM_APPIDS.iter().enumerate() {
            for (name_b, id_b) in &STEAM_APPIDS[i + 1..] {
                assert!(
                    id_a != id_b || name_a == name_b,
                    "appid {id_a} is mapped to both {name_a:?} and {name_b:?}"
                );
            }
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
        let dd = WheelCaps { sdk_trueforce: true };
        let action = |name: &str| GAMES.iter().find(|g| g.name == name).unwrap().setup_action(dd);
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

    /// The bug this whole wheel dimension exists for. Before it, every
    /// front-end told a G923 owner with ACC installed to install the shim
    /// and set PROTON_ENABLE_HIDRAW=1. That wheel does not answer the
    /// TrueForce SDK, and the variable diverts the game to raw HID reports
    /// it cannot drive force feedback through, so the advice cost them the
    /// force feedback they already had.
    #[test]
    fn a_wheel_without_sdk_trueforce_is_never_told_to_set_hidraw() {
        let g923 = WheelCaps::of(WheelModel::G923);
        assert!(!g923.sdk_trueforce);
        for g in GAMES.iter().filter(|g| g.ffb == Ffb::TrueForceShim) {
            assert_eq!(
                g.setup_action(g923),
                SetupAction::WorksOutOfBox,
                "{}: the shim cannot help this wheel",
                g.name
            );
            assert_eq!(g.launch_options(g923), None, "{}: no launch options", g.name);
            assert!(
                !g.setup_line(g923).contains("PROTON_ENABLE_HIDRAW=1"),
                "{}: must not ask for the variable that costs this wheel its force feedback",
                g.name
            );
        }
    }

    /// The same titles on a wheel that does answer the SDK: unchanged, so
    /// the fix above cannot be mistaken for switching TrueForce off for
    /// everyone.
    #[test]
    fn a_wheel_with_sdk_trueforce_still_gets_the_shim_recipe() {
        for model in [WheelModel::Rs50, WheelModel::GPro] {
            let caps = WheelCaps::of(model);
            assert!(caps.sdk_trueforce, "{model:?}");
            let acc = match_title("Assetto Corsa Competizione").unwrap();
            assert_eq!(acc.setup_action(caps), SetupAction::InstallShim);
            assert_eq!(acc.launch_options(caps), Some(LAUNCH_HIDRAW));
            assert_eq!(acc.setup_line(caps), acc.setup);
        }
    }

    /// An unidentified wheel takes the answer that cannot make things
    /// worse, while no wheel at all describes the general case.
    #[test]
    fn unknown_wheel_is_cautious_and_no_wheel_is_general() {
        assert!(!WheelCaps::of(WheelModel::Unknown).sdk_trueforce);
        assert!(WheelCaps::assumed().sdk_trueforce);
    }

    /// DirectInput titles are wheel-independent: logi-ffb presents its own
    /// virtual wheel, so the recipe does not change with the hardware.
    #[test]
    fn directinput_recipe_does_not_vary_by_wheel() {
        let lmu = match_title("Le Mans Ultimate").unwrap();
        for caps in [WheelCaps { sdk_trueforce: true }, WheelCaps { sdk_trueforce: false }] {
            assert_eq!(lmu.setup_action(caps), SetupAction::UseLogiFfb);
            assert_eq!(lmu.launch_options(caps), Some(LAUNCH_LOGI_FFB));
        }
    }

    /// Every title that offers launch options offers a string a user can
    /// paste into Steam unaltered: the `%command%` placeholder is what
    /// makes it a wrapper rather than a replacement for the game.
    #[test]
    fn every_offered_launch_option_is_pasteable() {
        for g in GAMES {
            for caps in [WheelCaps { sdk_trueforce: true }, WheelCaps { sdk_trueforce: false }] {
                if let Some(opts) = g.launch_options(caps) {
                    assert!(opts.contains("%command%"), "{}: {opts}", g.name);
                }
            }
        }
    }

    /// Every recipe must agree with how the app will actually behave.
    ///
    /// The setup line is prose and the action is code, and they have drifted
    /// apart more than once: a title told the reader to try the TrueForce
    /// shim while its classification meant the app would never offer one,
    /// and a G923 recipe told the same reader to skip the shim and to
    /// install it in one sentence. Prose that contradicts the button is
    /// worse than no prose, so the rules are checked here.
    #[test]
    fn every_recipe_agrees_with_what_the_app_will_do() {
        let dd = WheelCaps { sdk_trueforce: true };
        let classic = WheelCaps { sdk_trueforce: false };
        let mut problems = Vec::new();

        for g in GAMES {
            for (label, caps) in [("RS50/G PRO", dd), ("G923", classic)] {
                let line = g.setup_line(caps);
                let low = line.to_lowercase();
                let action = g.setup_action(caps);

                // Suggesting the shim only means something when the app
                // offers it, or when the text names the proxy route that
                // installs it for a wheel the SDK cannot drive.
                let suggests_shim = low.contains("shim")
                    && !low.contains("no shim")
                    && !low.contains("skip the shim");
                if suggests_shim
                    && action != SetupAction::InstallShim
                    && !line.contains("--proxy")
                {
                    problems.push(format!("{} [{label}]: suggests the shim, but the app offers {action:?}", g.name));
                }

                // Telling anyone to set the variable that removes their
                // force feedback is the bug this whole axis exists for.
                if line.contains("PROTON_ENABLE_HIDRAW=1") && action != SetupAction::InstallShim {
                    problems.push(format!("{} [{label}]: says to set PROTON_ENABLE_HIDRAW=1", g.name));
                }

                // A title nobody can run on Linux should not carry Linux
                // setup steps; it reads as though it were playable.
                if g.linux == Linux::Unsupported
                    && (suggests_shim || line.contains("PROTON_ENABLE_HIDRAW"))
                {
                    problems.push(format!("{}: not on Linux but gives Linux setup steps", g.name));
                }

                // A recipe must not say both.
                if low.contains("skip the shim") && low.contains("install the shim") {
                    problems.push(format!("{} [{label}]: says to skip and to install the shim", g.name));
                }
            }
        }
        assert!(problems.is_empty(), "recipes disagree with the app:\n  {}", problems.join("\n  "));
    }

    #[test]
    fn provisional_flag_tracks_soft_confidence() {
        assert!(Confidence::Expected.is_provisional());
        assert!(Confidence::Unknown.is_provisional());
        assert!(!Confidence::Verified.is_provisional());
        assert!(!Confidence::Documented.is_provisional());
    }
}
