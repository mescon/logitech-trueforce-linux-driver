use crate::app::{App, Focus};
use crate::curve_editor::CurveEditor;
use logi_dd_core::sysfs::SysfsIo;
use logi_dd_core::{shaping, Category, Device, Mode, Value};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Widget, Wrap};
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
    // shim) instead of a settings list. Every entry wears its digit-jump
    // number: pressing that digit lands there from anywhere.
    let mut cats: Vec<ListItem> = Category::ALL
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let style = if i == app.cat_idx {
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            ListItem::new(format!("{} {}", i + 1, c.label())).style(style)
        })
        .collect();
    cats.push(
        ListItem::new(format!("{} Setup", crate::app::SETUP_INDEX + 1)).style(if app.is_setup() {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        }),
    );
    f.render_widget(
        List::new(cats).block(pane_block("Category", app.focus == Focus::Sidebar)),
        body[0],
    );

    // The scroll keys clamp against what the last draw could show.
    app.body_height.set(body[1].height);

    if app.is_setup() {
        // The two composed views (Setup, Info/Testing) render more than a
        // small terminal fits, so they go through the scrolled window.
        draw_scrolled(f, body[1], setup_content_height(app), app.setup_scroll, |buf, rect| {
            draw_setup(buf, app, rect);
        });
    } else if app.is_info() {
        draw_scrolled(f, body[1], info_content_height(app), app.info_scroll, |buf, rect| {
            if app.no_wheel {
                // No wheel: the whole body is the monitor's empty state (an
                // evdev-only wheel input may still exist and rescan finds it).
                draw_monitor(buf, app, rect);
            } else {
                // The Info page: the identity rows (plus the doc link) on
                // top, the live input monitor below them.
                let rows_height = settings_height(app);
                let split = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(rows_height), Constraint::Min(3)])
                    .split(rect);
                draw_settings(buf, app, split[0]);
                draw_monitor(buf, app, split[1]);
            }
        });
    } else {
        draw_settings(f.buffer_mut(), app, body[1]);
    }

    // The curve editor takes over the body area as a modal when active.
    if let Some(ce) = &app.curve_edit {
        draw_curve_editor(f, ce, root[1]);
    }

    // The LED color picker floats centered over the body when active.
    if let Some(picker) = &app.color_picker {
        draw_color_picker(f, picker, root[1]);
    }

    // The `i` info popup floats centered over the body; any key closes it.
    if let Some(popup) = &app.info_popup {
        draw_info_popup(f, popup, root[1]);
    }

    // The `?` help overlay floats over everything; any key closes it.
    if app.help {
        draw_help(f, app, root[1]);
    }

    draw_status(f, app, root[2]);
}

/// The height the settings list wants: one line per row (plus the extra
/// lines a multi-line value renders), the Info view's App/Driver version
/// rows and doc-link line, and the block's two border lines. Used to
/// split the Info page between the identity rows and the live monitor.
fn settings_height<S: SysfsIo>(app: &App<S>) -> u16 {
    let mut lines = app.rows.len() + 3; // + App/Driver rows + doc link
    for row in &app.rows {
        if let Ok(Value::Text(s)) = &row.value {
            lines += s.matches('\n').count();
        }
    }
    (lines + 2).min(u16::MAX as usize) as u16
}

/// The Setup view's full content height in lines: the section lines plus
/// the block's two borders. Derived from the same builder that draws, so
/// the scroll offset always clamps to exactly what is drawn.
pub(crate) fn setup_content_height<S: SysfsIo>(app: &App<S>) -> u16 {
    (setup_sections(app).0.len() as u16).saturating_add(2)
}

/// Each Setup section's first content line (its header), in content
/// coordinates: what the section cursor scrolls to.
pub(crate) fn setup_section_starts<S: SysfsIo>(
    app: &App<S>,
) -> [u16; crate::app::SetupSection::ALL.len()] {
    setup_sections(app).1
}

