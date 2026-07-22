// SPDX-License-Identifier: GPL-2.0-only
//! Simulated TrueForce: engine haptics for games without native TrueForce.
//!
//! `logi-tf-sim` listens passively for game UDP telemetry (the Codemasters/EA
//! float array, modern F1, and EA Sports WRC all share port 20777; Project
//! CARS 2 / Automobilista 2 on port 5606; BeamNG OutGauge on port 4444),
//! synthesizes an engine-note sample stream from rpm and throttle, and plays
//! it through the wheel's real TrueForce audio path via the in-repo
//! libtrueforce C library.
//!
//! Layering:
//! - [`codemasters`] / [`pcars`] / [`wrc`]: pure
//!   `&[u8] -> Option<(game id, Telemetry)>` packet parsers, fixture-tested.
//! - [`f1`] / [`beamng`]: decoders that are pure per packet but hold a
//!   running redline (`max_rpm`), which those formats do not transmit.
//! - [`synth`]: pure engine-note generator (1 kHz sample stream).
//! - [`config`]: `~/.config/logi-dd/tf-sim.conf` key=value store.
//! - [`tf`]: safe wrapper over the libtrueforce FFI (stream lifecycle).
//! - [`leds`]: the rev-display feeder (`wheel_rev_level` via sysfs).
//! - [`daemon`]: the UDP listen / synthesize / watchdog loop.
//! - [`sweep`]: the `--sweep` hardware-test mode (synthetic RPM sweep).
//! - [`capture`]: the `capture` subcommand, for recording an unsupported
//!   game's raw telemetry so its wire format can be added later.

pub mod beamng;
pub mod capture;
pub mod codemasters;
pub mod config;
pub mod daemon;
pub mod error;
pub mod f1;
pub mod leds;
pub mod pcars;
pub mod sweep;
pub mod synth;
pub mod telemetry;
pub mod tf;
pub mod wrc;

pub use error::{Error, Result};
