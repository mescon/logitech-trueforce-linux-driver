use crate::app::App;
use crate::curve_editor::CurveEditor;
use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::{shaping, Category, Device, Mode, Value};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
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
                // Serial and firmware live in the Info category, not the
                // header. Keep the header to the app name and current mode.
                Span::raw("   mode: "),
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

    // categories, plus a trailing synthetic "Setup" entry (index
    // `Category::ALL.len()`, i.e. `app::SETUP_INDEX`) that is not a real
    // `Category`: it shows the game helpers (logi-ffb, the TrueForce SDK
    // shim) instead of a settings list.
    let mut cats: Vec<ListItem> = Category::ALL
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
    cats.push(ListItem::new("Setup").style(if app.is_setup() {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    }));
    f.render_widget(
        List::new(cats).block(Block::default().borders(Borders::ALL).title("Category")),
        body[0],
    );

    if app.is_setup() {
        draw_setup(f, app, body[1]);
    } else {
        // settings in the selected category
        let names = app.profile_names();
        let mut rows: Vec<ListItem> = app
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let spec = Device::<S>::spec(row.attr);
                // the edit state, only for the row being edited
                let editing = app.edit.as_ref().filter(|_| i == app.row_idx);

                let (val, mut val_style) = if !row.available {
                    ("(not on this wheel)".to_string(), Style::default().fg(Color::DarkGray))
                } else if row.attr == shaping::TOGGLE_ATTR {
                    // The synthetic view toggle (no registry spec): show
                    // which shaping controls the page currently offers.
                    let advanced = matches!(row.value, Ok(Value::Bool(true)));
                    (
                        (if advanced { "on (curves)" } else { "off (sensitivity)" }).to_string(),
                        value_style(false, false),
                    )
                } else if row.attr == "wheel_profile" {
                    // show the profile number with its onboard name
                    let n = match (editing.map(|e| &e.draft), &row.value) {
                        (Some(Value::Int(n)), _) => *n,
                        (_, Ok(Value::Int(n))) => *n,
                        _ => -1,
                    };
                    (profile_label(n, &names), value_style(editing.is_some(), false))
                } else if row.attr == "wheel_led_effect" {
                    // The LIGHTSYNC effect selector: show the current (or
                    // the cycled, while its modal is active) entry's label
                    // instead of the raw 1-9 number.
                    let cycling = app.effect_edit.as_ref().filter(|_| i == app.row_idx);
                    let text = match cycling {
                        Some(fe) => {
                            fe.labels.get(fe.index).cloned().unwrap_or_else(|| "?".to_string())
                        }
                        None => app.lightsync_effect_label(),
                    };
                    let mut style = value_style(cycling.is_some(), false);
                    if cycling.is_some() {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    (text, style)
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
        // On the Info category, append the project link so users know where
        // to find docs and source (a terminal cannot open it, but it is
        // copyable).
        if app.category() == Category::Info {
            rows.push(ListItem::new(Line::from(vec![
                Span::styled(format!("{:<24}", "Documentation"), Style::default().fg(Color::Gray)),
                Span::raw(" "),
                Span::styled(logi_dd_core::PROJECT_URL, Style::default().fg(Color::Cyan)),
            ])));
        }
        f.render_widget(
            List::new(rows).block(Block::default().borders(Borders::ALL).title("Settings")),
            body[1],
        );
    }

    // The curve editor takes over the body area as a modal when active.
    if let Some(ce) = &app.curve_edit {
        draw_curve_editor(f, ce, root[1]);
    }

    // status line (green on success, red on trouble) + a dim help line
    let help = if app.curve_edit.is_some() {
        "curve:  up/down field   <-/-> adjust   + add point   - delete   Enter save   Esc cancel"
    } else if app.effect_edit.is_some() {
        "effect:  <-/->  choose    Enter  apply    Esc  cancel"
    } else if app.edit.is_some() {
        "editing:  <-/->  adjust    type  text    Enter  commit    Esc  cancel"
    } else if app.is_setup() {
        if app.sdk_edit.is_some() {
            "SDK folder:  type path   Backspace erase   Enter save   Esc cancel"
        } else {
            "up/down select game   i install shim   u remove shim   r rescan   s SDK folder   <-/-> category   q quit"
        }
    } else if app.selected().map(|r| r.attr) == Some(shaping::TOGGLE_ATTR) {
        // The toggle row's help explains why the page shows only one of
        // the two shaping controls, same text the GUI row carries.
        shaping::TOGGLE_HELP
    } else if app.has_shaping_toggle() {
        "up/down select   <-/-> category   Enter edit   a advanced shaping   d desktop/onboard   r refresh   q quit"
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

/// Render the Setup body: the logi-ffb helper, the SDK folder line (edited
/// via `s`; see `App::sdk_edit`), the per-game shim manager (the selectable
/// Proton games list), and the static compatibility table at the bottom.
/// Shown instead of the settings list whenever `app.is_setup()`, mirroring
/// the GUI's Setup page in text form.
fn draw_setup<S: SysfsIo>(f: &mut Frame, app: &App<S>, area: Rect) {
    let found_style = |found: bool| {
        if found {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Red)
        }
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(11), Constraint::Min(3), Constraint::Length(12)])
        .split(area);

    // Top: logi-ffb + the SDK folder line (with the libtrueforce note).
    let shim_found = app.shim_binary.is_some();
    let sdk_line = match &app.sdk_edit {
        Some(draft) => Line::from(vec![
            Span::raw("SDK folder: "),
            Span::styled(format!("{draft}_"), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
        None => Line::from(vec![
            Span::raw("SDK folder: "),
            Span::raw(app.sdk_dir.clone()),
            Span::raw("  "),
            Span::styled(
                if app.sdk_valid { "SDK DLLs found" } else { "trueforce_sdk_x64.dll not found" },
                found_style(app.sdk_valid),
            ),
        ]),
    };
    let top = vec![
        Line::from(Span::styled(
            "Force feedback in games (logi-ffb)",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
        Line::from(
            "DirectInput sims run through Proton (for example Le Mans Ultimate) get no \
             force feedback by default. Running the game through logi-ffb gives them FFB \
             via a virtual wheel.",
        ),
        Line::from(vec![
            Span::raw("logi-ffb: "),
            Span::styled(
                if app.ffb_found { "found on PATH" } else { "not found on PATH" },
                found_style(app.ffb_found),
            ),
            Span::raw("    launch options: "),
            Span::styled("logi-ffb %command%", Style::default().fg(Color::Yellow)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "TrueForce SDK shim",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::raw("Installer: "),
            Span::styled(
                if shim_found { "found on PATH" } else { "not found on PATH" },
                found_style(shim_found),
            ),
        ]),
        sdk_line,
        Line::from(
            "Native Linux apps can drive TrueForce through this repo's libtrueforce \
             library. The SDK DLLs come from Logitech's G HUB on Windows and are never \
             redistributed; see the project README for how to copy them.",
        ),
    ];
    f.render_widget(
        Paragraph::new(top)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title("Setup")),
        rows[0],
    );

    // Middle: the installed Proton games, one selectable row each.
    let games: Vec<ListItem> = if app.games.is_empty() {
        vec![ListItem::new(if app.games_scanned {
            "No Steam installation with Proton games found (r to rescan)"
        } else {
            "Scanning Steam libraries..."
        })
        .style(Style::default().fg(Color::DarkGray))]
    } else {
        app.games
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let status = if g.shim_installed {
                    Span::styled("shim installed", Style::default().fg(Color::Green))
                } else {
                    Span::styled("-", Style::default().fg(Color::DarkGray))
                };
                let mut item = ListItem::new(Line::from(vec![
                    Span::raw(format!("{:<40}", g.name)),
                    Span::raw(" "),
                    status,
                ]));
                if i == app.game_idx {
                    item = item.style(Style::default().add_modifier(Modifier::REVERSED));
                }
                item
            })
            .collect()
    };
    f.render_widget(
        List::new(games).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Proton games (i install shim, u remove shim)"),
        ),
        rows[1],
    );

    // Bottom: the static compatibility table.
    let compat_rows: [(&str, &str); 8] = [
        ("ACC", "TrueForce (shim)"),
        ("AC EVO", "TrueForce (shim)"),
        ("iRacing", "FFB (native)"),
        ("Le Mans Ultimate", "FFB (logi-ffb)"),
        ("Automobilista 2", "FFB (logi-ffb)"),
        ("rFactor 2", "FFB (logi-ffb)"),
        ("Dirt Rally 2.0", "FFB (native)"),
        ("BeamNG.drive", "FFB (native)"),
    ];
    let mut compat = vec![Line::from(Span::styled(
        "\"FFB (logi-ffb)\" means launch with logi-ffb %command%.",
        Style::default().fg(Color::DarkGray),
    ))];
    compat.extend(compat_rows.iter().map(|(game, how)| {
        Line::from(vec![
            Span::raw(format!("{game:<20}")),
            Span::styled(*how, Style::default().fg(Color::Gray)),
        ])
    }));
    f.render_widget(
        Paragraph::new(compat)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title("Game compatibility")),
        rows[2],
    );
}

/// Render the modal curve editor over `area`: a left field panel and a right
/// live ASCII preview of the composed curve.
fn draw_curve_editor(f: &mut Frame, ce: &CurveEditor, area: Rect) {
    f.render_widget(Clear, area);
    let title = format!(" Curve editor: {} ", ce.attr.replace("wheel_", ""));
    let outer = Block::default().borders(Borders::ALL).title(title);
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(30), Constraint::Min(10)])
        .split(inner);

    // Left: the editable fields, selected one highlighted.
    let mut lines: Vec<Line> = CurveEditor::FIELDS
        .iter()
        .map(|fld| {
            let selected = *fld == ce.field;
            let marker = if selected { "> " } else { "  " };
            let style = if selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            Line::from(vec![
                Span::styled(format!("{marker}{:<16}", fld.label()), style),
                Span::styled(ce.value_of(*fld), style),
            ])
        })
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "+ add point   - delete",
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(Paragraph::new(lines), cols[0]);

    // Right: the ASCII curve, bordered, with 0%/100% guides.
    let plot = Block::default().borders(Borders::ALL).title("output vs input");
    let pinner = plot.inner(cols[1]);
    f.render_widget(plot, cols[1]);
    let (w, h) = (pinner.width as usize, pinner.height as usize);
    if w >= 4 && h >= 2 {
        let rows = ce.render(w, h);
        let text: Vec<Line> = rows
            .into_iter()
            .map(|r| Line::from(Span::styled(r, Style::default().fg(Color::Cyan))))
            .collect();
        f.render_widget(Paragraph::new(text), pinner);
    }
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