/// The Info/Testing view's full content height in lines: the identity
/// rows above the live monitor (the monitor alone in the no-wheel
/// state). Mirrors `draw`'s Info split and `draw_monitor`'s layouts.
pub(crate) fn info_content_height<S: SysfsIo>(app: &App<S>) -> u16 {
    let monitor = match &app.test.dev {
        // The empty state: 5 text lines plus the block's two borders.
        None => 7,
        // The gauges block (13) plus the button tester: the recent-press
        // line, one line per wheel button, and two borders.
        Some(_) => 13 + 3 + logi_dd_core::evtest::WHEEL_BUTTONS.len() as u16,
    };
    if app.no_wheel {
        monitor
    } else {
        settings_height(app).saturating_add(monitor)
    }
}

/// Render a composed view that may be taller than its viewport: `render`
/// draws the full `content_height` into an off-screen buffer and the
/// window at `scroll` is copied into the frame; content that fits renders
/// straight into the frame instead. While content is clipped, a dim
/// "more above/below" marker (with the boundary line over the total)
/// overlays the corresponding edge; the footer names the scroll keys.
fn draw_scrolled(
    f: &mut Frame,
    area: Rect,
    content_height: u16,
    scroll: u16,
    render: impl FnOnce(&mut Buffer, Rect),
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if content_height <= area.height {
        render(f.buffer_mut(), area);
        return;
    }
    // Clamp here too: the offset is clamped on every key press, but a
    // resize (or a content change) can shrink the range under it.
    let scroll = scroll.min(content_height - area.height);
    let virt = Rect::new(area.x, area.y, area.width, content_height);
    let mut buf = Buffer::empty(virt);
    render(&mut buf, virt);
    let dst = f.buffer_mut();
    for y in 0..area.height {
        for x in 0..area.width {
            dst[(area.x + x, area.y + y)] = buf[(area.x + x, area.y + scroll + y)].clone();
        }
    }
    let marker = |dst: &mut Buffer, y: u16, text: String| {
        let w = text.chars().count() as u16;
        if w + 2 <= area.width {
            dst.set_string(
                area.x + area.width - w - 2,
                y,
                text,
                Style::default().fg(Color::DarkGray),
            );
        }
    };
    if scroll > 0 {
        marker(dst, area.y, format!(" more above ({}/{}) ", scroll + 1, content_height));
    }
    if scroll < content_height - area.height {
        marker(
            dst,
            area.y + area.height - 1,
            format!(" more below ({}/{}) ", scroll + area.height, content_height),
        );
    }
}

/// Render the selected category's settings rows (the main body of every
/// device category; on the Info page this is the top block, above the
/// live input monitor). Renders into a `Buffer` rather than the `Frame`,
/// so the Info page can compose it inside `draw_scrolled`'s off-screen
/// pass; the plain categories pass the frame's own buffer.
fn draw_settings<S: SysfsIo>(buf: &mut Buffer, app: &App<S>, area: Rect) {
    // No wheel: a one-line empty state instead of the rows.
    if app.no_wheel {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "(no wheel connected - r to retry)",
                Style::default().fg(Color::Red),
            )),
        ];
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(pane_block("Settings", app.focus == Focus::Content))
            .render(area, buf);
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
                    "  changes apply to the wheel immediately",
                    Style::default().fg(Color::DarkGray),
                ));
                rows.insert(0, ListItem::new(Line::from(spans)));
            }
        }
        // On the Info category, append the software versions (this app,
        // and the loaded kernel module's stamp; `c` prints the same pair
        // on the status line for a manual copy) and the project link so
        // users know where to find docs and source (a terminal cannot
        // open it, but it is copyable).
        if app.category() == Category::Info {
            let display_row = |label: &str, value: String| {
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{label:<24}"), Style::default().fg(Color::Gray)),
                    Span::raw(" "),
                    Span::styled(value, Style::default()),
                ]))
            };
            rows.push(display_row("App", app.app_version_text().to_string()));
            rows.push(display_row("Driver", app.driver_version_text()));
            rows.push(ListItem::new(Line::from(vec![
                Span::styled(format!("{:<24}", "Documentation"), Style::default().fg(Color::Gray)),
                Span::raw(" "),
                Span::styled(logi_dd_core::PROJECT_URL, Style::default().fg(Color::Cyan)),
            ])));
        }
        List::new(rows)
            .block(pane_block("Settings", app.focus == Focus::Content))
            .render(area, buf);
}

