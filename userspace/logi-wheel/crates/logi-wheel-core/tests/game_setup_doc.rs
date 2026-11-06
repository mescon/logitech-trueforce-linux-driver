// SPDX-License-Identifier: GPL-2.0-only
//! Renders `docs/GAME_SETUP.md` from the compatibility registry and checks
//! the committed file still matches.
//!
//! The doc is generated rather than written because the project has already
//! shipped documentation that contradicted the code: the README told people
//! to set `PROTON_ENABLE_HIDRAW=1` for DirectInput sims when the truth was
//! the reverse. A hand-maintained per-game matrix would go stale the first
//! time a row changed and nobody would find out. This way the test fails.
//!
//! To regenerate after changing the registry:
//!
//! ```text
//! UPDATE_GAME_SETUP=1 cargo test -p logi-wheel-core --test game_setup_doc
//! ```

use logi_wheel_core::games::{
    self, Confidence, Ffb, GameCompat, Linux, SetupAction, SimTf, WheelCaps,
};
use std::path::PathBuf;

/// The two wheel classes the recipes differ between. The doc's columns key
/// on `sdk_trueforce` alone, so the Xbox edition of the G923 shares the
/// direct-drive column: it answers Logitech's TrueForce SDK too, even
/// though it drives it over a different route than RS50 or G PRO.
const CLASSES: [(&str, WheelCaps); 2] = [
    ("RS50 / G PRO", WheelCaps::direct_drive()),
    ("G923", WheelCaps::classic()),
];

fn doc_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../docs/GAME_SETUP.md")
        .canonicalize()
        .expect("docs/GAME_SETUP.md must exist; create it empty and regenerate")
}

/// The short recipe tag for a table cell: what to do, and the launch
/// options to paste, if any.
fn recipe_cell(g: &GameCompat, caps: WheelCaps) -> String {
    if g.linux == Linux::Unsupported {
        return "-".to_string();
    }
    // An SDK title on a wheel that cannot use the SDK is the one cell where
    // "nothing to do" is true but insufficient: the reader arrived here
    // having seen the other column say to set a variable, and needs telling
    // that doing so on their wheel is not merely pointless.
    if g.ffb == Ffb::TrueForceShim && !caps.sdk_trueforce {
        // Simulated TrueForce is only on offer where the game publishes
        // telemetry to synthesise it from; a title without any has plain
        // force feedback on this wheel and nothing to add to it.
        return if g.simulated_tf.live_id().is_some() {
            "Turn on simulated TrueForce<br>and leave \
             `PROTON_ENABLE_HIDRAW` unset"
        } else {
            "Plain force feedback<br>leave `PROTON_ENABLE_HIDRAW` unset"
        }
        .to_string();
    }
    let action = match g.setup_action(caps) {
        SetupAction::InstallShim => "Install the shim",
        SetupAction::UseLogiFfb => "DirectInput force feedback",
        SetupAction::SimulatedTrueForce => "Turn on simulated TrueForce",
        SetupAction::WorksOutOfBox => "Nothing to do",
    };
    // The one launch command is always `logi-launch %command%`: it works out
    // raw HID, the logi-ffb proxy, the relay and the daemon per game, so the
    // table never shows the raw option (a bare PROTON_ENABLE_HIDRAW is unsafe
    // without the proxy the launcher stages, and a bare logi-ffb loses the
    // telemetry). A game that needs no launcher at all shows only the action.
    match g.launch_options(caps) {
        Some(_) => format!("{action}<br>`logi-launch %command%`"),
        None => action.to_string(),
    }
}

