// SPDX-License-Identifier: GPL-2.0-only
//! `logi-tf-relay`: the Wine-side shared-memory telemetry relay.
//!
//! Runs INSIDE the game's Proton prefix (it is a Windows executable), opens
//! the sim's named shared-memory section with the ordinary Win32 API - which
//! Wine implements, exactly as G HUB and SimHub do on Windows - and forwards
//! engine telemetry over localhost UDP in the relay wire format
//! (`logi_wheel_core::relay`) that `logi-tf-sim` listens for on port 20780.
//! Spec: `dev/docs/shared-memory-telemetry-plan.md`.
//!
//! Two modes:
//!
//! - `logi-tf-relay --game <id> --dump <file>`: open the
//!   section, write its first bytes to `<file>`, exit. This produces the
//!   REAL byte fixture each per-game decoder is written and unit-tested
//!   against - the same discipline the native UDP parsers follow. Run it
//!   from inside the prefix while a session is live.
//! - `logi-tf-relay --game <id>` (normal mode): stream telemetry. Every
//!   game in `games` decodes today, each having earned a trustworthy
//!   layout a different way; see each decoder's module docs.
//!
//! On non-Windows hosts this compiles to a stub that says to cross-compile
//! (`cargo build -p logi-tf-relay --target x86_64-pc-windows-gnu`), so the
//! workspace always builds without a Windows toolchain.

mod assettocorsa;
mod games;
mod iracing;
mod raceroom;
mod rfactor2;

use std::process::ExitCode;

const USAGE: &str = "logi-tf-relay: shared-memory telemetry for logi-tf-sim (runs inside the Proton prefix)

USAGE:
  logi-tf-relay --game <id>                   stream telemetry to logi-tf-sim
  logi-tf-relay --game <id> --dump <file>     write the section's bytes to <file>
  logi-tf-relay --section <name> --dump <file>  dump any named section

Games:  iracing, raceroom, assetto, acc, ac-evo, rf2, lmu

--dump is for reporting a game that decodes nothing: take it while a session
is actually RUNNING, sitting in the menus is not always enough, and send the
file to the project. The rule here is a trustworthy layout before every
decoder, never struct offsets from memory.";

/// Max bytes `--dump` writes: enough for every header + descriptor table we
/// know of (iRacing: 112-byte header + ~300 varHeaders à 144 byte ≈ 43 KiB)
/// without dragging a whole rF2 vehicle array to disk.
#[cfg(windows)]
const DUMP_LIMIT: usize = 64 * 1024;

#[derive(Debug)]
struct Args {
    section: Option<String>,
    /// The second section this game's decoder needs, when it needs one.
    /// Assetto Corsa's redline and the rF2 family's "which car is the
    /// player" both live outside the section carrying engine speed.
    aux: Option<(String, usize)>,
    dump: Option<String>,
    /// Which known game was named, when one was. `--section` alone leaves
    /// this `None`: an arbitrary section can be dumped but not decoded,
    /// because a decoder is per format, not per section name.
    game: Option<&'static str>,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut section = None;
    let mut aux = None;
    let mut dump = None;
    let mut game_id = None;
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--game" => {
                let id = it.next().ok_or("--game needs a game id")?;
                // The known-ids list is derived rather than written out: a
                // hardcoded copy went stale the first time a game was added.
                let game = games::by_id(id).ok_or_else(|| {
                    let known: Vec<&str> = games::GAMES.iter().map(|g| g.id).collect();
                    format!("unknown game {id:?} (known: {})", known.join(", "))
                })?;
                if let Some(prerequisite) = game.prerequisite {
                    eprintln!("logi-tf-relay: note for {}: {}", game.name, prerequisite);
                }
                section = Some(game.section.to_string());
                aux = game.aux_section.map(|(name, len)| (name.to_string(), len));
                game_id = Some(game.id);
            }
            "--section" => {
                section = Some(it.next().ok_or("--section needs a name")?.clone());
                // An explicitly named section is dumped, never decoded, so
                // whatever a preceding --game set up no longer applies.
                aux = None;
                game_id = None;
            }
            "--dump" => {
                dump = Some(it.next().ok_or("--dump needs a file path")?.clone());
            }
            "--help" | "-h" => return Err(String::new()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(Args { section, aux, dump, game: game_id })
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("logi-tf-relay: {msg}\n");
            }
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let Some(section) = args.section else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };

    let decodable = args.game.and_then(games::by_id).is_some_and(|g| g.decodable);
    match args.dump {
        Some(path) => run_dump(&section, &path),
        // Each decodable game earned that status a different way; see the
        // `decodable` field in `games` and each decoder's module docs.
        None if decodable => {
            let read_len = args
                .game
                .and_then(games::by_id)
                .map_or(games::DEFAULT_READ_LEN, |g| g.read_len);
            run_stream(&section, args.aux.as_ref(), args.game.unwrap_or(""), read_len)
        }
        None => {
            // Reached by --section with no --game: a section can always be
            // dumped, but decoding is per format, not per section name.
            eprintln!(
                "logi-tf-relay: nothing decodes {section:?}. Name a game with \
                 --game to stream, or dump the bytes:\n\
                 Run: logi-tf-relay --section \"{section}\" --dump dump.bin"
            );
            ExitCode::FAILURE
        }
    }
}