/// Render the status line (green on success, red on trouble) + the slim
/// footer (the keymap table's footer-flagged bindings; `?` has the rest).
/// A selected shaping toggle row swaps the footer for its explainer, the
/// same text the GUI rows carry.
fn draw_status<S: SysfsIo>(f: &mut Frame, app: &App<S>, area: Rect) {
    let plain_settings = !app.is_setup() && !app.is_info() && app.edit.is_none();
    let help = if plain_settings
        && app.selected().is_some_and(|r| shaping::toggle_axis(&r.attr).is_some())
    {
        shaping::TOGGLE_HELP.to_string()
    } else {
        crate::keymap::footer(app)
    };
    let lines = vec![
        Line::from(Span::styled(
            app.status.clone(),
            Style::default().fg(status_colour(&app.status)),
        )),
        Line::from(Span::styled(help, Style::default().fg(Color::DarkGray))),
    ];
    f.render_widget(Paragraph::new(lines), area);
}

/// Render the `?` help overlay: the full keymap for the current context
/// plus the globals, straight from `crate::keymap::sections` (the same
/// table the footer renders from). Any key closes it.
fn draw_help<S: SysfsIo>(f: &mut Frame, app: &App<S>, area: Rect) {
    let sections = crate::keymap::sections(app);
    let mut lines: Vec<Line> = Vec::new();
    for (i, section) in sections.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            section.title.to_string(),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
        for b in &section.bindings {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<14}", b.keys), Style::default().fg(Color::Yellow)),
                Span::raw(b.action.to_string()),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "any key closes",
        Style::default().fg(Color::DarkGray),
    )));
    let width = area.width.saturating_sub(6).clamp(20, 64).min(area.width);
    let height = (lines.len() as u16).saturating_add(2).min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" Keys ")),
        rect,
    );
}

/// One Setup section's header line: the cursor marker, the numbered
/// label (reverse-video while selected) and, while selected, its key
/// hints, so every action is discoverable right where it applies.
fn setup_header(label: &str, selected: bool, hint: &str) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{}{}", if selected { "> " } else { "  " }, label),
        if selected {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        },
    )];
    if selected && !hint.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(hint.to_string(), Style::default().fg(Color::DarkGray)));
    }
    Line::from(spans)
}