fn render() -> String {
    let mut out = String::new();
    out.push_str("# Game setup, per game and per wheel\n\n");
    out.push_str(
        "**Generated file. Do not edit.** The compatibility registry in\n\
         `userspace/logi-wheel/crates/logi-wheel-core/src/games.rs` is the\n\
         source. `tests/game_setup_doc.rs` renders this file from it. The test\n\
         fails if this file drifts from the registry. The settings app checks\n\
         your own installed games against that same registry. So what you\n\
         read here is what the app will tell you.\n\n",
    );
    out.push_str(
        "**Most people need none of this.** Put `logi-launch %command%` in a\n\
         game's Steam launch options. Launch options are the command Steam\n\
         runs to start a game. `logi-launch` works out the whole recipe for\n\
         the game and the attached wheel, and applies it. It sets the raw-HID\n\
         setting only where that wheel wants it. (Raw HID gives a game direct\n\
         access to the wheel's USB reports.) It starts the logi-ffb proxy for\n\
         the DirectInput games. (DirectInput is the older Windows\n\
         force-feedback interface. The proxy is a virtual wheel that forwards\n\
         force to the real one.) It starts the telemetry daemon. (Telemetry\n\
         is the live data a game publishes about the car: rpm, gear, speed.)\n\
         It puts the relay inside the game's prefix for the sims that need\n\
         one. (The prefix is the private Windows folder that Proton, Steam's\n\
         layer for running Windows games, makes for each game.) A game with\n\
         its own TrueForce keeps it. Nothing is layered on top. See\n\
         LAUNCH_OPTIONS.md.\n\n\
         **This table is for doing it by hand**, or for finding out what went\n\
         wrong. Read on to see why the answer differs per wheel.\n\n",
    );
    out.push_str(
        "What a game needs depends on the wheel as well as the game. The\n\
         direct-drive wheels answer Logitech's TrueForce SDK. (The SDK is\n\
         Logitech's library that a game loads to send TrueForce haptics.) A\n\
         sim with built-in TrueForce reaches them through the staged SDK\n\
         DLLs. `PROTON_ENABLE_HIDRAW` is what lets it, and `logi-launch` sets it\n\
         scoped to the wheel, never the bare `1`.\n\n\
         The G923 does not answer that SDK. Setting the variable there does\n\
         not add TrueForce. It diverts the game to raw HID reports. The wheel\n\
         cannot drive force feedback through those. So it costs you the force\n\
         feedback you already had. Leave it unset.\n\n\
         That wheel can still have haptics in those games, by a different\n\
         route. `logi-tf-sim` synthesizes an engine note from the game's own\n\
         telemetry. A small relay reads that telemetry out of the game's\n\
         shared memory (`docs/SHARED_MEMORY_RELAY.md`). This is confirmed\n\
         working on a G923 in Assetto Corsa Competizione and EVO.\n\n\
         The shim's `--proxy` option is not a second route in these games.\n\
         (The shim is the set of Logitech SDK files staged into a game's\n\
         prefix.) Both games check the library's signature before they load\n\
         it. They refuse the unsigned proxy, so it carries nothing. On a wheel\n\
         that uses Logitech's library, it also costs the game that library's\n\
         force feedback. `./tools/setup.sh doctor` flags a prefix that has\n\
         it.\n\n",
    );
    out.push_str(
        "To set launch options:\n\n\
         1. In Steam, open the game's Properties.\n\
         2. Paste the line into Launch Options, exactly, `%command%`\n\
         included.\n\n\
         `%command%` is the placeholder Steam replaces with the game itself.\n\
         Without it, the line replaces the game instead of wrapping it.\n\n",
    );

    out.push_str("## Recipes\n\n");
    out.push_str("| Game | Runs on Linux | Force feedback");
    for (name, _) in CLASSES {
        out.push_str(&format!(" | On {name}"));
    }
    out.push_str(" |\n|---|---|---");
    for _ in CLASSES {
        out.push_str("|---");
    }
    out.push_str("|\n");
    for g in games::sorted_by_name() {
        let provisional = if g.confidence.is_provisional() { " *" } else { "" };
        out.push_str(&format!(
            "| {}{} | {} | {}",
            g.name,
            provisional,
            g.linux.label(),
            g.ffb_cell()
        ));
        for (_, caps) in CLASSES {
            out.push_str(&format!(" | {}", recipe_cell(g, caps)));
        }
        out.push_str(" |\n");
    }
    out.push_str(
        "\nRows marked `*` are not confirmed on this driver yet. They are\n\
         expected or documented, not tested end to end.\n\n",
    );

    out.push_str("## Simulated TrueForce\n\n");
    out.push_str(
        "Games with no TrueForce of their own can still have engine haptics\n\
         and rev lights. `logi-tf-sim` synthesizes them from whatever\n\
         telemetry the game publishes. This works on every supported wheel,\n\
         including the G923. It is ordinary force feedback driven from\n\
         telemetry, not the SDK.\n\n\
         How the telemetry reaches the daemon depends on the game. Most games\n\
         broadcast it over UDP. (UDP is a simple network protocol. The game\n\
         sends packets to a port.) Those games only need that switched on in\n\
         their own settings. Euro Truck Simulator 2 and American Truck\n\
         Simulator use a plugin instead (`docs/SCS_PLUGIN.md`). iRacing\n\
         publishes to shared memory, and a small in-prefix relay forwards it\n\
         (`docs/SHARED_MEMORY_RELAY.md`). In every case, enable the game in\n\
         the app's Setup page afterwards.\n\n",
    );
    out.push_str("| Game | Simulated TrueForce |\n|---|---|\n");
    for g in games::sorted_by_name().iter().filter(|g| g.linux != Linux::Unsupported) {
        let cell = match g.simulated_tf {
            // A title with TrueForce of its own that ALSO has a telemetry
            // decoder must say both, or its row contradicts the sibling
            // title next to it: one reads "supported today" and the other
            // "not needed, the game has real TrueForce", and a reader with a
            // direct-drive wheel concludes the first one now requires
            // something. It does not. Simulated TrueForce is the fallback
            // for the wheels that cannot receive the real thing.
            SimTf::LiveNow(_) if g.ffb == Ffb::TrueForceShim => {
                "use the game's own TrueForce; on a direct-drive wheel the helper adds no \
                 haptics of its own, and where it runs beside the game it drives only the rev \
                 lights and the screen; simulated haptics are the fallback for a wheel that \
                 cannot receive the real thing"
            }
            SimTf::LiveNow(_) => "supported today",
            SimTf::PossibleWithParser => "possible, needs a telemetry parser first",
            SimTf::No => "no usable telemetry",
            SimTf::NotApplicableNative => "not needed: the game has real TrueForce",
        };
        out.push_str(&format!("| {} | {} |\n", g.name, cell));
    }

    out.push_str("\n## What each recipe means\n\n");
    let acc = games::match_title("Assetto Corsa Competizione").expect("registry has ACC");
    out.push_str(&format!(
        "- **Install the shim.** Stage Logitech's signed SDK DLLs into the \
         game's Proton prefix, from the app's Setup page or \
         `tools/install-tf-shim.sh`. {}\n",
        acc.setup_line(WheelCaps::direct_drive())
    ));
    out.push_str(&format!(
        "- **On a wheel with no SDK TrueForce.** {}\n",
        acc.setup_line(WheelCaps::classic())
    ));
    let lmu = games::match_title("Le Mans Ultimate").expect("registry has Le Mans Ultimate");
    out.push_str(&format!(
        "- **Launch via logi-ffb.** {}\n",
        lmu.setup_line(WheelCaps::direct_drive())
    ));
    out.push_str(
        "- **Nothing to do.** The wheel is an ordinary Linux force feedback \
         device. The game drives it directly.\n",
    );

    out.push_str("\n## Confidence\n\n");
    for c in [Confidence::Verified, Confidence::Documented, Confidence::Expected, Confidence::Unknown] {
        let meaning = match c {
            Confidence::Verified => "confirmed end to end by this project",
            Confidence::Documented => "documented by the vendor or a reliable community source",
            Confidence::Expected => "expected to work, not confirmed",
            Confidence::Unknown => "genuinely unknown",
        };
        let n = games::GAMES.iter().filter(|g| g.confidence == c).count();
        out.push_str(&format!("- **{}** ({n} titles): {meaning}\n", c.label()));
    }
    out
}

