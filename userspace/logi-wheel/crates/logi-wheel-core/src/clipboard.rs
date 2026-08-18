//! Putting text on the system clipboard, best-effort.
//!
//! Shared by both front-ends so a copy button and a copy key behave the
//! same way, and so there is one place to teach about a clipboard tool
//! rather than two that can drift.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long a clipboard tool is given to fail before it is taken at its
/// word.
///
/// Long enough that "command not found" or "cannot connect to the display"
/// is seen and the next tool is tried; short enough that nobody notices it
/// on a key press.
const SETTLE: Duration = Duration::from_millis(150);

/// Copy `text` to the clipboard: try `wl-copy` (Wayland), then
/// `xclip -selection clipboard` (X11).
///
/// Every failure is ignored, and deliberately. There may be no clipboard
/// tool installed, no display server at all (a terminal over SSH), or a
/// hung helper; none of that is worth an error dialog, because both
/// front-ends show the text they are copying and it can be selected by
/// hand. Returns whether a tool took the text, for callers that want to
/// say so.
///
/// # Why this does not wait for the tool to exit
///
/// On both Wayland and X11 the clipboard has no storage of its own: the
/// program that owns a selection serves it, on request, for as long as it
/// holds it. So `wl-copy` and `xclip` are *supposed* to keep running after
/// they are handed the text, and how long they run is decided by something
/// else entirely (a clipboard manager taking the selection over, or the
/// next program to copy anything). Waiting for one to exit therefore waits
/// on the user's whole desktop: with no clipboard manager running it never
/// returns at all. It was doing exactly that, which hung the terminal app
/// on the copy key and left a thread stuck behind the window's copy button
/// for the life of the process.
///
/// A tool is given [`SETTLE`] to fail instead. If it exits within that with
/// an error it was not usable (not installed, no display) and the next one
/// is tried; if it is still alive it has the text and is doing its job, so
/// it is left alone.
pub fn copy(text: &str) -> bool {
    if let Ok(child) = Command::new("wl-copy").arg(text).spawn() {
        if took_the_text(child) {
            return true;
        }
    }
    let Ok(mut child) = Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(Stdio::piped())
        .spawn()
    else {
        return false;
    };
    // Dropped, not just written to: xclip reads until end of input, so a
    // pipe still open on this side is a copy that never finishes.
    let Some(mut stdin) = child.stdin.take() else {
        return false;
    };
    let wrote = stdin.write_all(text.as_bytes()).is_ok();
    drop(stdin);
    wrote && took_the_text(child)
}

/// Whether a spawned clipboard tool took the text: it either exited
/// successfully, or it is still running, which is what owning a selection
/// looks like. Only an early failure counts as a refusal.
fn took_the_text(mut child: Child) -> bool {
    let deadline = Instant::now() + SETTLE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            // Still holding the selection, which is the job. Left running
            // on purpose; the caller's process reaps it on exit.
            Ok(None) if Instant::now() >= deadline => return true,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tool that lives on, the way a real clipboard owner does, must read
    /// as a successful copy rather than as a call that never returns.
    #[test]
    fn a_tool_that_keeps_running_counts_as_a_copy() {
        let child = Command::new("sleep").arg("30").spawn().expect("sleep exists");
        let started = Instant::now();
        assert!(took_the_text(child), "a live clipboard owner is a copy that worked");
        assert!(
            started.elapsed() < SETTLE * 4,
            "it must not wait for the tool to exit: took {:?}",
            started.elapsed()
        );
    }

    /// The other half: a tool that is there but cannot do the job (no
    /// display, bad arguments) must be reported as a failure, so the caller
    /// falls through to the next one.
    #[test]
    fn a_tool_that_fails_immediately_is_not_a_copy() {
        let child = Command::new("false").spawn().expect("false exists");
        assert!(!took_the_text(child));
    }

    #[test]
    fn a_tool_that_succeeds_immediately_is_a_copy() {
        let child = Command::new("true").spawn().expect("true exists");
        assert!(took_the_text(child));
    }
}