/// The Setup view's sections as flat lines plus each section's first line
/// (its header) in content coordinates. The selected section expands to
/// its full body; the others render compactly (header + one status line),
/// so the whole page fits typical terminals. One builder feeds the draw,
/// the content height and the section-cursor scrolling, so they can never
/// disagree. Lines stay under ~56 columns (pre-wrapped by hand): the
/// paragraph renders without wrapping so the height stays exact.
fn setup_sections<S: SysfsIo>(
    app: &App<S>,
) -> (Vec<Line<'static>>, [u16; crate::app::SetupSection::ALL.len()]) {
    use crate::app::SetupSection;
    let found_style = |found: bool| {
        if found {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Red)
        }
    };
    let dim = Style::default().fg(Color::DarkGray);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut starts = [0u16; SetupSection::ALL.len()];

    for (i, section) in SetupSection::ALL.iter().enumerate() {
        starts[i] = lines.len() as u16;
        let selected = app.setup_section_idx == i;
        let inside = selected && app.setup_inside;
        match section {
            SetupSection::Ffb => {
                lines.push(setup_header(section.label(), selected, ""));
                let ffb_span = Span::styled(
                    match &app.ffb_path {
                        Some(p) => format!("found: {}", p.display()),
                        None => "not found (PATH or next to logi-dd)".to_string(),
                    },
                    found_style(app.ffb_path.is_some()),
                );
                if selected {
                    for text in [
                        "DirectInput sims run through Proton (for example",
                        "Le Mans Ultimate) get no force feedback by default.",
                        "Running a game through logi-ffb gives it FFB via a",
                        "virtual wheel.",
                    ] {
                        lines.push(Line::from(format!("  {text}")));
                    }
                    lines.push(Line::from(vec![Span::raw("  logi-ffb: "), ffb_span]));
                    lines.push(Line::from(vec![
                        Span::raw("  Steam launch options: "),
                        Span::styled("logi-ffb %command%", Style::default().fg(Color::Yellow)),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::styled("  logi-ffb: ", dim),
                        ffb_span,
                        Span::styled("   launch: logi-ffb %command%", dim),
                    ]));
                }
            }
            SetupSection::Sdk => {
                lines.push(setup_header(
                    section.label(),
                    selected,
                    "[Enter or s edits the SDK folder]",
                ));
                let dlls_span = match &app.sdk_resolved {
                    Some(dir) => Span::styled(
                        format!("SDK DLLs: found at {}", dir.display()),
                        found_style(true),
                    ),
                    None => Span::styled("SDK DLLs: not found", found_style(false)),
                };
                if selected {
                    lines.push(Line::from(vec![
                        Span::raw("  Installer: "),
                        Span::styled(
                            match &app.shim_binary {
                                Some(p) => format!("found: {}", p.display()),
                                None => "not found (PATH or the repo's tools/)".to_string(),
                            },
                            found_style(app.shim_binary.is_some()),
                        ),
                    ]));
                    lines.push(match &app.sdk_edit {
                        Some(draft) => Line::from(vec![
                            Span::raw("  SDK folder: "),
                            Span::styled(
                                format!("{draft}_"),
                                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                            ),
                        ]),
                        None => Line::from(format!("  SDK folder: {}", app.sdk_dir)),
                    });
                    lines.push(Line::from(vec![Span::raw("  "), dlls_span]));
                    for text in [
                        "The DLLs come from Logitech's G HUB on Windows and",
                        "are never redistributed; the README says how to copy",
                        "them. Native Linux apps use libtrueforce instead.",
                    ] {
                        lines.push(Line::from(Span::styled(format!("  {text}"), dim)));
                    }
                } else {
                    lines.push(Line::from(vec![
                        Span::styled("  installer: ", dim),
                        Span::styled(
                            if app.shim_binary.is_some() { "found" } else { "not found" },
                            found_style(app.shim_binary.is_some()),
                        ),
                        Span::styled("   ", dim),
                        dlls_span,
                    ]));
                }
            }
            SetupSection::Games => {
                let hint = if inside {
                    "[i install  u remove  g sim TF  Esc back]"
                } else {
                    "[Enter opens the list]"
                };
                lines.push(setup_header(section.label(), selected, hint));
                if selected {
                    if app.games.is_empty() {
                        lines.push(Line::from(Span::styled(
                            if app.games_scanned {
                                "  No Steam installation with Proton games found (r rescans)"
                            } else {
                                "  Scanning Steam libraries..."
                            },
                            dim,
                        )));
                    }
                    for (g_idx, g) in app.games.iter().enumerate() {
                        let cursor = inside && g_idx == app.game_idx;
                        let name_style = if cursor {
                            Style::default().add_modifier(Modifier::REVERSED)
                        } else {
                            Style::default()
                        };
                        let status = if g.shim_installed {
                            Span::styled("shim installed", Style::default().fg(Color::Green))
                        } else {
                            Span::styled("-", dim)
                        };
                        // Games the tf-sim daemon can identify show their
                        // live per-game state (g toggles it); others show
                        // nothing.
                        let sim = match logi_dd_core::tfsim::game_id_for_title(&g.name) {
                            Some(id) => {
                                let game = app.tf_cfg.game(id);
                                if game.enabled {
                                    Span::styled(
                                        format!("  sim TF: on {}%", game.intensity),
                                        Style::default().fg(Color::Green),
                                    )
                                } else {
                                    Span::styled("  sim TF: off".to_string(), dim)
                                }
                            }
                            None => Span::raw(""),
                        };
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("  {}{:<34}", if cursor { "> " } else { "  " }, g.name),
                                name_style,
                            ),
                            Span::raw(" "),
                            status,
                            sim,
                        ]));
                    }
                } else {
                    let with_shim = app.games.iter().filter(|g| g.shim_installed).count();
                    lines.push(Line::from(Span::styled(
                        if !app.games_scanned {
                            "  not scanned yet".to_string()
                        } else if app.games.is_empty() {
                            "  none found (r rescans)".to_string()
                        } else {
                            format!("  {} game(s), {} with the shim", app.games.len(), with_shim)
                        },
                        dim,
                    )));
                }
            }
            SetupSection::Compat => {
                lines.push(setup_header(section.label(), selected, ""));
                // "*" marks titles not verified on this driver yet; the
                // last column is the per-game "Simulated TF" state (live
                // where the daemon can identify the title, "planned" for
                // the other FFB-only titles, "native TF" where the shim
                // already delivers the real thing). Mirrors the GUI Setup
                // page's `compat_rows`.
                let compat_rows: [(&str, &str, &str); 14] = [
                    ("ACC", "TrueForce (shim)", "native TF"),
                    ("AC EVO", "TrueForce (shim)", "native TF"),
                    ("iRacing", "FFB (native)", "planned"),
                    ("Le Mans Ultimate", "FFB (logi-ffb)", "planned"),
                    ("Automobilista 2", "FFB (logi-ffb)", "planned"),
                    ("rFactor 2", "FFB (logi-ffb)", "planned"),
                    ("Assetto Corsa", "FFB (logi-ffb) *", "planned"),
                    ("Project CARS 2", "FFB (logi-ffb) *", "planned"),
                    ("Dirt Rally 2.0", "FFB (native)", "planned"),
                    ("EA SPORTS WRC", "FFB *", "planned"),
                    ("F1 series", "FFB *", "planned"),
                    ("Euro Truck Simulator 2", "FFB (native Linux)", "planned"),
                    ("American Truck Simulator", "FFB (native Linux)", "planned"),
                    ("BeamNG.drive", "FFB (native)", "planned"),
                ];
                if selected {
                    for text in [
                        "FFB (logi-ffb) = launch with logi-ffb %command%.",
                        "Simulated TF = telemetry haptics via logi-tf-sim,",
                        "live values where the daemon knows the title.",
                        "native TF = the shim delivers real TrueForce.",
                        "* = expected to work, not verified on this driver.",
                    ] {
                        lines.push(Line::from(Span::styled(format!("  {text}"), dim)));
                    }
                    lines.push(Line::from(Span::styled(
                        format!("  {:<25}{:<19}{}", "Game", "Force feedback", "Simulated TF"),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                    )));
                    for (game, how, tf) in compat_rows {
                        // Titles with a tf-sim game id show that game's
                        // live tf-sim.conf state instead of the static text
                        // (AMS2 and Project CARS 2 share one id, so they
                        // always agree).
                        let cell = match logi_dd_core::tfsim::game_id_for_title(game) {
                            Some(id) => {
                                let sim = app.tf_cfg.game(id);
                                if sim.enabled {
                                    Span::styled(
                                        format!("on {}%", sim.intensity),
                                        Style::default().fg(Color::Green),
                                    )
                                } else {
                                    Span::styled("off".to_string(), dim)
                                }
                            }
                            None => Span::styled(tf.to_string(), dim),
                        };
                        lines.push(Line::from(vec![
                            Span::raw(format!("  {game:<25}")),
                            Span::styled(format!("{how:<19}"), Style::default().fg(Color::Gray)),
                            cell,
                        ]));
                    }
                } else {
                    lines.push(Line::from(Span::styled(
                        format!("  {} known titles", compat_rows.len()),
                        dim,
                    )));
                }
            }
            SetupSection::SimTf => {
                let hint = if inside {
                    "[m master  e intensity  p pitch  d daemon  t sweep  Esc back]"
                } else {
                    "[Enter opens the controls]"
                };
                lines.push(setup_header(section.label(), selected, hint));
                let master_span = Span::styled(
                    if app.tf_cfg.enabled { "on" } else { "off" },
                    found_style(app.tf_cfg.enabled),
                );
                let daemon_span = if app.tf_daemon {
                    Span::styled("running", Style::default().fg(Color::Green))
                } else {
                    Span::styled("stopped", dim)
                };
                if selected {
                    for text in [
                        "Synthesizes TrueForce engine haptics from a game's",
                        "UDP telemetry, for titles without native TrueForce.",
                    ] {
                        lines.push(Line::from(format!("  {text}")));
                    }
                    lines.push(Line::from(vec![
                        Span::raw("  logi-tf-sim: "),
                        Span::styled(
                            match &app.tf_bin {
                                Some(p) => format!("found: {}", p.display()),
                                None => "not found (PATH or next to logi-dd)".to_string(),
                            },
                            found_style(app.tf_bin.is_some()),
                        ),
                        Span::raw("   daemon: "),
                        daemon_span,
                    ]));
                    // Whichever value editor is active shows as its yellow
                    // draft.
                    let draft_or = |draft: &Option<String>, value: String| match draft {
                        Some(d) => Span::styled(
                            format!("{d}_"),
                            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                        ),
                        None => Span::raw(value),
                    };
                    lines.push(Line::from(vec![
                        Span::raw("  master: "),
                        master_span,
                        Span::raw("   intensity: "),
                        draft_or(&app.tf_intensity_edit, format!("{}%", app.tf_cfg.intensity)),
                        Span::raw("   pitch: "),
                        draft_or(&app.tf_pitch_edit, format!("{}%", app.tf_cfg.pitch_pct)),
                    ]));
                    lines.push(Line::from(Span::styled(
                        "  pitch = felt rev rate; 100 = crank speed. Per-game",
                        dim,
                    )));
                    lines.push(Line::from(Span::styled(
                        "  switches live in the Proton games list (g).",
                        dim,
                    )));
                } else {
                    lines.push(Line::from(vec![
                        Span::styled("  master: ", dim),
                        master_span,
                        Span::styled(format!("   intensity: {}%", app.tf_cfg.intensity), dim),
                        Span::styled("   daemon: ", dim),
                        daemon_span,
                    ]));
                }
            }
        }
        if i + 1 < SetupSection::ALL.len() {
            lines.push(Line::from(""));
        }
    }
    (lines, starts)
}

