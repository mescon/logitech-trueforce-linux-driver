//! Command-line front end.
//!
//! There is no subcommand: the common case is running a game with the proxy
//! active (`logi-ffb <game command...>`), so the whole remaining argument
//! list is taken verbatim as the command to exec. `--daemon` runs the proxy
//! standalone in the foreground instead.

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use nix::sys::signal::{self, SaFlags, SigAction, SigHandler, SigSet, Signal};

use crate::proxy::Proxy;
use crate::{proxy, steering};

const USAGE: &str = "usage: logi-ffb <game command>\n       logi-ffb --daemon";

/// The result of parsing argv.
#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    /// Run this game command with the proxy active.
    Run { cmd: Vec<String> },
    /// Run the proxy standalone in the foreground.
    Daemon,
    /// Print usage and exit.
    Usage,
}

/// Parse `args` (`args[0]` is the program name) into a [`Parsed`] value.
///
/// Only the first non-program token decides the mode: `--daemon` selects
/// [`Parsed::Daemon`]; `-h`/`--help`/no further tokens select
/// [`Parsed::Usage`]; anything else is taken verbatim, along with every
/// token after it, as [`Parsed::Run`]. A game path never starts with `--`,
/// so there is no need for a `--` separator or a `run` subcommand.
pub fn parse(args: &[String]) -> Parsed {
    match args.get(1).map(String::as_str) {
        None | Some("-h") | Some("--help") => Parsed::Usage,
        Some("--daemon") => Parsed::Daemon,
        Some(_) => Parsed::Run { cmd: args[1..].to_vec() },
    }
}

/// Set by [`handle_stop_signal`] when running as `--daemon`; the daemon
/// loop polls this instead of receiving it as a parameter, since a signal
/// handler cannot capture state.
static DAEMON_STOP: AtomicBool = AtomicBool::new(false);

/// Installed for `SIGINT`/`SIGTERM` in daemon mode. Only performs an atomic
/// store, which is async-signal-safe, so it is sound to run at an arbitrary
/// interruption point.
extern "C" fn handle_stop_signal(_signal: libc::c_int) {
    DAEMON_STOP.store(true, Ordering::SeqCst);
}

/// Install `handle_stop_signal` for `SIGINT` and `SIGTERM`.
///
/// # Safety
/// `sigaction` is unsafe because a handler that is not async-signal-safe
/// (or that panics/unwinds across the signal boundary) can corrupt process
/// state if invoked mid-syscall. `handle_stop_signal` performs nothing but
/// an atomic store, so installing it here is sound.
fn install_daemon_signal_handlers() -> crate::Result<()> {
    let action = SigAction::new(SigHandler::Handler(handle_stop_signal), SaFlags::empty(), SigSet::empty());
    for sig in [Signal::SIGINT, Signal::SIGTERM] {
        unsafe { signal::sigaction(sig, &action) }
            .map_err(|e| crate::Error::Io(format!("sigaction({sig})"), std::io::Error::from(e)))?;
    }
    Ok(())
}

/// Entry point called from `main`. Replaces the Task 1 stub.
pub fn dispatch(args: &[String]) -> crate::Result<ExitCode> {
    match parse(args) {
        Parsed::Usage => {
            eprintln!("{USAGE}");
            // No arguments at all is a usage error; an explicit -h/--help
            // is a successful request for help.
            if args.len() <= 1 {
                Ok(ExitCode::FAILURE)
            } else {
                Ok(ExitCode::SUCCESS)
            }
        }
        Parsed::Daemon => run_daemon(),
        Parsed::Run { cmd } => run_game(cmd),
    }
}

/// Run the proxy standalone in the foreground until `SIGINT`/`SIGTERM`.
fn run_daemon() -> crate::Result<ExitCode> {
    let paths = proxy::discover_wheel()?;
    let mut proxy = Proxy::new(paths)?;
    install_daemon_signal_handlers()?;
    proxy.run(&DAEMON_STOP)?;
    Ok(ExitCode::SUCCESS)
}

/// Bring the proxy up on a background thread, steer enumeration away from
/// the real wheel, exec `cmd` with the steering env applied, and wait for
/// it. The proxy is stopped and joined before returning, whether the child
/// exited cleanly or its spawn failed.
fn run_game(cmd: Vec<String>) -> crate::Result<ExitCode> {
    let paths = proxy::discover_wheel()?;
    let (vendor, product, name) = (paths.vendor, paths.product, paths.name.clone());
    let mut proxy = Proxy::new(paths)?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let proxy_thread = std::thread::spawn(move || proxy.run(&stop_for_thread));

    let plan = steering::plan_for(vendor, product, &name);
    steering::apply(&plan, std::env::var("WINEPREFIX").ok().as_deref())?;

    let mut command = std::process::Command::new(&cmd[0]);
    command.args(&cmd[1..]);
    for (k, v) in steering::child_env(&plan) {
        command.env(k, v);
    }

    let spawn_result = command.spawn().and_then(|mut child| child.wait());

    stop.store(true, Ordering::SeqCst);
    let join_result = proxy_thread.join();

    let status = spawn_result.map_err(|e| crate::Error::Io(format!("spawn {}", cmd[0]), e))?;

    match join_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => eprintln!("logi-ffb: proxy loop error: {e}"),
        Err(_) => eprintln!("logi-ffb: proxy thread panicked"),
    }

    Ok(ExitCode::from(status.code().unwrap_or(0) as u8))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn bare_command_parses_as_run() {
        let a = argv(&["logi-ffb", "echo", "hi"]);
        assert!(matches!(parse(&a), Parsed::Run { ref cmd } if cmd == &["echo", "hi"]));
    }
    #[test]
    fn daemon_flag_parses() {
        let a = argv(&["logi-ffb", "--daemon"]);
        assert!(matches!(parse(&a), Parsed::Daemon));
    }
    #[test]
    fn no_args_is_usage() {
        let a = argv(&["logi-ffb"]);
        assert!(matches!(parse(&a), Parsed::Usage));
    }
}
