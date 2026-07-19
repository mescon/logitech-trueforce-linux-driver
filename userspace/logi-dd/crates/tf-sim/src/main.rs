// SPDX-License-Identifier: GPL-2.0-only
//! `logi-tf-sim` entry point.

use std::process::ExitCode;

use tf_sim::config::Config;
use tf_sim::{daemon, sweep};

const USAGE: &str = "\
logi-tf-sim: simulated TrueForce daemon

Synthesizes engine haptics from game UDP telemetry and streams them
through the wheel's TrueForce audio path, for games without native
TrueForce support.

usage: logi-tf-sim             run the daemon (listens for telemetry)
       logi-tf-sim --sweep [pitch%]   play a 6 s synthetic RPM sweep and exit
                               (pitch 10-200 overrides the config, default 100)
       logi-tf-sim -h|--help   show this help

The daemon listens passively on UDP for Codemasters/EA telemetry
(DiRT Rally 2.0 and friends, port 20777) and Project CARS 2 /
Automobilista 2 telemetry (port 5606), and streams while telemetry
flows. Enable UDP telemetry in the game's settings.

WARNING: --sweep drives the wheel with real haptic force. Hold the rim.

While streaming, the daemon also drives the wheel's rev-LED display
from telemetry RPM (leds=0 turns that off).

config: ~/.config/logi-dd/tf-sim.conf (key=value)
  enabled=0|1, intensity=0-100, leds=0|1, port.codemasters=, port.pcars=,
  game.<id>.enabled=0|1, game.<id>.intensity=0-100
  (game ids: dirt-rally-2, codemasters, ams2-pcars2)";

/// What argv asked for.
#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Daemon,
    Sweep(Option<u8>),
    Help,
    Unknown(String),
}

/// Only the first argument decides the mode; there are no subcommands.
fn parse(args: &[String]) -> Mode {
    match args.get(1).map(String::as_str) {
        None => Mode::Daemon,
        Some("-h") | Some("--help") => Mode::Help,
        Some("--sweep") => match args.get(2) {
            None => Mode::Sweep(None),
            Some(p) => match p.parse::<u8>() {
                Ok(v) if (10..=200u16).contains(&u16::from(v)) => Mode::Sweep(Some(v)),
                _ => Mode::Unknown(p.clone()),
            },
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
        Mode::Daemon => daemon::run(&Config::load()),
        Mode::Sweep(pitch) => daemon::install_signal_handlers()
            .and_then(|()| sweep::run(&Config::load(), pitch, &daemon::STOP)),
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
}
