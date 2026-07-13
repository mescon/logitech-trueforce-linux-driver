use crate::app::App;
use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::{Category, Device};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

pub fn draw<S: SysfsIo>(f: &mut Frame, app: &App<S>) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1), Constraint::Length(2)])
        .split(f.area());

    // header: device + mode
    let info = app.device.info().ok();
    let header = match &info {
        Some(i) => format!(" logi-dd   serial {}   fw {}   mode: {:?}", i.serial, i.firmware, i.mode),
        None => " logi-dd   (no wheel)".to_string(),
    };
    f.render_widget(Paragraph::new(header).block(Block::default().borders(Borders::ALL)), root[0]);

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
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(c.label()).style(style)
        })
        .collect();
    f.render_widget(List::new(cats).block(Block::default().borders(Borders::ALL).title("Category")), body[0]);

    // settings in the selected category
    let rows: Vec<ListItem> = app
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let spec = Device::<S>::spec(row.attr);
            let val = match (&row.value, spec) {
                _ if !row.available => "(not on this wheel)".to_string(),
                (Ok(v), Some(s)) => {
                    if let Some(ed) = &app.edit {
                        if i == app.row_idx {
                            s.kind.display(&ed.draft)
                        } else {
                            s.kind.display(v)
                        }
                    } else {
                        s.kind.display(v)
                    }
                }
                (Err(e), _) => format!("<{e}>"),
                _ => "?".to_string(),
            };
            let mut style = Style::default();
            if !row.available {
                style = style.add_modifier(Modifier::DIM);
            }
            if i == app.row_idx {
                style = style.add_modifier(Modifier::REVERSED);
            }
            ListItem::new(Line::from(format!("{:<24} {}", row.label, val))).style(style)
        })
        .collect();
    f.render_widget(List::new(rows).block(Block::default().borders(Borders::ALL).title("Settings")), body[1]);

    // status / help
    let help = if app.edit.is_some() {
        "editing:  <-/->  adjust   type  text   Enter  commit   Esc  cancel"
    } else {
        "up/down  select    <-/->  category    Enter  edit    d  desktop mode    r  refresh    q  quit"
    };
    let status = format!("{}\n{}", app.status, help);
    f.render_widget(Paragraph::new(status), root[2]);
}
