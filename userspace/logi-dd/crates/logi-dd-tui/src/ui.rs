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

// The chrome uses only the 16 named ANSI colours, so the scheme adapts to
// the user's terminal palette (light or dark). The one exception is the
// LIGHTSYNC strip preview, whose whole point is the exact stored colors:
// it renders `Color::Rgb` blocks (a non-truecolor terminal approximates).

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
        // The LIGHTSYNC view leads with the strip preview: the ACTIVE
        // slot's 10 stored colors as truecolor blocks (LED1 leftmost,
        // mirrored pairs collapsed), plus the try-on-wheel hint. The
        // GUI's animated direction preview has no text-mode counterpart;
        // the hint says where to find it.
        if app.category() == Category::Leds && !app.no_wheel {
            if let Some(colors) = app.led_preview_colors() {
                let mut spans = vec![
                    Span::styled(format!("{:<24}", "Strip preview"), Style::default().fg(Color::Gray)),
                    Span::raw(" "),
                ];
                for c in &colors {
                    spans.push(Span::styled("██", Style::default().fg(Color::Rgb(c.r, c.g, c.b))));
                }
                spans.push(Span::styled(
                    "  t shows it on the wheel (custom slots play a rev sweep)",
                    Style::default().fg(Color::DarkGray),
                ));
                rows.insert(0, ListItem::new(Line::from(spans)));
            }
        }
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
        } else if app.tf_intensity_edit.is_some() {
            "TF intensity (0-100):  type digits   Backspace erase   Enter save   Esc cancel"
        } else if app.tf_pitch_edit.is_some() {
            "TF pitch (10-200; felt rev rate, 100 = crank speed):  type digits   Backspace erase   Enter save   Esc cancel"
        } else if app.tf_sweep_confirm {
            "confirm:  y plays the ~6 s sweep on the wheel   any other key cancels"
        } else if app.tf_sweep_active() {
            "s stop sweep   up/down game   i/u shim   g game sim TF   m TF on/off   e intensity   p pitch   d daemon   r rescan   q quit"
        } else {
            "up/down game   i/u shim   g game sim TF   m TF on/off   e intensity   p pitch   d daemon start/stop   t test sweep   s SDK folder   r rescan   <-/-> category   q quit"
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
    } else if !app.is_setup() && !app.is_info() && app.category() == Category::Leds {
        // No text-mode animation preview; the GUI has the animated one.
        "up/down select   Enter edit   t try lighting on the wheel (custom slots play a rev sweep, built-ins hold 5 s, then restored)   d desktop/onboard   q quit"
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
        // 18 = the 14 compat rows + note + header + the block's 2 borders;
        // 5 = the Simulated TrueForce block's 3 lines + its 2 borders.
        .constraints([
            Constraint::Length(11),
            Constraint::Length(5),
            Constraint::Min(3),
            Constraint::Length(18),
        ])
        .split(area);

    // Top: logi-ffb + the SDK folder line (with the libtrueforce note).
    let shim_found = app.shim_binary.is_some();
    let sdk_line = match &app.sdk_edit {
        Some(draft) => Line::from(vec![
            Span::raw("SDK folder: "),
            Span::styled(format!("{draft}_"), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
        // The concrete resolution outcome next to the field: the resolved
        // path installs actually use (green), or a red not-found line.
        None => Line::from(vec![
            Span::raw("SDK folder: "),
            Span::raw(app.sdk_dir.clone()),
            Span::raw("  "),
            match &app.sdk_resolved {
                Some(dir) => Span::styled(
                    format!("SDK DLLs: found at {}", dir.display()),
                    found_style(true),
                ),
                None => Span::styled(
                    "SDK DLLs: not found - copy them from a Windows G HUB install; see the README",
                    found_style(false),
                ),
            },
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

    // Simulated TrueForce: the daemon block (its per-game cells live in
    // the games list and the compatibility table below).
    let tf = vec![
        Line::from(
            "Synthesizes TrueForce engine haptics from a game's UDP telemetry, for \
             titles without native TrueForce.",
        ),
        // The daemon line: the resolved binary and whether it is running.
        Line::from(vec![
            Span::raw("logi-tf-sim: "),
            Span::styled(
                match &app.tf_bin {
                    Some(p) => format!("found: {}", p.display()),
                    None => "not found (PATH or next to logi-dd)".to_string(),
                },
                found_style(app.tf_bin.is_some()),
            ),
            Span::raw("    daemon: "),
            if app.tf_daemon {
                Span::styled("running", Style::default().fg(Color::Green))
            } else {
                Span::styled("stopped", Style::default().fg(Color::DarkGray))
            },
        ]),
        // The master line, with whichever value editor is active shown as
        // its yellow draft.
        Line::from(vec![
            Span::raw("master: "),
            Span::styled(
                if app.tf_cfg.enabled { "on" } else { "off" },
                found_style(app.tf_cfg.enabled),
            ),
            Span::raw("   intensity: "),
            match &app.tf_intensity_edit {
                Some(draft) => Span::styled(
                    format!("{draft}_"),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                None => Span::raw(format!("{}%", app.tf_cfg.intensity)),
            },
            Span::raw("   pitch (felt rev rate; 100 = crank speed): "),
            match &app.tf_pitch_edit {
                Some(draft) => Span::styled(
                    format!("{draft}_"),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                None => Span::raw(format!("{}%", app.tf_cfg.pitch_pct)),
            },
        ]),
    ];
    f.render_widget(
        Paragraph::new(tf).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Simulated TrueForce (m on/off, e intensity, p pitch, d daemon, t test sweep)"),
        ),
        rows[1],
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
                // Games the tf-sim daemon can identify show their live
                // per-game state (g toggles it); others show nothing.
                let sim = match logi_dd_core::tfsim::game_id_for_title(&g.name) {
                    Some(id) => {
                        let game = app.tf_cfg.game(id);
                        if game.enabled {
                            Span::styled(
                                format!("   sim TF: on {}%", game.intensity),
                                Style::default().fg(Color::Green),
                            )
                        } else {
                            Span::styled("   sim TF: off", Style::default().fg(Color::DarkGray))
                        }
                    }
                    None => Span::raw(""),
                };
                let mut item = ListItem::new(Line::from(vec![
                    Span::raw(format!("{:<40}", g.name)),
                    Span::raw(" "),
                    status,
                    sim,
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
                .title("Proton games (i install shim, u remove shim, g simulated TF)"),
        ),
        rows[2],
    );

    // Bottom: the compatibility table. "expected" marks titles not
    // verified on this driver yet; the third column is the per-game
    // "Simulated TF" state (live where the daemon can identify the title,
    // "planned" for the other FFB-only titles, "n/a (native)" where the
    // shim already delivers real TrueForce). Mirrors the GUI Setup page's
    // `compat_rows`.
    let compat_rows: [(&str, &str, &str); 14] = [
        ("ACC", "TrueForce (shim)", "n/a (native)"),
        ("AC EVO", "TrueForce (shim)", "n/a (native)"),
        ("iRacing", "FFB (native)", "planned"),
        ("Le Mans Ultimate", "FFB (logi-ffb)", "planned"),
        ("Automobilista 2", "FFB (logi-ffb)", "planned"),
        ("rFactor 2", "FFB (logi-ffb)", "planned"),
        ("Assetto Corsa", "FFB (logi-ffb, expected)", "planned"),
        ("Project CARS 2", "FFB (logi-ffb, expected)", "planned"),
        ("Dirt Rally 2.0", "FFB (native)", "planned"),
        ("EA SPORTS WRC", "FFB (expected)", "planned"),
        ("F1 series", "FFB (expected)", "planned"),
        ("Euro Truck Simulator 2", "FFB (native Linux)", "planned"),
        ("American Truck Simulator", "FFB (native Linux)", "planned"),
        ("BeamNG.drive", "FFB (native)", "planned"),
    ];
    let mut compat = vec![
        Line::from(Span::styled(
            "\"FFB (logi-ffb)\" = launch with logi-ffb %command%. \"Simulated TF\" = engine haptics synthesized from telemetry (logi-tf-sim): live per-game values where the daemon can identify the title; n/a (native) titles get real TrueForce via the shim.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            format!("{:<26}{:<26}{}", "Game", "Force feedback", "Simulated TF"),
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
        )),
    ];
    compat.extend(compat_rows.iter().map(|(game, how, tf)| {
        // Titles with a tf-sim game id show that game's live tf-sim.conf
        // state instead of the static text (both AMS2 and Project CARS 2
        // share one id, so they always agree).
        let cell = match logi_dd_core::tfsim::game_id_for_title(game) {
            Some(id) => {
                let sim = app.tf_cfg.game(id);
                if sim.enabled {
                    Span::styled(
                        format!("on {}%", sim.intensity),
                        Style::default().fg(Color::Green),
                    )
                } else {
                    Span::styled("off".to_string(), Style::default().fg(Color::DarkGray))
                }
            }
            None => Span::styled((*tf).to_string(), Style::default().fg(Color::DarkGray)),
        };
        Line::from(vec![
            Span::raw(format!("{game:<26}")),
            Span::styled(format!("{how:<26}"), Style::default().fg(Color::Gray)),
            cell,
        ])
    }));
    f.render_widget(
        Paragraph::new(compat)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title("Game compatibility")),
        rows[3],
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
