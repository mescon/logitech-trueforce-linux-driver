mod app;
mod curve_editor;
mod edit;
mod ui;

use app::App;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use logi_dd_core::sysfs::RealSysfs;
use logi_dd_core::Device;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let device = match Device::discover() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("logi-dd: {e}");
            std::process::exit(1);
        }
    };
    run(App::new(device))
}

fn run(mut app: App<RealSysfs>) -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    if let Err(e) = execute!(out, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    let mut term = match Terminal::new(CrosstermBackend::new(out)) {
        Ok(t) => t,
        Err(e) => {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            return Err(e.into());
        }
    };

    // Run the loop, capturing any error via `break` instead of `?`, so the
    // teardown below always runs and never leaves the terminal in raw mode.
    let res: Result<(), Box<dyn std::error::Error>> = loop {
        if let Err(e) = term.draw(|f| ui::draw(f, &app)) {
            break Err(e.into());
        }
        match event::read() {
            Ok(Event::Key(k)) if k.kind == event::KeyEventKind::Press => app.on_key(k.code),
            Ok(_) => {}
            Err(e) => break Err(e.into()),
        }
        // A queued shim run blocks (an --all-steam Proton-prefix scan can
        // take a while), so show a status line first, run, then drop any
        // keypresses that queued up meanwhile: a buffered second 'i' would
        // otherwise re-trigger the installer the moment it finished.
        if let Some((arg, verb)) = app.take_pending_shim() {
            app.status = format!("shim {verb}: running...");
            if let Err(e) = term.draw(|f| ui::draw(f, &app)) {
                break Err(e.into());
            }
            app.run_shim(arg, verb);
            while let Ok(true) = event::poll(std::time::Duration::ZERO) {
                if event::read().is_err() {
                    break;
                }
            }
        }
        if app.quit {
            break Ok(());
        }
    };

    // Always restore the terminal, regardless of how the loop ended.
    let _ = disable_raw_mode();
    let _ = execute!(term.backend_mut(), LeaveAlternateScreen);
    let _ = term.show_cursor();
    res
}
