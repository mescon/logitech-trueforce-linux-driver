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

/// The two wheel classes the recipes differ between. There is no third:
/// what changes a recipe is whether the wheel answers Logitech's TrueForce
/// SDK, and that splits the supported wheels exactly here.
const CLASSES: [(&str, WheelCaps); 2] = [
    ("RS50 / G PRO", WheelCaps { sdk_trueforce: true }),
    ("G923", WheelCaps { sdk_trueforce: false }),
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
        return "Turn on simulated TrueForce<br>and leave \
                `PROTON_ENABLE_HIDRAW` unset"
            .to_string();
    }
    let action = match g.setup_action(caps) {
        SetupAction::InstallShim => "Install the shim",
        SetupAction::UseLogiFfb => "Launch via logi-ffb",
        SetupAction::SimulatedTrueForce => "Turn on simulated TrueForce",
        SetupAction::WorksOutOfBox => "Nothing to do",
    };
    match g.launch_options(caps) {
        Some(opts) => format!("{action}<br>`{opts}`"),
        None => action.to_string(),
    }
}

fn render() -> String {
    let mut out = String::new();
    out.push_str("# Game setup, per game and per wheel\n\n");
    out.push_str(
        "**Generated file. Do not edit.** It is rendered from the\n\
         compatibility registry in\n\
         `userspace/logi-wheel/crates/logi-wheel-core/src/games.rs` by\n\
         `tests/game_setup_doc.rs`, which fails if this file drifts from it.\n\
         The settings app resolves your own installed games against that same\n\
         registry, so what you read here is what the app will tell you.\n\n",
    );
    out.push_str(
        "What a game needs depends on the wheel as well as the game. The\n\
         direct-drive wheels answer Logitech's TrueForce SDK, so a sim with\n\
         built-in TrueForce reaches them through the staged SDK DLLs, and\n\
         `PROTON_ENABLE_HIDRAW=1` is what lets it.\n\n\
         The G923 does not answer that SDK. Setting the variable there does\n\
         not add TrueForce, it diverts the game to raw HID reports the wheel\n\
         cannot drive force feedback through, so it costs you the force\n\
         feedback you already had. Leave it unset.\n\n\
         That wheel is still capable of haptics in those games, by a\n\
         different route: `logi-tf-sim` synthesizes an engine note from the\n\
         game's own telemetry, read out of its shared memory by a small relay\n\
         (`docs/SHARED_MEMORY_RELAY.md`). Confirmed working on a G923 in\n\
         Assetto Corsa Competizione and EVO.\n\n\
         There is a second route that would be better if it worked: installing\n\
         the shim with `--proxy` puts this project's own SDK proxy in the\n\
         game's path to copy the TrueForce the game is already producing,\n\
         which is the real thing rather than an imitation of it. Nobody has\n\
         yet got the game to load that proxy, so it is not the recommendation.\n\n",
    );
    out.push_str(
        "Launch options go in Steam under the game's Properties. Paste them\n\
         exactly, `%command%` included: it is the placeholder Steam replaces\n\
         with the game itself, so without it the line replaces the game\n\
         instead of wrapping it.\n\n\
         **Or skip the table.** `logi-launch %command%` works out everything\n\
         below for the game being launched and the wheel attached, and\n\
         applies it: the raw-HID setting only where that wheel wants it, the\n\
         logi-ffb proxy for the DirectInput games, the telemetry daemon, and\n\
         the relay inside the game's prefix for the sims that need one. A\n\
         game with its own TrueForce keeps it, and nothing is layered on\n\
         top. The table below is for doing it by hand, or for working out\n\
         what went wrong. See LAUNCH_OPTIONS.md.\n\n",
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
        "\nRows marked `*` are not confirmed on this driver yet: expected or\n\
         documented rather than tested end to end.\n\n",
    );

    out.push_str("## Simulated TrueForce\n\n");
    out.push_str(
        "Games with no TrueForce of their own can still have engine haptics\n\
         and rev lights, synthesized by `logi-tf-sim` from whatever telemetry\n\
         the game publishes. This works on every supported wheel, including\n\
         the G923: it is ordinary force feedback driven from telemetry, not\n\
         the SDK.\n\n\
         How the telemetry reaches the daemon depends on the game. Most\n\
         broadcast it over UDP and only need that switched on in their own\n\
         settings. Euro Truck Simulator 2 and American Truck Simulator use a\n\
         plugin instead (`docs/SCS_PLUGIN.md`), and iRacing publishes to\n\
         shared memory that a small in-prefix relay forwards\n\
         (`docs/SHARED_MEMORY_RELAY.md`). Either way, enable the game in the\n\
         app's Setup page afterwards.\n\n",
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
                "the game's own TrueForce is the route to use; \
                 simulated is the fallback for a wheel that cannot receive it"
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
        acc.setup_line(WheelCaps { sdk_trueforce: true })
    ));
    out.push_str(&format!(
        "- **On a wheel with no SDK TrueForce.** {}\n",
        acc.setup_line(WheelCaps { sdk_trueforce: false })
    ));
    let lmu = games::match_title("Le Mans Ultimate").expect("registry has Le Mans Ultimate");
    out.push_str(&format!(
        "- **Launch via logi-ffb.** {}\n",
        lmu.setup_line(WheelCaps { sdk_trueforce: true })
    ));
    out.push_str(
        "- **Nothing to do.** The wheel is an ordinary Linux force feedback \
         device and the game drives it directly.\n",
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
    let dd = recipe_cell(acc, WheelCaps { sdk_trueforce: true });
    let classic = recipe_cell(acc, WheelCaps { sdk_trueforce: false });
    assert_ne!(dd, classic);
    assert!(dd.contains("PROTON_ENABLE_HIDRAW=1"), "{dd}");
    // The hazard is the assignment, not the name: this cell mentions the
    // variable precisely in order to say to leave it alone.
    assert!(!classic.contains("PROTON_ENABLE_HIDRAW=1"), "{classic}");
    assert!(classic.contains("unset"), "{classic}");
    assert!(
        classic.contains("simulated TrueForce"),
        "the G923 column must name the route that actually works: {classic}"
    );
}
