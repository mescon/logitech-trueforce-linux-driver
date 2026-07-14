use crate::app::App;
use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::{Category, Device, Mode, Value};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;
use std::collections::BTreeMap;

// Only the 16 named ANSI colours are used, so the scheme adapts to the user's
// terminal palette (light or dark) and needs no truecolor support.

pub fn draw<S: SysfsIo>(f: &mut Frame, app: &App<S>) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1), Constraint::Length(2)])
        .split(f.area());

    // header: device identity + current mode (mode coloured green/yellow)
    let info = app.device.info().ok();
    let header = match &info {
        Some(i) => {
            let (mode_str, mode_col) = match i.mode {
                Mode::Desktop => ("desktop", Color::Green),
                Mode::Onboard => ("onboard", Color::Yellow),
            };
            Line::from(vec![
                Span::styled(
                    " logi-dd",
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("   serial {}   fw {}   ", i.serial, i.firmware),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw("mode: "),
                Span::styled(
                    mode_str,
                    Style::default().fg(mode_col).add_modifier(Modifier::BOLD),
                ),
            ])
        }
        None => Line::from(Span::styled(
            " logi-dd   no wheel found",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
    };
    f.render_widget(
        Paragraph::new(header).block(Block::default().borders(Borders::ALL)),
        root[0],
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(20), Constraint::Min(1)])
        .split(root[1]);

    // categories
    let cats: Vec<ListItem> = Category::ALL
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let style = if i == app.cat_idx {
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            ListItem::new(c.label()).style(style)
        })
        .collect();
    f.render_widget(
        List::new(cats).block(Block::default().borders(Borders::ALL).title("Category")),
        body[0],
    );

    // settings in the selected category
    let names = app.profile_names();
    let rows: Vec<ListItem> = app
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let spec = Device::<S>::spec(row.attr);
            // the edit state, only for the row being edited
            let editing = app.edit.as_ref().filter(|_| i == app.row_idx);

            let (val, mut val_style) = if !row.available {
                ("(not on this wheel)".to_string(), Style::default().fg(Color::DarkGray))
            } else if row.attr == "wheel_profile" {
                // show the profile number with its onboard name
                let n = match (editing.map(|e| &e.draft), &row.value) {
                    (Some(Value::Int(n)), _) => *n,
                    (_, Ok(Value::Int(n))) => *n,
                    _ => -1,
                };
                (profile_label(n, &names), value_style(editing.is_some(), false))
            } else if row.attr == "wheel_profile_names" {
                match &row.value {
                    Ok(Value::Text(s)) => (
                        s.lines().collect::<Vec<_>>().join("    "),
                        Style::default().fg(Color::Gray),
                    ),
                    Err(e) => (format!("<{e}>"), Style::default().fg(Color::Red)),
                    _ => ("?".to_string(), Style::default()),
                }
            } else {
                match (&row.value, spec) {
                    (Ok(v), Some(s)) => {
                        let text = match editing {
                            Some(ed) => ed.display(),
                            None => s.kind.display(v),
                        };
                        (text, value_style(editing.is_some(), false))
                    }
                    (Err(e), _) => (format!("<{e}>"), value_style(false, true)),
                    _ => ("?".to_string(), Style::default()),
                }
            };
            if editing.is_some() {
                val_style = val_style.add_modifier(Modifier::BOLD);
            }

            let line = Line::from(vec![
                Span::styled(format!("{:<24}", row.label), Style::default().fg(Color::Gray)),
                Span::raw(" "),
                Span::styled(val, val_style),
            ]);
            let mut item = ListItem::new(line);
            if i == app.row_idx {
                item = item.style(Style::default().add_modifier(Modifier::REVERSED));
            }
            item
        })
        .collect();
    f.render_widget(
        List::new(rows).block(Block::default().borders(Borders::ALL).title("Settings")),
        body[1],
    );

    // status line (green on success, red on trouble) + a dim help line
    let help = if app.edit.is_some() {
        "editing:  <-/->  adjust    type  text    Enter  commit    Esc  cancel"
    } else {
        "up/down select   <-/-> category   Enter edit   d toggle desktop/onboard   r refresh   q quit"
    };
    let lines = vec![
        Line::from(Span::styled(
            app.status.clone(),
            Style::default().fg(status_colour(&app.status)),
        )),
        Line::from(Span::styled(
            help.to_string(),
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(Paragraph::new(lines), root[2]);
}

/// Render a profile number with its onboard slot name.
fn profile_label(n: i32, names: &BTreeMap<u8, String>) -> String {
    if n == 0 {
        return "0: desktop".to_string();
    }
    if n < 0 {
        return "?".to_string();
    }
    let name = names.get(&(n as u8)).map(String::as_str).unwrap_or("(unnamed)");
    format!("{n}: {name}")
}

/// Value colour: red on an unreadable value, yellow while being edited,
/// default otherwise.
fn value_style(editing: bool, error: bool) -> Style {
    if error {
        Style::default().fg(Color::Red)
    } else if editing {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    }
}

/// Colour the status line by whether it reads as an error or a success.
fn status_colour(s: &str) -> Color {
    let l = s.to_lowercase();
    if l.is_empty() {
        Color::Reset
    } else if l.contains("error")
        || l.contains("denied")
        || l.contains("needs")
        || l.contains("fail")
        || l.contains("unavailable")
    {
        Color::Red
    } else {
        Color::Green
    }
}