/// Render the Setup body: the sectioned page `setup_sections` builds
/// (logi-ffb, the SDK shim, the Proton games, the compatibility table,
/// Simulated TrueForce). Shown instead of the settings list whenever
/// `app.is_setup()`. Renders into `draw_scrolled`'s buffer: `area` is the
/// view's full content height, not the viewport.
fn draw_setup<S: SysfsIo>(buf: &mut Buffer, app: &App<S>, area: Rect) {
    let (lines, _) = setup_sections(app);
    Paragraph::new(lines)
        .block(pane_block("Setup", app.focus == Focus::Content))
        .render(area, buf);
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
/// Renders into `draw_scrolled`'s buffer, like the other composed views.
fn draw_monitor<S: SysfsIo>(buf: &mut Buffer, app: &App<S>, area: Rect) {
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
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title("Test area"))
            .render(area, buf);
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
    Paragraph::new(top)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title("Test area"))
        .render(rows[0], buf);

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
    List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Buttons (highlighted while held)"))
        .render(rows[1], buf);
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

/// Render the modal LED color picker: the 10 LEDs as truecolor blocks
/// with a cursor on top, the 16-swatch palette grid below (Tab moves the
/// arrows between the two), the live hex preview of what `w` would write,
/// and the key line. The focused half wears the accent marker so the
/// arrows' target is always visible.
fn draw_color_picker(f: &mut Frame, picker: &crate::color_picker::ColorPicker, area: Rect) {
    use crate::color_picker::{PickerFocus, PALETTE, PALETTE_COLS};

    let led_focus = picker.focus == PickerFocus::Leds;
    let focus_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);
    let mut lines: Vec<Line> = Vec::new();

    // The LED strip: 10 numbered, colored blocks; the cursor LED wears
    // brackets (and the row label the accent while the arrows act here).
    lines.push(Line::from(Span::styled(
        "LEDs (left = LED 1)",
        if led_focus { focus_style } else { dim },
    )));
    let mut strip: Vec<Span> = vec![Span::raw("  ")];
    let mut ruler = String::from("  ");
    for (i, c) in picker.colors.iter().enumerate() {
        let block = Span::styled("██", Style::default().fg(Color::Rgb(c.r, c.g, c.b)));
        if i == picker.cursor {
            strip.push(Span::styled("[", focus_style));
            strip.push(block);
            strip.push(Span::styled("]", focus_style));
            ruler.push_str(&format!(" {:<3}", i + 1));
        } else {
            strip.push(Span::raw(" "));
            strip.push(block);
            strip.push(Span::raw(" "));
            ruler.push_str(&format!(" {:<3}", i + 1));
        }
    }
    lines.push(Line::from(strip));
    lines.push(Line::from(Span::styled(ruler, dim)));
    lines.push(Line::from(""));

    // The palette grid: PALETTE_COLS swatches per row, the selected one
    // bracketed; its name prints next to the grid label.
    lines.push(Line::from(vec![
        Span::styled(
            "Palette",
            if led_focus { dim } else { focus_style },
        ),
        Span::raw("  "),
        Span::styled(PALETTE[picker.palette].0, Style::default().add_modifier(Modifier::BOLD)),
    ]));
    for row in PALETTE.chunks(PALETTE_COLS).enumerate() {
        let (row_idx, swatches) = row;
        let mut spans: Vec<Span> = vec![Span::raw("  ")];
        for (col_idx, (_, c)) in swatches.iter().enumerate() {
            let idx = row_idx * PALETTE_COLS + col_idx;
            let block = Span::styled("██", Style::default().fg(Color::Rgb(c.r, c.g, c.b)));
            if idx == picker.palette {
                spans.push(Span::styled("[", focus_style));
                spans.push(block);
                spans.push(Span::styled("]", focus_style));
            } else {
                spans.push(Span::raw(" "));
                spans.push(block);
                spans.push(Span::raw(" "));
            }
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));

    // The hex entry (while open) or the live preview of the exact write.
    match &picker.hex {
        Some(draft) => lines.push(Line::from(vec![
            Span::raw(format!("  LED {} hex: ", picker.cursor + 1)),
            Span::styled(
                format!("{draft}_"),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
        ])),
        // "  w: " + the 69-char strip = 74 columns: exactly the modal's
        // inner width on an 80-column terminal, so no hex value clips.
        None => lines.push(Line::from(vec![
            Span::styled("  w: ", dim),
            Span::styled(picker.preview(), dim),
        ])),
    }
    lines.push(Line::from(Span::styled(
        "  Tab focus  Enter paint  a all  p pair  x hex  w write  Esc cancel",
        dim,
    )));

    let width = area.width.saturating_sub(4).clamp(30, 76).min(area.width);
    let height = (lines.len() as u16).saturating_add(2).min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" LED colors ")),
        rect,
    );
}

