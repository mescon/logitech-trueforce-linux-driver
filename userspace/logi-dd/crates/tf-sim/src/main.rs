// SPDX-License-Identifier: GPL-2.0-only
//! `logi-tf-sim` entry point.

use std::process::ExitCode;

use tf_sim::config::Config;
use tf_sim::{capture, daemon, sweep};

const USAGE: &str = "\
logi-tf-sim: simulated TrueForce daemon

Synthesizes engine haptics from game UDP telemetry and streams them
through the wheel's TrueForce audio path, for games without native
TrueForce support.

usage: logi-tf-sim             run the daemon (listens for telemetry)
       logi-tf-sim --sweep [pitch%]   play a 6 s synthetic RPM sweep and exit
                               (pitch 10-200 overrides the config, default 100)
       logi-tf-sim capture --port <PORT> [--out <FILE>] [--label <TEXT>]
                               record a game's raw UDP telemetry to a file,
                               to bootstrap support for a game not listed below
       logi-tf-sim -h|--help   show this help

The daemon listens passively on UDP for Codemasters/EA telemetry
(DiRT Rally 2.0 and friends, modern F1, and EA Sports WRC, all on
port 20777), Project CARS 2 / Automobilista 2 telemetry (port 5606),
and BeamNG OutGauge (port 4444), and streams while telemetry flows.
Enable UDP telemetry in the game's settings.

WARNING: --sweep drives the wheel with real haptic force. Hold the rim.

While streaming, the daemon also drives the wheel's rev-LED display
from telemetry RPM (leds=0 turns that off).

If your game sends UDP telemetry but is not one of the formats above,
run `logi-tf-sim capture --port <PORT>` while driving to record its raw
packets, then open an issue with the resulting file so support can be
added.

config: ~/.config/logi-dd/tf-sim.conf (key=value)
  enabled=0|1, intensity=0-100, leds=0|1, port.codemasters=, port.pcars=,
  port.beamng=, game.<id>.enabled=0|1, game.<id>.intensity=0-100
  (game ids: dirt-rally-2, codemasters, ams2-pcars2, f1, beamng, ea-wrc)";

/// Parsed `capture` subcommand arguments.
#[derive(Debug, PartialEq, Eq, Clone)]
struct CaptureArgs {
    port: u16,
    out: Option<String>,
    label: String,
}

/// What argv asked for.
#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Daemon,
    Sweep(Option<u8>),
    Capture(CaptureArgs),
    Version,
    Help,
    Unknown(String),
    /// A `capture` argument error, with a specific message (as opposed to
    /// [`Mode::Unknown`]'s bare "unknown argument").
    CaptureUsage(String),
}

/// Parse everything after `capture` into [`CaptureArgs`].
fn parse_capture(args: &[String]) -> Result<CaptureArgs, String> {
    let mut port: Option<u16> = None;
    let mut out: Option<String> = None;
    let mut label = String::new();
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let next = args.get(i + 1).cloned().ok_or_else(|| format!("capture: {flag} requires a value"));
        match flag {
            "--port" => {
                let v = next?;
                port = Some(v.parse::<u16>().map_err(|_| format!("capture: invalid --port value '{v}'"))?);
                i += 2;
            }
            "--out" => {
                out = Some(next?);
                i += 2;
            }
            "--label" => {
                label = next?;
                i += 2;
            }
            other => return Err(format!("capture: unknown argument '{other}'")),
        }
    }
    let port = port.ok_or_else(|| "capture: --port <PORT> is required".to_string())?;
    Ok(CaptureArgs { port, out, label })
}

