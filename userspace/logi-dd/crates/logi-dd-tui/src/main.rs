mod app;
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
    execute!(out, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;

    let res = loop {
        term.draw(|f| ui::draw(f, &app))?;
        if let Event::Key(k) = event::read()? {
            if k.kind == event::KeyEventKind::Press {
                app.on_key(k.code);
            }
        }
        if app.quit {
            break Ok(());
        }
    };

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    res
}