/// Render the `i` info popup: a centered, cleared, bordered paragraph over
/// the body (the same Clear + bordered block pattern the curve editor
/// uses), sized to its wrapped content.
fn draw_info_popup(f: &mut Frame, popup: &crate::app::InfoPopup, area: Rect) {
    let width = area.width.saturating_sub(6).clamp(20, 56).min(area.width);
    let inner_w = width.saturating_sub(2).max(1) as usize;
    // Wrapped-height estimate (Paragraph wraps at the inner width), so the
    // popup hugs its content instead of showing empty rows.
    let text_lines: usize = popup
        .lines
        .iter()
        .map(|l| l.chars().count().div_ceil(inner_w).max(1))
        .sum();
    let height = (text_lines as u16).saturating_add(2).min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    f.render_widget(Clear, rect);
    let lines: Vec<Line> = popup.lines.iter().map(|l| Line::from(l.clone())).collect();
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default().borders(Borders::ALL).title(format!(" {} ", popup.title)),
        ),
        rect,
    );
}

/// A pane's bordered block: the focused pane's border wears the accent
/// colour and a bold title, so the pane Up/Down act on is always visible.
fn pane_block(title: &str, focused: bool) -> Block<'static> {
    let block = Block::default().borders(Borders::ALL);
    if focused {
        block
            .border_style(Style::default().fg(Color::Cyan))
            .title(Span::styled(
                title.to_string(),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ))
    } else {
        block.title(title.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SETUP_INDEX;
    use logi_dd_core::sysfs::FakeSysfs;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// The whole test backend buffer as one string, for containment
    /// asserts against what a terminal of that size would show.
    fn screen(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn wheel_app() -> App<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_range", "900");
        App::new(logi_dd_core::Device::with_io(fs))
    }

    /// An app parked on the Setup view without the Steam scan a key-driven
    /// entry would run (the scan reads this machine's real libraries).
    fn setup_view_app() -> App<FakeSysfs> {
        let mut a = wheel_app();
        a.cat_idx = SETUP_INDEX;
        a.games_scanned = true;
        a.reload();
        a
    }

    #[test]
    fn setup_sections_render_compact_and_expand_on_selection() {
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut a = setup_view_app();
        a.focus = Focus::Content;
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        // Every section header is on one screen thanks to the compact
        // rendering; the unselected compatibility table shows only its
        // one-line summary.
        for header in [
            "Force feedback in games (logi-ffb)",
            "TrueForce SDK shim",
            "Proton games",
            "Game compatibility",
            "Simulated TrueForce",
        ] {
            assert!(text.contains(header), "missing header {header}:\n{text}");
        }
        assert!(text.contains("known titles"), "compact compat summary:\n{text}");
        assert!(!text.contains("BeamNG.drive"), "the table body stays collapsed:\n{text}");
        // Selecting the compatibility section expands the full table.
        use crossterm::event::KeyCode;
        for _ in 0..3 {
            a.on_key(KeyCode::Down);
        }
        assert_eq!(a.setup_section(), crate::app::SetupSection::Compat);
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(text.contains("BeamNG.drive"), "the selected section expands:\n{text}");
        assert!(!text.contains("known titles"), "the summary line yields to the body:\n{text}");
    }

    #[test]
    fn small_terminal_flags_the_clipped_setup_view_and_scrolls_to_the_end() {
        // 80x24: with the compatibility table expanded the page cannot
        // fit, so the markers and the scroll fallback must still work.
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut a = setup_view_app();
        a.focus = Focus::Content;
        a.setup_section_idx = crate::app::SetupSection::ALL
            .iter()
            .position(|s| *s == crate::app::SetupSection::Compat)
            .unwrap();
        a.setup_scroll = 0;
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(text.contains("more below"), "clipped content is flagged:\n{text}");
        // Scroll to the bottom: the marker flips and the table's last
        // rows show.
        a.scroll_view(i32::from(a.max_scroll()));
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(text.contains("more above"), "the scrolled state is flagged:\n{text}");
        assert!(!text.contains("more below"), "nothing is clipped below any more:\n{text}");
        assert!(text.contains("BeamNG.drive"), "scrolling reaches the bottom:\n{text}");
    }

    #[test]
    fn a_tall_terminal_needs_no_scroll_marker() {
        let mut term = Terminal::new(TestBackend::new(100, 60)).unwrap();
        let a = setup_view_app();
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(!text.contains("more below") && !text.contains("more above"), "{text}");
        assert!(text.contains("Game compatibility"), "everything fits:\n{text}");
    }

    #[test]
    fn info_view_scrolls_down_to_the_button_tester() {
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut a = wheel_app();
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Info).unwrap();
        a.reload();
        a.test.dev = Some(logi_dd_core::evtest::WheelInput {
            event_path: "/nonexistent/event99".to_string(),
            name: "Logitech RS50 Base".to_string(),
        });
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(text.contains("more below"), "the composed Info view clips at 24 lines:\n{text}");
        a.scroll_view(i32::from(a.max_scroll()));
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        // At the bottom the button tester's rows fill the viewport (its
        // title scrolled past); the last buttons prove the end is reachable.
        assert!(text.contains("G1 (Logo)"), "the button tester becomes reachable:\n{text}");
        assert!(text.contains("more above"), "{text}");
        assert!(!text.contains("more below"), "{text}");
    }
}