#[test]
fn game_setup_doc_matches_the_registry() {
    let rendered = render();
    let path = doc_path();
    if std::env::var_os("UPDATE_GAME_SETUP").is_some() {
        std::fs::write(&path, &rendered).expect("write docs/GAME_SETUP.md");
        return;
    }
    let committed = std::fs::read_to_string(&path).expect("read docs/GAME_SETUP.md");
    assert_eq!(
        committed, rendered,
        "docs/GAME_SETUP.md is stale. Regenerate it:\n\
         \n    UPDATE_GAME_SETUP=1 cargo test -p logi-wheel-core --test game_setup_doc\n"
    );
}

/// A title that has real TrueForce must never be described as merely
/// "supported today" by the simulated-TrueForce table, because a reader with
/// a direct-drive wheel takes that to mean the game now needs something it
/// does not. Assetto Corsa Competizione is the case: it has native
/// TrueForce and gained a telemetry decoder for the wheels that cannot
/// receive it, and for a while its row and AC EVO's flatly contradicted
/// each other.
#[test]
fn a_native_trueforce_title_never_reads_as_needing_the_simulated_kind() {
    let doc = render();
    for g in games::GAMES.iter().filter(|g| g.ffb == Ffb::TrueForceShim) {
        if g.simulated_tf.live_id().is_none() {
            continue;
        }
        let row = doc
            .lines()
            .filter(|l| l.starts_with(&format!("| {} |", g.name)))
            .find(|l| !l.contains("Proton") && !l.contains("Not on Linux"))
            .unwrap_or_else(|| panic!("no simulated-TrueForce row for {}", g.name));
        assert!(
            row.contains("own TrueForce") && row.contains("fallback"),
            "{} has real TrueForce; its row must say so rather than imply the \
             simulated kind is what to use: {row}",
            g.name
        );
    }
}

/// The doc's whole purpose is that the two wheel columns say different
/// things for the SDK titles. If they ever render identically the file is
/// no longer carrying the distinction it exists for, whatever else it says.
#[test]
fn the_two_wheel_columns_actually_differ_for_sdk_titles() {
    let acc = games::match_title("Assetto Corsa Competizione").unwrap();
    let dd = recipe_cell(acc, WheelCaps::direct_drive());
    let classic = recipe_cell(acc, WheelCaps::classic());
    assert_ne!(dd, classic);
    // The direct-drive column installs the shim and launches through
    // logi-launch; it never shows a bare PROTON_ENABLE_HIDRAW, which is
    // unsafe without the proxy the launcher stages (an RS50 in ACC,
    // 2026-09-07).
    assert!(dd.contains("Install the shim") && dd.contains("logi-launch %command%"), "{dd}");
    assert!(!dd.contains("PROTON_ENABLE_HIDRAW"), "{dd}");
    // The G923 column names the route that works and says to leave the
    // variable alone; the hazard is the assignment, not the name.
    assert!(!classic.contains("PROTON_ENABLE_HIDRAW=1"), "{classic}");
    assert!(classic.contains("unset"), "{classic}");
    assert!(
        classic.contains("simulated TrueForce"),
        "the G923 column must name the route that actually works: {classic}"
    );
}