/// Stream decoded telemetry to the daemon until interrupted.
///
/// Re-reads the section every tick rather than holding a mapped view: the
/// section is small, the rate is low, and a fresh read cannot observe a
/// half-updated buffer the way a retained pointer can. Undecodable ticks
/// are skipped silently, since a menu, a replay or a paused session all
/// legitimately produce them.
#[cfg(windows)]
fn run_stream(
    section: &str,
    aux: Option<&(String, usize)>,
    game: &str,
    read_len: usize,
) -> ExitCode {
    use std::net::UdpSocket;

    let port = std::env::var("LOGI_TF_SIM_RELAY_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .filter(|p| *p != 0)
        .unwrap_or(logi_wheel_core::relay::DEFAULT_PORT);

    let Ok(socket) = UdpSocket::bind("127.0.0.1:0") else {
        eprintln!("logi-tf-relay: could not open a local UDP socket");
        return ExitCode::FAILURE;
    };
    if socket.connect(("127.0.0.1", port)).is_err() {
        eprintln!("logi-tf-relay: could not target 127.0.0.1:{port}");
        return ExitCode::FAILURE;
    }

    eprintln!("logi-tf-relay: streaming {section:?} to 127.0.0.1:{port} (ctrl-c to stop)");
    let mut warned = false;
    // Owned by the session rather than being process-global: it is state
    // about this run of this game, and a global made the tests
    // order-dependent as well as being wrong across sessions.
    let mut airborne_gate = assettocorsa::AirborneGate::default();
    loop {
        match win::read_section(section, read_len) {
            Ok(bytes) => {
                warned = false;
                // Assetto Corsa's redline is in a second section, read on
                // the same tick so a car change cannot pair a new engine
                // speed with the previous car's redline.
                let aux_bytes = match aux {
                    Some((name, len)) => win::read_section(name, *len).ok(),
                    None => None,
                };
                let sample = match game {
                    raceroom::ID => raceroom::decode(&bytes),
                    assettocorsa::ID_EVO => assettocorsa::decode_evo(&bytes),
                    id @ (assettocorsa::ID | assettocorsa::ID_ACC) => {
                        let id =
                            if id == assettocorsa::ID_ACC { assettocorsa::ID_ACC } else { assettocorsa::ID };
                        aux_bytes.and_then(|s| assettocorsa::decode(&bytes, &s, id, &mut airborne_gate))
                    }
                    id @ (rfactor2::ID_RF2 | rfactor2::ID_LMU) => {
                        // The two share a decoder but not a settings
                        // switch, so the id decides which gates the sample.
                        let id = if id == rfactor2::ID_LMU {
                            rfactor2::ID_LMU
                        } else {
                            rfactor2::ID_RF2
                        };
                        aux_bytes.and_then(|s| rfactor2::decode(&bytes, &s, id))
                    }
                    _ => iracing::decode(&bytes),
                };
                if let Some(sample) = sample {
                    let _ = socket.send(&logi_wheel_core::relay::encode(&sample));
                }
            }
            Err(err) => {
                // The game not being up yet is the normal case on startup,
                // so say it once and keep trying rather than exiting.
                if !warned {
                    eprintln!("logi-tf-relay: {section:?} not readable yet ({err}); waiting");
                    warned = true;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}

/// Linux stub: the relay only means anything inside the prefix.
#[cfg(not(windows))]
fn run_stream(
    _section: &str,
    _aux: Option<&(String, usize)>,
    _game: &str,
    _read_len: usize,
) -> ExitCode {
    eprintln!(
        "logi-tf-relay: this is the Linux stub. The relay has to be \
         cross-compiled and run inside the game's Proton prefix:\n  \
         rustup target add x86_64-pc-windows-gnu\n  \
         cargo build --release -p logi-tf-relay --target x86_64-pc-windows-gnu"
    );
    ExitCode::FAILURE
}

#[cfg(windows)]
fn run_dump(section: &str, path: &str) -> ExitCode {
    match win::read_section(section, DUMP_LIMIT) {
        Ok(bytes) => {
            if let Err(err) = std::fs::write(path, &bytes) {
                eprintln!("logi-tf-relay: could not write {path:?}: {err}");
                return ExitCode::FAILURE;
            }
            println!(
                "logi-tf-relay: wrote {} bytes from {section:?} to {path:?}",
                bytes.len()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!(
                "logi-tf-relay: could not open {section:?}: {err}\n\
                 Check that the game is running with a session actually live \
                 (sitting in the menus is not always enough), and that the \
                 relay runs in the SAME Proton prefix as the game."
            );
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn run_dump(_section: &str, _path: &str) -> ExitCode {
    eprintln!(
        "logi-tf-relay: this is the Linux stub. The relay has to be \
         cross-compiled and run inside the game's Proton prefix:\n  \
         rustup target add x86_64-pc-windows-gnu\n  \
         cargo build --release -p logi-tf-relay --target x86_64-pc-windows-gnu"
    );
    ExitCode::FAILURE
}

/// The Win32 side: open a named section read-only and copy out its bytes.
/// Isolated so everything unsafe lives in one place with one contract.
#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
    use windows_sys::Win32::System::Memory::{
        MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, VirtualQuery, FILE_MAP_READ,
        MEMORY_BASIC_INFORMATION,
    };

    /// Open `section`, map it read-only, and return up to `limit` bytes.
    /// The copy is byte-for-byte; a live game keeps writing while we read,
    /// which is fine for a dump fixture (the header fields we care about
    /// are static once a session is up).
    pub fn read_section(section: &str, limit: usize) -> Result<Vec<u8>, String> {
        let wide: Vec<u16> = section.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let mapping = OpenFileMappingW(FILE_MAP_READ, 0, wide.as_ptr());
            if mapping.is_null() {
                return Err(format!("OpenFileMappingW: error {}", GetLastError()));
            }
            let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
            if view.Value.is_null() {
                let err = GetLastError();
                CloseHandle(mapping);
                return Err(format!("MapViewOfFile: error {err}"));
            }

            // The section size: VirtualQuery on the view gives the region length.
            let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            // Falling back to `limit` here would read that many bytes from
            // a view that may be smaller, which is an out-of-bounds read
            // rather than a short answer. The rF2 family asks for 236 KiB,
            // so guessing is not survivable; not knowing the size is a
            // failed read.
            if VirtualQuery(
                view.Value,
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            ) == 0
            {
                let err = GetLastError();
                UnmapViewOfFile(view);
                CloseHandle(mapping);
                return Err(format!("VirtualQuery: error {err}"));
            }
            let size = info.RegionSize.min(limit);

            let bytes = std::slice::from_raw_parts(view.Value as *const u8, size).to_vec();
            UnmapViewOfFile(view);
            CloseHandle(mapping);
            Ok(bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn game_id_resolves_to_its_section() {
        let a = parse_args(&args(&["--game", "iracing", "--dump", "d.bin"])).unwrap();
        assert_eq!(a.section.as_deref(), Some("Local\\IRSDKMemMapFileName"));
        assert_eq!(a.dump.as_deref(), Some("d.bin"));
    }

    #[test]
    fn explicit_section_wins_over_nothing() {
        let a = parse_args(&args(&["--section", "$R3E", "--dump", "x"])).unwrap();
        assert_eq!(a.section.as_deref(), Some("$R3E"));
    }

    #[test]
    fn unknown_game_and_missing_values_are_errors() {
        assert!(parse_args(&args(&["--game", "gran-turismo"])).is_err());
        assert!(parse_args(&args(&["--game"])).is_err());
        assert!(parse_args(&args(&["--dump"])).is_err());
        assert!(parse_args(&args(&["--frobnicate"])).is_err());
    }

    #[test]
    fn help_is_the_empty_error() {
        assert_eq!(parse_args(&args(&["--help"])).unwrap_err(), "");
    }
}
