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
    } else if app.is_info() {
        if app.no_wheel {
            // No wheel: the whole body is the monitor's empty state (an
            // evdev-only wheel input may still exist and rescan finds it).
            draw_monitor(f, app, body[1]);
        } else {
            // The Info page: the identity rows (plus the doc link) on top,
            // the live input monitor below them.
            let rows_height = settings_height(app);
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(rows_height), Constraint::Min(3)])
                .split(body[1]);
            draw_settings(f, app, split[0]);
            draw_monitor(f, app, split[1]);
        }
    } else {
        draw_settings(f, app, body[1]);
    }

    // The curve editor takes over the body area as a modal when active.
    if let Some(ce) = &app.curve_edit {
        draw_curve_editor(f, ce, root[1]);
    }

    draw_status(f, app, root[2]);
}

/// The height the settings list wants: one line per row (plus the extra
/// lines a multi-line value renders), the Info doc-link line, and the
/// block's two border lines. Used to split the Info page between the
/// identity rows and the live monitor.
fn settings_height<S: SysfsIo>(app: &App<S>) -> u16 {
    let mut lines = app.rows.len() + 1; // + doc link
    for row in &app.rows {
        if let Ok(Value::Text(s)) = &row.value {
            lines += s.matches('\n').count();
        }
    }
    (lines + 2).min(u16::MAX as usize) as u16
}

/// Render the selected category's settings rows (the main body of every
/// device category; on the Info page this is the top block, above the
/// live input monitor).
fn draw_settings<S: SysfsIo>(f: &mut Frame, app: &App<S>, area: Rect) {
    // No wheel: a one-line empty state instead of the rows.
    if app.no_wheel {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "(no wheel connected - r to retry)",
                Style::default().fg(Color::Red),
            )),
        ];
        f.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title("Settings")),
            area,
        );
        return;
    }
    let names = app.profile_names();
    let mut rows: Vec<ListItem> = app
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let spec = Device::<S>::spec(&row.attr);
                // the edit state, only for the row being edited
                let editing = app.edit.as_ref().filter(|_| i == app.row_idx);

                let (mut val, mut val_style) = if !row.available {
                    ("(not on this wheel)".to_string(), Style::default().fg(Color::DarkGray))
                } else if shaping::toggle_axis(&row.attr).is_some() {
                    // A synthetic per-axis view toggle (no registry spec):
                    // show which shaping control the axis currently offers.
                    let curve = matches!(row.value, Ok(Value::Bool(true)));
                    ((if curve { "curve" } else { "sensitivity" }).to_string(), value_style(false, false))
                } else if row.attr == crate::app::PROFILE_NEW_ATTR {
                    // The desktop Profiles page's Save row: the name
                    // prompt's draft while it is open, the key hint at
                    // rest.
                    match &app.profile_name_edit {
                        Some(draft) => (
                            format!("{draft}_"),
                            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                        ),
                        None => (
                            match &row.value {
                                Ok(Value::Text(s)) => s.clone(),
                                _ => String::new(),
                            },
                            Style::default().fg(Color::DarkGray),
                        ),
                    }
                } else if row.attr.starts_with(crate::app::PROFILE_ROW_PREFIX) {
                    // A saved computer profile: the value column is the
                    // key hint (no registry spec behind these rows).
                    let hint = match &row.value {
                        Ok(Value::Text(s)) => s.clone(),
                        _ => String::new(),
                    };
                    (hint, Style::default().fg(Color::DarkGray))
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

                // A multi-line text value (the firmware's base/motor pair)
                // renders its extra lines indented under the first instead
                // of being collapsed onto one line by `Kind::display`.
                let mut extra: Vec<Line> = Vec::new();
                if row.available && editing.is_none() {
                    if let Ok(Value::Text(s)) = &row.value {
                        if s.contains('\n') {
                            let mut parts = s.lines().map(str::to_string);
                            val = parts.next().unwrap_or_default();
                            extra = parts
                                .map(|p| {
                                    Line::from(vec![
                                        Span::raw(" ".repeat(25)),
                                        Span::styled(p, val_style),
                                    ])
                                })
                                .collect();
                        }
                    }
                }

                let line = Line::from(vec![
                    Span::styled(format!("{:<24}", row.label), Style::default().fg(Color::Gray)),
                    Span::raw(" "),
                    Span::styled(val, val_style),
                ]);
                let mut lines = vec![line];
                lines.extend(extra);
                let mut item = ListItem::new(lines);
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
            area,
        );
}