/// Only the first argument decides the mode; there are no other subcommands.
fn parse(args: &[String]) -> Mode {
    match args.get(1).map(String::as_str) {
        None => Mode::Daemon,
        Some("-h") | Some("--help") => Mode::Help,
        Some("--version") | Some("-V") => Mode::Version,
        Some("--sweep") => match args.get(2) {
            None => Mode::Sweep(None),
            Some(p) => match p.parse::<u8>() {
                Ok(v) if (10..=200u16).contains(&u16::from(v)) => Mode::Sweep(Some(v)),
                _ => Mode::Unknown(p.clone()),
            },
        },
        Some("capture") => match parse_capture(&args[2..]) {
            Ok(a) => Mode::Capture(a),
            Err(msg) => Mode::CaptureUsage(msg),
        },
        Some(other) => Mode::Unknown(other.to_string()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let result = match parse(&args) {
        Mode::Help => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Mode::Unknown(arg) => {
            eprintln!("logi-tf-sim: unknown argument '{arg}'\n{USAGE}");
            return ExitCode::FAILURE;
        }
        Mode::CaptureUsage(msg) => {
            eprintln!("logi-tf-sim: {msg}\n{USAGE}");
            return ExitCode::FAILURE;
        }
        Mode::Daemon => daemon::run(&Config::load()),
        Mode::Version => {
            println!("logi-tf-sim {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Mode::Sweep(pitch) => daemon::install_signal_handlers()
            .and_then(|()| sweep::run(&Config::load(), pitch, &daemon::STOP)),
        Mode::Capture(a) => {
            let out = a.out.unwrap_or_else(|| format!("./logi-tf-capture-{}.bin", a.port));
            daemon::install_signal_handlers()
                .and_then(|()| capture::run(a.port, &out, &a.label, &daemon::STOP))
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("logi-tf-sim: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_args_runs_the_daemon() {
        assert_eq!(parse(&argv(&["logi-tf-sim"])), Mode::Daemon);
    }

    #[test]
    fn sweep_flag_parses() {
        assert_eq!(parse(&argv(&["logi-tf-sim", "--sweep"])), Mode::Sweep(None));
        assert_eq!(parse(&argv(&["logi-tf-sim", "--sweep", "50"])), Mode::Sweep(Some(50)));
        assert!(matches!(parse(&argv(&["logi-tf-sim", "--sweep", "999"])), Mode::Unknown(_)));
    }

    #[test]
    fn help_flags_parse() {
        assert_eq!(parse(&argv(&["logi-tf-sim", "-h"])), Mode::Help);
        assert_eq!(parse(&argv(&["logi-tf-sim", "--help"])), Mode::Help);
    }

    #[test]
    fn unknown_arguments_are_rejected() {
        assert_eq!(parse(&argv(&["logi-tf-sim", "--bogus"])), Mode::Unknown("--bogus".into()));
    }

    #[test]
    fn capture_parses_all_flags() {
        let mode = parse(&argv(&[
            "logi-tf-sim",
            "capture",
            "--port",
            "20999",
            "--out",
            "/tmp/x.bin",
            "--label",
            "my game",
        ]));
        assert_eq!(
            mode,
            Mode::Capture(CaptureArgs {
                port: 20999,
                out: Some("/tmp/x.bin".into()),
                label: "my game".into()
            })
        );
    }

    #[test]
    fn capture_defaults_out_and_label_when_omitted() {
        let mode = parse(&argv(&["logi-tf-sim", "capture", "--port", "4000"]));
        assert_eq!(mode, Mode::Capture(CaptureArgs { port: 4000, out: None, label: String::new() }));
    }

    #[test]
    fn capture_requires_port() {
        assert!(matches!(parse(&argv(&["logi-tf-sim", "capture"])), Mode::CaptureUsage(_)));
        assert!(matches!(parse(&argv(&["logi-tf-sim", "capture", "--out", "x"])), Mode::CaptureUsage(_)));
    }

    #[test]
    fn capture_rejects_bad_port_and_unknown_flags() {
        assert!(matches!(
            parse(&argv(&["logi-tf-sim", "capture", "--port", "notanumber"])),
            Mode::CaptureUsage(_)
        ));
        assert!(matches!(
            parse(&argv(&["logi-tf-sim", "capture", "--port", "4000", "--bogus", "1"])),
            Mode::CaptureUsage(_)
        ));
    }

    #[test]
    fn capture_flag_missing_its_value_is_rejected() {
        assert!(matches!(parse(&argv(&["logi-tf-sim", "capture", "--port"])), Mode::CaptureUsage(_)));
    }
}