/// Render the status line (green on success, red on trouble) + a dim
/// context-sensitive help line.
fn draw_status<S: SysfsIo>(f: &mut Frame, app: &App<S>, area: Rect) {
    let help = if app.curve_edit.is_some() {
        "curve:  up/down field   <-/-> adjust   + add point   - delete   Enter save   Esc cancel"
    } else if app.effect_edit.is_some() {
        "effect:  <-/->  choose    Enter  apply    Esc  cancel"
    } else if app.edit.is_some() {
        "editing:  <-/->  adjust    type  text    Enter  commit    Esc  cancel"
    } else if app.profile_name_edit.is_some() {
        "profile name:  type name   Backspace erase   Enter save   Esc cancel"
    } else if app.profile_delete_confirm.is_some() {
        "confirm:  y delete   any other key cancels"
    } else if app.is_setup() {
        if app.sdk_edit.is_some() {
            "SDK folder:  type path   Backspace erase   Enter save   Esc cancel"
        } else {
            "up/down select game   i install shim   u remove shim   r rescan   s SDK folder   <-/-> category   q quit"
        }
    } else if app.is_info() {
        if app.test.confirm.is_some() {
            "confirm:  y continue   any other key cancels"
        } else if app.test.sim_running() {
            "s stop sim   r rescan   d desktop/onboard   <-/-> category   q quit"
        } else {
            "f force feedback sim   t TrueForce texture sim   r rescan   d desktop/onboard   <-/-> category   q quit"
        }
    } else if app.selected().is_some_and(|r| shaping::toggle_axis(&r.attr).is_some()) {
        // A toggle row's help explains why each axis shows only one of
        // the two shaping controls, same text the GUI rows carry.
        shaping::TOGGLE_HELP
    } else if app.no_wheel && !app.is_setup() && !app.is_info() {
        "no wheel connected   r retry discovery   <-/-> category   q quit"
    } else if app.rows.iter().any(|r| r.attr == crate::app::PROFILE_NEW_ATTR) {
        // The desktop Profiles page: the computer-side profile store.
        "up/down select   Enter apply/save   n new profile   d delete profile (or desktop/onboard on Mode)   <-/-> category   q quit"
    } else if app.has_shaping_toggle() {
        "up/down select   <-/-> category   Enter edit   a sensitivity/curve for this axis   d desktop/onboard   r refresh   q quit"
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
    f.render_widget(Paragraph::new(lines), area);
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
                if app.sdk_valid {
                    "SDK DLLs found"
                } else {
                    "no DLLs here; installer will use its own lookup (repo sdk/ or $LOGITECH_TRUEFORCE_SDK_DIR)"
                },
                if app.sdk_valid { found_style(true) } else { Style::default().fg(Color::DarkGray) },
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
                match &app.ffb_path {
                    Some(p) => format!("found: {}", p.display()),
                    None => "not found (PATH or next to logi-dd)".to_string(),
                },
                found_style(app.ffb_path.is_some()),
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
                match &app.shim_binary {
                    Some(p) => format!("found: {}", p.display()),
                    None => "not found (PATH or the repo's tools/)".to_string(),
                },
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

/// A `#`-filled 0..65535 gauge, `width` cells wide.
fn fill_bar(value: i32, width: usize) -> String {
    let filled = (value.clamp(0, 65535) as usize * width) / 65535;
    format!("{}{}", "#".repeat(filled), "-".repeat(width.saturating_sub(filled)))
}

/// A 0..65535 position gauge (for the centered steering axis): a `|`
/// marker on a `-` track, center marked when idle.
fn position_bar(value: i32, width: usize) -> String {
    let width = width.max(3);
    let pos = (value.clamp(0, 65535) as usize * (width - 1)) / 65535;
    (0..width).map(|i| if i == pos { '|' } else { '-' }).collect()
}

/// Render the Info page's live input monitor: the steering/pedal state
/// read off the wheel's evdev node, the light-up button list, and the
/// guarded force-sim status. Mirrors the GUI's Info page in text form.
fn draw_monitor<S: SysfsIo>(f: &mut Frame, app: &App<S>, area: Rect) {
    use logi_dd_core::evtest;

    let t = &app.test;
    let Some(dev) = &t.dev else {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                if t.scanned { "No wheel input found" } else { "Scanning for the wheel..." },
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Connect the wheel to this machine, then press r to rescan."),
            Line::from(
                "The monitor reads the wheel's /dev/input event device, so your user \
                 needs read access to it (the project's udev rule sets this up).",
            ),
        ];
        f.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title("Test area")),
            area,
        );
        return;
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(13), Constraint::Min(3)])
        .split(area);

    let deg = t.degrees();
    let bar_w = (rows[0].width.saturating_sub(14)).clamp(10, 50) as usize;
    let mut top = vec![
        Line::from(vec![
            Span::raw("Device: "),
            Span::styled(dev.name.clone(), Style::default().fg(Color::Cyan)),
            Span::raw(format!("  ({})", dev.event_path)),
        ]),
        Line::from(vec![
            Span::raw("Monitor: "),
            if t.monitoring() {
                Span::styled("live", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
            } else {
                Span::styled("off (r to rescan)", Style::default().fg(Color::Yellow))
            },
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("Steering  "),
            Span::styled(
                format!("{deg:+8.1} deg"),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "   (range {} deg: {} to +{})",
                t.range,
                -(t.range as i32) / 2,
                t.range / 2
            )),
        ]),
        Line::from(format!("          [{}]", position_bar(t.steering_raw, bar_w))),
        Line::from(""),
    ];
    for (label, value) in
        [("Throttle", t.axes[0]), ("Brake", t.axes[1]), ("Clutch", t.axes[2]), ("Handbrake", t.axes[3])]
    {
        top.push(Line::from(vec![
            Span::styled(format!("{label:<9} "), Style::default().fg(Color::Gray)),
            Span::raw(format!("[{}] ", fill_bar(value, bar_w))),
            Span::styled(format!("{value:>5}"), Style::default().fg(Color::Gray)),
        ]));
    }
    top.push(Line::from(vec![
        Span::styled(format!("{:<9} ", "D-pad"), Style::default().fg(Color::Gray)),
        Span::styled(
            evtest::hat_label(t.hat.0, t.hat.1).to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]));
    if t.sim_running() {
        top.push(Line::from(Span::styled(
            "force playing... (25%, 2 s; s to stop)",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        )));
    }
    if let Some(err) = &t.open_error {
        top.push(Line::from(Span::styled(err.clone(), Style::default().fg(Color::Red))));
    }
    f.render_widget(
        Paragraph::new(top)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title("Test area")),
        rows[0],
    );

    // The button tester: every wheel button, reverse-video while held,
    // with the recent-press history on top.
    let recent = if t.recent.is_empty() {
        "-".to_string()
    } else {
        t.recent.iter().map(|c| evtest::button_name(*c)).collect::<Vec<_>>().join(", ")
    };
    let mut items: Vec<ListItem> = vec![ListItem::new(Line::from(vec![
        Span::styled("Last pressed: ", Style::default().fg(Color::Gray)),
        Span::raw(recent),
    ]))];
    items.extend(evtest::WHEEL_BUTTONS.iter().map(|(code, label)| {
        let held = t.pressed.contains(code);
        let mut item = ListItem::new(format!("  {label:<18}"));
        if held {
            item = item.style(Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD));
        }
        item
    }));
    f.render_widget(
        List::new(items).block(
            Block::default().borders(Borders::ALL).title("Buttons (highlighted while held)"),
        ),
        rows[1],
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
        || l.contains("no wheel")
    {
        Color::Red
    } else {
        Color::Green
    }
}
