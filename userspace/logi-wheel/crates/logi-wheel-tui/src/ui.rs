use crate::app::{App, Focus};
use crate::curve_editor::CurveEditor;
use crate::wheel_test::TestView;
use logi_wheel_core::fftest::StepState;
use logi_wheel_core::sysfs::SysfsIo;
use logi_wheel_core::{shaping, Category, Device, Mode, Value};
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

    // header: device identity + current mode (mode coloured green/yellow),
    // from the cached snapshot `reload()` keeps fresh, not a fresh sysfs
    // read every draw (this runs at ~30 Hz while the live monitor polls).
    let info = app.info();
    let header = match &info {
        Some(i) => {
            let (mode_str, mode_col) = match i.mode {
                Mode::Desktop => ("desktop", Color::Green),
                Mode::Onboard => ("onboard", Color::Yellow),
            };
            Line::from(vec![
                Span::styled(
                    " logi-wheel ",
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                // The rev-light arc, the project mark, in true color.
                Span::styled("\u{2584}\u{2584}", Style::default().fg(Color::Rgb(0x2f, 0xd0, 0x5a))),
                Span::styled("\u{2584}\u{2584}", Style::default().fg(Color::Rgb(0xf5, 0xc5, 0x18))),
                Span::styled("\u{2584}", Style::default().fg(Color::Rgb(0xff, 0x8c, 0x1a))),
                Span::styled("\u{2584}", Style::default().fg(Color::Rgb(0xff, 0x3b, 0x30))),
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
            " logi-wheel   no wheel found",
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
    // A category the connected device has nothing to show for (e.g. a
    // G923 has no LIGHTSYNC/Profiles rows at all) is left out entirely
    // rather than listed as an empty page; its digit (i + 1) simply is not
    // shown, matching `nav_key`'s digit-jump and `move_cat`'s stepping,
    // which both skip it the same way.
    let mut cats: Vec<ListItem> = Category::ALL
        .iter()
        .enumerate()
        .filter(|(_, c)| app.category_applicable(**c))
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
            // The Info page: the identity rows (plus the doc link) on top,
            // the live input monitor below them. The identity block shows
            // regardless of `no_wheel` (see `draw_settings`): a "No wheel
            // detected" Wheel row plus the software versions, none of which
            // need a wheel; the monitor below independently shows its own
            // evdev-availability state (an evdev-only wheel input may still
            // exist and rescan finds it, even with no sysfs wheel).
            let rows_height = settings_height(app);
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(rows_height), Constraint::Min(3)])
                .split(rect);
            draw_settings(buf, app, split[0]);
            draw_monitor(buf, app, split[1]);
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

    // The "Add a game" picker floats centered over the body when active.
    if let Some(picker) = &app.add_game {
        draw_add_game_picker(f, app, picker, root[1]);
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
    if app.category() == Category::Info {
        lines += 1; // the Wheel row every wheel (or "no wheel detected") gets
        if app.device.model() == logi_wheel_core::WheelModel::G923 {
            lines += 2; // the synthetic Serial + Firmware rows
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
        // The gauges block, the button tester (the recent-press line, one
        // line per wheel button, and two borders - the button count is
        // model-aware, see `evtest::button_codes_for_model`), and the
        // "Force feedback test" panel below it - must match whatever
        // `draw_monitor` actually renders.
        Some(_) => {
            monitor_top_height(&app.test)
                + 3
                + logi_wheel_core::evtest::button_codes_for_model(app.device.model()).len() as u16
                + test_plan_height(&app.test)
        }
    };
    // The identity block renders regardless of `no_wheel` now (see
    // `draw_settings`), so its height always counts here too.
    settings_height(app).saturating_add(monitor)
}

/// `draw_monitor`'s top block (device/monitor/steering/pedals/D-pad):
/// eleven content lines, one more while `open_error` is set (the block's
/// only remaining variable line now that the sim countdown and status
/// live in their own "Force feedback test" panel below), plus the
/// block's two borders.
fn monitor_top_height(t: &TestView) -> u16 {
    11 + u16::from(t.open_error.is_some()) + 2
}

/// The "Force feedback test" panel's height: a single hint line before
/// anything is confirmed, or a header line plus one row per step in the
/// confirmed kind's table (kept on screen after the run ends - see
/// `TestView::sim_kind`'s doc comment - so this does not shrink back down
/// once a sequence has played), plus the block's two borders. Derived
/// from the same data `draw_test_plan` renders, so scrolling always
/// clamps to exactly what is drawn (same pattern as
/// `setup_content_height`).
fn test_plan_height(t: &TestView) -> u16 {
    let lines = match t.sim_kind {
        None => 1,
        Some(kind) => 1 + kind.steps().len() as u16,
    };
    lines + 2
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
    // A scrollbar-like column down the right edge, so it is obvious at a
    // glance that the view scrolls: a dim track with a brighter thumb whose
    // size and position track the visible window over the whole content.
    if area.width >= 2 {
        let col = area.x + area.width - 1;
        let track = Style::default().fg(Color::DarkGray);
        let thumb = Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD);
        let h = area.height;
        let thumb_h = ((h as u32 * h as u32) / content_height as u32).max(1) as u16;
        let travel = h - thumb_h.min(h);
        let thumb_top = if content_height > h {
            (scroll as u32 * travel as u32 / (content_height - h) as u32) as u16
        } else {
            0
        };
        for y in 0..h {
            let on_thumb = y >= thumb_top && y < thumb_top + thumb_h;
            dst.set_string(col, area.y + y, "\u{2502}", track);
            if on_thumb {
                dst.set_string(col, area.y + y, "\u{2588}", thumb);
            }
        }
    }
}

/// The no-wheel empty state: the first thing actually wrong, why it stops
/// the wheel working, and the one command that fixes it, then the full list
/// of checks.
///
/// Severity is spelled out (`FAILED`, `WARN`, `ok`) rather than carried by
/// colour alone, so the list reads the same to anyone who cannot separate
/// the two hues, and in a terminal with colour turned off.
///
/// Falls back to the old one-liner when no diagnosis has been made, which is
/// the case under test: the checks read the real `/sys`, so they are only
/// ever run by the real loop in `main`.
fn no_wheel_lines<S: SysfsIo>(app: &App<S>) -> Vec<Line<'static>> {
    use logi_wheel_core::diagnose::{copyable, Severity};

    let mut lines = vec![Line::from("")];
    let Some(problem) = app.diagnosis.iter().find(|f| f.severity != Severity::Ok) else {
        lines.push(Line::from(Span::styled(
            "(no wheel connected - r to retry)",
            Style::default().fg(Color::Red),
        )));
        return lines;
    };

    lines.push(Line::from(Span::styled(
        problem.title.clone(),
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(problem.detail.clone()));
    if let Some(fix) = &problem.fix {
        lines.push(Line::from(""));
        lines.push(Line::from("Run this to fix it:"));
        lines.push(Line::from(Span::styled(
            format!("  {}", copyable(fix)),
            Style::default().add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("Checks:"));
    for finding in &app.diagnosis {
        let (mark, style) = match finding.severity {
            Severity::Ok => ("ok    ", Style::default().fg(Color::Green)),
            Severity::Warning => ("WARN  ", Style::default().fg(Color::Yellow)),
            Severity::Blocking => ("FAILED", Style::default().fg(Color::Red)),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {mark} "), style),
            Span::raw(finding.title.clone()),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("r retries discovery."));
    lines
}

/// Render the selected category's settings rows (the main body of every
/// device category; on the Info page this is the top block, above the
/// live input monitor). Renders into a `Buffer` rather than the `Frame`,
/// so the Info page can compose it inside `draw_scrolled`'s off-screen
/// pass; the plain categories pass the frame's own buffer.
fn draw_settings<S: SysfsIo>(buf: &mut Buffer, app: &App<S>, area: Rect) {
    // No wheel: a one-line empty state instead of the rows, EXCEPT on the
    // Info page, whose identity block (the Wheel row's own "No wheel
    // detected", plus App/Driver/Documentation, none of which need a
    // wheel) is exactly what Request 2 wants visible even with nothing
    // connected: the app should open showing what was detected, or that
    // nothing was, not hide that behind a generic placeholder.
    if app.no_wheel && app.category() != Category::Info {
        Paragraph::new(no_wheel_lines(app))
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
                    // An accessory setting is not missing from the wheel, it is
                    // waiting on hardware the user can plug in, so say so rather
                    // than claiming the wheel does not have it.
                    // Say which of the two reasons applies: the accessory is
                    // missing, or it is present but switched to another mode.
                    let why = match logi_wheel_core::device::required_mode(&row.attr) {
                        Some(m) if app.device.accessory_attached() == Some(true) => {
                            format!("(needs {m} mode)")
                        }
                        _ if logi_wheel_core::device::requires_accessory(&row.attr) => {
                            "(needs handbrake accessory)".to_string()
                        }
                        _ => "(not on this wheel)".to_string(),
                    };
                    (why, Style::default().fg(Color::DarkGray))
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
                } else if row.attr == crate::app::ONBOARD_NAME_ATTR {
                    // The active onboard slot's own name field: a plain
                    // text edit (not the registry's multi-slot SlotText
                    // rotator), so it renders the same way any other
                    // editable text row would, just with no registry spec
                    // behind it.
                    let text = match editing {
                        Some(ed) => ed.display(),
                        None => match &row.value {
                            Ok(Value::Text(s)) => s.clone(),
                            _ => String::new(),
                        },
                    };
                    (text, value_style(editing.is_some(), false))
                } else if row.attr.starts_with("onboard-") {
                    // Every other "Edit onboard slot" flow row (the slot
                    // picker, the copy picker, and the copy/revert/exit
                    // action rows): the value column is the key hint, same
                    // as a saved computer profile row above.
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
        // mirrored pairs collapsed), plus the applies-immediately caption.
        // The GUI's animated direction preview has no text-mode
        // counterpart.
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
        // On the Info category, lead with which wheel was detected, append
        // the software versions (this app, and the loaded kernel module's
        // stamp; `c` prints the same pair on the status line for a manual
        // copy) and the project link so users know where to find docs and
        // source (a terminal cannot open it, but it is copyable).
        if app.category() == Category::Info {
            let display_row = |label: &str, value: String| {
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{label:<24}"), Style::default().fg(Color::Gray)),
                    Span::raw(" "),
                    Span::styled(value, Style::default()),
                ]))
            };
            // Which wheel was detected, first thing on the page: the
            // evdev node's own name when found, else "No wheel detected"
            // (a `no_wheel` device, or a fresh connect evdev has not
            // caught up with yet - `info()` errors either way). Cached by
            // `reload()`, not read fresh here (this draws at ~30 Hz while
            // the live monitor polls).
            let info = app.info();
            let wheel_name = info.map(|i| i.name.clone()).filter(|n| !n.is_empty());
            rows.insert(0, display_row("Wheel", wheel_name.unwrap_or_else(|| "No wheel detected".to_string())));
            // A G923 has no wheel_serial/wheel_firmware sysfs at all (its
            // registry rows above are empty for this category): show the
            // same identity here instead, sourced from the HID uniq string
            // and the cached HID++ query (see `App::g923_firmware`).
            if app.device.model() == logi_wheel_core::WheelModel::G923 {
                let serial = info.map(|i| i.serial.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| "-".to_string());
                rows.insert(1, display_row("Serial", serial));
                rows.insert(2, display_row("Firmware", app.g923_firmware.clone().unwrap_or_else(|| "unavailable".to_string())));
            }
            rows.push(display_row("App", app.app_version_text().to_string()));
            rows.push(display_row("Driver", app.driver_version_text()));
            rows.push(ListItem::new(Line::from(vec![
                Span::styled(format!("{:<24}", "Documentation"), Style::default().fg(Color::Gray)),
                Span::raw(" "),
                Span::styled(logi_wheel_core::PROJECT_URL, Style::default().fg(Color::Cyan)),
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
/// Clip `s` to at most `max` characters, marking a cut with a trailing
/// ellipsis (so the last visible char is the ellipsis, keeping the total at
/// `max`). Character-based so multibyte game names never split mid-glyph.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('\u{2026}');
    out
}

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

    // Plain-English primer on the two kinds of feedback this page sets up;
    // always shown. Kept to two hand-wrapped lines (under ~58 columns) so
    // the compact page still fits every section header on an 80x24 / 100x30
    // terminal.
    lines.push(Line::from(Span::styled(
        "Force feedback = physical force, every wheel game has it.",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        "TrueForce = fine extra vibration; built in, or simulated here.",
        dim,
    )));
    lines.push(Line::from(""));

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
                        None => "not found (PATH or next to logi-wheel)".to_string(),
                    },
                    found_style(app.ffb_path.is_some()),
                );
                if selected {
                    for text in [
                        "Games that use the older DirectInput force-feedback",
                        "method (for example Le Mans Ultimate) get no force",
                        "feedback through Proton by default. Launch them with",
                        "logi-ffb to get force feedback via a virtual wheel.",
                    ] {
                        lines.push(Line::from(format!("  {text}")));
                    }
                    lines.push(Line::from(vec![Span::raw("  logi-ffb: "), ffb_span]));
                    lines.push(Line::from(vec![
                        Span::raw("  Steam launch options: "),
                        Span::styled(logi_wheel_core::games::LAUNCH_LOGI_FFB, Style::default().fg(Color::Yellow)),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::styled("  logi-ffb: ", dim),
                        ffb_span,
                        Span::styled(format!("   launch: {}", logi_wheel_core::games::LAUNCH_LOGI_FFB), dim),
                    ]));
                }
            }
            SetupSection::Sdk => {
                lines.push(setup_header(
                    section.label(),
                    selected,
                    "[Enter or s edits the folder]",
                ));
                let dlls_span = match &app.sdk_resolved {
                    Some(dir) => Span::styled(
                        format!("TrueForce files: found at {}", dir.display()),
                        found_style(true),
                    ),
                    None => Span::styled("TrueForce files: not found", found_style(false)),
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
                            Span::raw("  Folder: "),
                            Span::styled(
                                format!("{draft}_"),
                                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                            ),
                        ]),
                        None => Line::from(format!("  Folder: {}", app.sdk_dir)),
                    });
                    lines.push(Line::from(vec![Span::raw("  "), dlls_span]));
                    for text in [
                        "Games with built-in TrueForce (ACC, AC EVO) need these",
                        "files in their Proton folder. They come from Logitech's",
                        "G HUB on Windows and are never redistributed; the README",
                        "says how to copy them. Install per game in Your games.",
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
                    "[i/u install/remove  h helper  c copy launch  g sim TF  a add  Esc back]"
                } else {
                    "[Enter opens the list]"
                };
                lines.push(setup_header(section.label(), selected, hint));
                if selected {
                    lines.push(Line::from(Span::styled(
                        "  Your installed Proton games and what each one needs.",
                        dim,
                    )));
                    if app.games.is_empty() {
                        lines.push(Line::from(Span::styled(
                            if app.games_scanned {
                                "  No known games found (r rescans, a adds one by hand)"
                            } else {
                                "  Scanning your game launchers..."
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
                        // Match the game to the compatibility registry and
                        // show only the status its enablement action needs
                        // (shared with the GUI's "Your games" list).
                        let compat = logi_wheel_core::games::match_title(&g.name);
                        let status = match compat.map(|c| c.setup_action(app.wheel_caps())) {
                            Some(logi_wheel_core::games::SetupAction::InstallShim) => {
                                if g.shim_installed {
                                    Span::styled("TrueForce on", Style::default().fg(Color::Green))
                                } else {
                                    Span::styled("TrueForce off (i installs)", dim)
                                }
                            }
                            Some(logi_wheel_core::games::SetupAction::UseLogiFfb) => Span::styled(
                                format!("launch: {}", logi_wheel_core::games::LAUNCH_LOGI_FFB),
                                Style::default().fg(Color::Yellow),
                            ),
                            Some(logi_wheel_core::games::SetupAction::SimulatedTrueForce) => {
                                match compat.and_then(|c| c.simulated_tf.live_id()) {
                                    Some(id) => {
                                        let sim = app.tf_cfg.game(id);
                                        if sim.enabled {
                                            Span::styled(
                                                format!("sim TF on {}%", sim.intensity),
                                                Style::default().fg(Color::Green),
                                            )
                                        } else {
                                            Span::styled("sim TF off (g)".to_string(), dim)
                                        }
                                    }
                                    None => Span::raw(""),
                                }
                            }
                            Some(logi_wheel_core::games::SetupAction::WorksOutOfBox) => {
                                Span::styled("works out of the box", Style::default().fg(Color::Green))
                            }
                            // Unrecognised titles are filtered out of this
                            // list unless they already carry the shim (see
                            // `launchers::keep_for_setup`), so `None` here
                            // always means "added by hand".
                            None => Span::styled(
                                "TrueForce added by you (u removes)",
                                Style::default().fg(Color::Green),
                            ),
                        };
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("  {}{:<20}", if cursor { "> " } else { "  " }, truncate(&g.name, 18)),
                                name_style,
                            ),
                            Span::styled(format!(" [{:<6}] ", g.source.label()), dim),
                            status,
                        ]));
                        // The plain-English "what makes it best" line from
                        // the registry, dimmed under the game; an added-by-
                        // hand title gets its own short explainer instead.
                        if let Some(c) = compat {
                            lines.push(Line::from(Span::styled(
                                format!("    {}", c.setup_line(app.wheel_caps())),
                                dim,
                            )));
                            // The launch options this title needs on this
                            // wheel, spelled out so they can be copied (c)
                            // or read off the screen. Nothing is written to
                            // the user's Steam config.
                            if let Some(opts) = c.launch_options(app.wheel_caps()) {
                                lines.push(Line::from(vec![
                                    Span::styled("    launch options: ", dim),
                                    Span::styled(opts, Style::default().fg(Color::Yellow)),
                                    Span::styled("  [c copies]", dim),
                                ]));
                            }
                        } else {
                            lines.push(Line::from(Span::styled(
                                "    Added by you; remove if this game does not use TrueForce.",
                                dim,
                            )));
                        }
                    }
                } else {
                    lines.push(Line::from(Span::styled(
                        if !app.games_scanned {
                            "  not scanned yet".to_string()
                        } else if app.games.is_empty() {
                            "  none found (r rescans)".to_string()
                        } else {
                            format!("  {} installed game(s) (Enter opens the list)", app.games.len())
                        },
                        dim,
                    )));
                }
            }
            SetupSection::SimTf => {
                let hint = if inside && app.tf_effects_open {
                    "[[ ] layer  v level  l hide  m/e/p/x/d/t  Esc back]"
                } else if inside {
                    "[m master  e intensity  p pitch  x effects  l layers  d daemon  t sweep  Esc back]"
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
                        "Creates TrueForce-style engine vibration from a game's",
                        "own telemetry, for games without built-in TrueForce.",
                    ] {
                        lines.push(Line::from(format!("  {text}")));
                    }
                    lines.push(Line::from(vec![
                        Span::raw("  logi-tf-sim: "),
                        Span::styled(
                            match &app.tf_bin {
                                Some(p) => format!("found: {}", p.display()),
                                None => "not found (PATH or next to logi-wheel)".to_string(),
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
                        "  rev rate = how fast the engine buzz rises; 100 tracks",
                        dim,
                    )));
                    lines.push(Line::from(Span::styled(
                        "  the engine. Turn it on per game in Your games (g).",
                        dim,
                    )));
                    // The haptic effects layer.
                    lines.push(Line::from(vec![
                        Span::raw("  extra effects: "),
                        Span::styled(
                            if app.tf_cfg.effects { "on" } else { "off" },
                            found_style(app.tf_cfg.effects),
                        ),
                        Span::styled("   (limiters, shifts, ABS, grip)", dim),
                    ]));
                    lines.push(Line::from(Span::styled(
                        "  only for games you enabled above; games with built-in",
                        dim,
                    )));
                    lines.push(Line::from(Span::styled(
                        "  TrueForce are not affected by any of it.",
                        dim,
                    )));
                    // Gated on the layer being on, matching the GUI: levels
                    // that currently do nothing should not be presented as
                    // if they did.
                    if app.tf_effects_open && app.tf_cfg.effects {
                        for (i, effect) in logi_wheel_core::tfsim::EFFECTS.iter().enumerate() {
                            let picked = i == app.tf_effect_idx;
                            let gain = app.tf_cfg.effect_gains.get(effect.key);
                            let value = match (&app.tf_effect_edit, picked) {
                                (Some(d), true) => Span::styled(
                                    format!("{d}_"),
                                    Style::default()
                                        .fg(Color::Yellow)
                                        .add_modifier(Modifier::BOLD),
                                ),
                                _ => Span::raw(format!("{gain}%")),
                            };
                            let name = if picked {
                                Span::styled(
                                    format!("{:<14}", effect.label),
                                    Style::default().add_modifier(Modifier::BOLD),
                                )
                            } else {
                                Span::raw(format!("{:<14}", effect.label))
                            };
                            lines.push(Line::from(vec![
                                Span::raw(if picked { "  > " } else { "    " }),
                                name,
                                value,
                            ]));
                            // The caveat only on the selected row: ten of
                            // them at once would drown the list.
                            if picked && !effect.note.is_empty() {
                                lines.push(Line::from(Span::styled(
                                    format!("      {}", effect.note),
                                    dim,
                                )));
                            }
                        }
                    } else if app.tf_cfg.effects {
                        lines.push(Line::from(Span::styled(
                            "  l shows each layer's level",
                            dim,
                        )));
                    }
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
    // The exhaustive game-compatibility list lives in the project wiki now;
    // the page links there instead of carrying a static table. A terminal
    // cannot open it, but the URL is copyable.
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Full game compatibility list: ", dim),
        Span::styled(
            "github.com/mescon/logitech-trueforce-linux-driver/wiki/Force-Feedback-in-Games",
            Style::default().fg(Color::Cyan),
        ),
    ]));
    (lines, starts)
}

/// Render the Setup body: the sectioned page `setup_sections` builds (the
/// primer, then Your games, the logi-ffb helper, the TrueForce files
/// folder, and Simulated TrueForce, then the wiki compatibility link).
/// Shown instead of the settings list whenever
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
/// read off the wheel's evdev node, the light-up button list, and (below
/// them, `draw_test_plan`) the guarded force-sim plan and progress.
/// Mirrors the GUI's Info page in text form. Renders into
/// `draw_scrolled`'s buffer, like the other composed views.
fn draw_monitor<S: SysfsIo>(buf: &mut Buffer, app: &App<S>, area: Rect) {
    use logi_wheel_core::evtest;

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
        .constraints([
            Constraint::Length(monitor_top_height(t)),
            Constraint::Length(
                3 + logi_wheel_core::evtest::button_codes_for_model(app.device.model()).len() as u16,
            ),
            Constraint::Length(test_plan_height(t)),
        ])
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
    if let Some(err) = &t.open_error {
        top.push(Line::from(Span::styled(err.clone(), Style::default().fg(Color::Red))));
    }
    Paragraph::new(top)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title("Test area"))
        .render(rows[0], buf);

    // The button tester: every wheel button, reverse-video while held,
    // with the recent-press history on top. The code list and labels are
    // model-aware (see `evtest::button_codes_for_model`/
    // `button_name_for_model`): RS50/G PRO keep their captured diagram
    // labels, a G923 gets its own captured `G923_BUTTONS` labels instead
    // of the RS50's - wrong for it - labels (e.g. its own 0x2c8 is its PS
    // button, not the RS50's left encoder, which the G923 does not have).
    let model = app.device.model();
    let recent = if t.recent.is_empty() {
        "-".to_string()
    } else {
        t.recent
            .iter()
            .map(|c| evtest::button_name_for_model(model, *c))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut items: Vec<ListItem> = vec![ListItem::new(Line::from(vec![
        Span::styled("Last pressed: ", Style::default().fg(Color::Gray)),
        Span::raw(recent),
    ]))];
    items.extend(evtest::button_codes_for_model(model).into_iter().map(|code| {
        let label = evtest::button_name_for_model(model, code);
        let held = t.pressed.contains(&code);
        let mut item = ListItem::new(format!("  {label:<18}"));
        if held {
            item = item.style(Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD));
        }
        item
    }));
    List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Buttons (highlighted while held)"))
        .render(rows[1], buf);

    draw_test_plan(buf, t, rows[2]);
}

/// Render the "Force feedback test" panel: before anything is confirmed,
/// a one-line hint naming the keys; once a sequence has been confirmed,
/// its whole plan as one row per step - label, duration, and live state
/// (pending/counting down/playing/done/skipped, straight off
/// `TestView::sim_progress`, the same state machine the GUI renders) -
/// shown in full up front and left in place after the run ends, which is
/// the core of the request this replaces a single overwriting status
/// line with. Every row's state is also spelled out as its own word
/// (never color alone), matching `StepState::status_text`.
fn draw_test_plan(buf: &mut Buffer, t: &TestView, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    match t.sim_kind {
        None => {
            lines.push(Line::from(Span::styled(
                "f: simulate force feedback   t: simulate TrueForce texture",
                Style::default().fg(Color::Gray),
            )));
        }
        Some(kind) => {
            let progress = t.sim_progress();
            let header = if t.sim_running() {
                format!("{}: running (s to stop)", kind.label())
            } else {
                format!("{}: not running (f/t to run again)", kind.label())
            };
            lines.push(Line::from(Span::styled(header, Style::default().add_modifier(Modifier::BOLD))));
            for (step, state) in kind.steps().iter().zip(progress.states.iter()) {
                let style = match state {
                    StepState::Pending => Style::default().fg(Color::Gray),
                    StepState::Countdown(_) => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    StepState::Playing => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                    StepState::Done => Style::default().fg(Color::Cyan),
                    StepState::Skipped => Style::default().fg(Color::DarkGray),
                };
                let secs = f32::from(step.duration_ms) / 1000.0;
                lines.push(Line::from(Span::styled(
                    format!("  {:<9} {} ({secs:.1}s)", state.status_text(), step.label),
                    style,
                )));
            }
        }
    }
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title("Force feedback test"))
        .render(area, buf);
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

/// Render the "Add a game" picker: `App::addable`'s unrecognised Wine
/// games, plus a trailing "type a path" row that swaps to a text field
/// once selected. The selected row (or the manual field, while typing)
/// wears reverse video, same convention as every other list in this TUI.
fn draw_add_game_picker<S: SysfsIo>(
    f: &mut Frame,
    app: &App<S>,
    picker: &crate::app::AddGamePicker,
    area: Rect,
) {
    let dim = Style::default().fg(Color::DarkGray);
    let width = area.width.saturating_sub(6).clamp(30, 64).min(area.width);
    let manual_row = app.addable.len();
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        "Install the TrueForce SDK shim into a game this list did not recognise.",
        dim,
    )));
    lines.push(Line::from(""));
    if app.addable.is_empty() {
        lines.push(Line::from(Span::styled("  No unrecognised Wine games were found.", dim)));
    }
    for (i, g) in app.addable.iter().enumerate() {
        let cursor = picker.manual.is_none() && picker.idx == i;
        let style =
            if cursor { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
        lines.push(Line::from(Span::styled(
            format!(
                "{}{:<28} [{}]",
                if cursor { "> " } else { "  " },
                truncate(&g.name, 26),
                g.source.label()
            ),
            style,
        )));
    }
    match &picker.manual {
        Some(draft) => lines.push(Line::from(vec![
            Span::raw("> prefix path: "),
            Span::styled(
                format!("{draft}_"),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
        ])),
        None => {
            let cursor = picker.idx == manual_row;
            let style =
                if cursor { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
            lines.push(Line::from(Span::styled(
                format!("{}type a wine prefix path...", if cursor { "> " } else { "  " }),
                style,
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        if picker.manual.is_some() {
            "[Enter installs  Esc back to the list]"
        } else {
            "[Up/Down select  Enter installs  Esc cancels]"
        },
        dim,
    )));

    let height = (lines.len() as u16).saturating_add(2).min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Add a game ")),
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
    use logi_wheel_core::sysfs::FakeSysfs;
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
        App::new(logi_wheel_core::Device::with_io(fs))
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

    fn g923_app() -> App<FakeSysfs> {
        let fs = FakeSysfs::new();
        fs.set("range", "900");
        fs.set("gain", "65535");
        fs.set("autocenter", "0");
        fs.set("combine_pedals", "0");
        let device =
            logi_wheel_core::Device::with_io_and_model(fs, logi_wheel_core::WheelModel::G923);
        let mut a = App::new(device);
        a.reload();
        a
    }

    #[test]
    fn g923_sidebar_omits_leds_but_keeps_profiles() {
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let a = g923_app();
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(!text.contains("LIGHTSYNC"), "LIGHTSYNC must be hidden for a G923:\n{text}");
        // The categories with real content, plus Profiles (the computer
        // profile store) and Setup, are all still there.
        for label in
            ["Force feedback", "Steering", "Pedals", "Info / Testing", "Profiles / mode", "Setup"]
        {
            assert!(text.contains(label), "missing {label}:\n{text}");
        }
    }

    #[test]
    fn info_is_the_first_sidebar_entry_and_the_default_view() {
        // Request 1: Info/Testing is the first sidebar item and what a
        // freshly built app (index 0, no navigation) shows.
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let a = wheel_app();
        assert!(a.is_info(), "a fresh App starts on the Info view");
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        let info_pos = text.find("Info / Testing").expect("Info / Testing listed");
        let ffb_pos = text.find("Force feedback").expect("Force feedback listed");
        assert!(info_pos < ffb_pos, "Info must lead the sidebar:\n{text}");
    }

    #[test]
    fn info_page_leads_with_a_wheel_row() {
        // Request 2: a "Wheel" row, ahead of Serial/Firmware/App/Driver.
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let a = wheel_app();
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        let wheel_pos = text.find("Wheel").expect("Wheel row present");
        let serial_pos = text.find("Serial").expect("Serial row present");
        let app_pos = text.find("App").expect("App row present");
        assert!(wheel_pos < serial_pos && wheel_pos < app_pos, "Wheel must lead the block:\n{text}");
    }

    #[test]
    fn the_no_wheel_page_explains_the_problem_and_names_the_fix() {
        use logi_wheel_core::diagnose::{Fix, Severity};
        let mut a = App::new(logi_wheel_core::Device::with_io(FakeSysfs::new()));
        assert!(a.no_wheel);
        // Off the Info page, whose identity block has its own empty state.
        a.set_cat(Category::ALL.iter().position(|c| *c == Category::Steering).unwrap());
        a.diagnosis = vec![
            logi_wheel_core::diagnose::Finding {
                severity: Severity::Ok,
                title: "Wheel detected".to_string(),
                detail: "Attached over USB.".to_string(),
                fix: None,
            },
            logi_wheel_core::diagnose::Finding {
                severity: Severity::Blocking,
                title: "The driver is not running".to_string(),
                detail: "The driver is installed but not loaded.".to_string(),
                fix: Some(Fix { command: "modprobe hid-logitech-dd".into(), needs_root: true }),
            },
        ];

        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);

        assert!(text.contains("The driver is not running"), "{text}");
        assert!(text.contains("not loaded"), "{text}");
        // The fix is shown ready to run, privilege included.
        assert!(text.contains("sudo modprobe hid-logitech-dd"), "{text}");
        // Severity is readable without colour: the marks carry it.
        assert!(text.contains("FAILED"), "{text}");
        assert!(text.contains("ok "), "{text}");
        // The old one-liner is gone once there is something better to say.
        assert!(!text.contains("(no wheel connected"), "{text}");
    }

    #[test]
    fn the_no_wheel_page_falls_back_when_nothing_was_diagnosed() {
        // The checks read the real /sys, so they never run under test. The
        // empty state must still say something.
        let mut a = App::new(logi_wheel_core::Device::with_io(FakeSysfs::new()));
        a.set_cat(Category::ALL.iter().position(|c| *c == Category::Steering).unwrap());
        assert!(a.diagnosis.is_empty());
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| draw(f, &a)).unwrap();
        assert!(screen(&term).contains("no wheel connected"));
    }

    #[test]
    fn info_page_shows_no_wheel_detected_without_a_device() {
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let a = App::new(logi_wheel_core::Device::with_io(FakeSysfs::new()));
        assert!(a.no_wheel);
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(text.contains("No wheel detected"), "{text}");
    }

    #[test]
    fn g923_info_page_shows_serial_and_firmware_rows() {
        // Request 3: a G923 (no wheel_serial/wheel_firmware sysfs at all)
        // still gets Serial/Firmware rows on the Info page. `g923_app`'s
        // `with_io_and_model` has no real HID sysfs directory behind it, so
        // both are deterministically the "no value" state here (a live
        // device is exercised separately, see `logi-wheel-core::hidpp`'s and
        // `device`'s own tests).
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let a = g923_app();
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        let firmware_line = text
            .lines()
            .find(|l| l.contains("Firmware"))
            .expect("a Firmware row is rendered");
        assert!(firmware_line.contains("unavailable"), "{firmware_line}");
        let serial_line =
            text.lines().find(|l| l.contains("Serial")).expect("a Serial row is rendered");
        assert!(serial_line.contains('-'), "{serial_line}");
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
            "Your games",
            "Force feedback helper (logi-ffb)",
            "TrueForce files",
            "Simulated TrueForce",
        ] {
            assert!(text.contains(header), "missing header {header}:\n{text}");
        }
        // The full compatibility list is a wiki link now, not a table.
        assert!(text.contains("Full game compatibility list"), "wiki link present:\n{text}");
        // Your games leads the accordion and is selected by default, so its
        // list body is expanded.
        assert!(
            text.contains("Your installed Proton games"),
            "the selected section is expanded:\n{text}"
        );
        // Selecting the Simulated TrueForce section (last) expands its controls.
        use crossterm::event::KeyCode;
        a.on_key(KeyCode::Down); // Ffb
        a.on_key(KeyCode::Down); // Sdk
        a.on_key(KeyCode::Down); // SimTf
        assert_eq!(a.setup_section(), crate::app::SetupSection::SimTf);
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(text.contains("master:"), "the selected section expands:\n{text}");
    }

    #[test]
    fn games_list_shows_the_source_tag_and_an_added_by_hand_row() {
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut a = setup_view_app();
        a.focus = Focus::Content;
        a.games = vec![
            logi_wheel_core::launchers::DiscoveredGame {
                name: "Assetto Corsa Competizione".to_string(),
                source: logi_wheel_core::launchers::Source::Steam,
                kind: logi_wheel_core::launchers::GameKind::Wine {
                    prefix: std::path::PathBuf::from("/pfx/acc"),
                },
                shim_installed: false,
            },
            logi_wheel_core::launchers::DiscoveredGame {
                name: "TEKKEN 8".to_string(),
                source: logi_wheel_core::launchers::Source::Lutris,
                kind: logi_wheel_core::launchers::GameKind::Wine {
                    prefix: std::path::PathBuf::from("/pfx/tekken"),
                },
                shim_installed: true,
            },
        ];
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(text.contains("[Steam"), "the recognised game's source tag:\n{text}");
        assert!(text.contains("[Lutris"), "the added-by-hand game's source tag:\n{text}");
        assert!(text.contains("added by you"), "the added-by-hand explainer:\n{text}");
    }

    #[test]
    fn add_game_picker_renders_the_addable_list_and_the_manual_row() {
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut a = setup_view_app();
        a.focus = Focus::Content;
        a.addable = vec![logi_wheel_core::launchers::DiscoveredGame {
            name: "TEKKEN 8".to_string(),
            source: logi_wheel_core::launchers::Source::Lutris,
            kind: logi_wheel_core::launchers::GameKind::Wine {
                prefix: std::path::PathBuf::from("/pfx/tekken"),
            },
            shim_installed: false,
        }];
        a.add_game = Some(crate::app::AddGamePicker { idx: 0, manual: None });
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(text.contains("Add a game"), "the picker's title:\n{text}");
        assert!(text.contains("TEKKEN 8"), "the addable row:\n{text}");
        assert!(text.contains("type a wine prefix path"), "the trailing manual row:\n{text}");
    }

    #[test]
    fn small_terminal_flags_the_clipped_setup_view_and_scrolls_to_the_end() {
        // A short terminal cannot fit every section, so the markers and the
        // scroll fallback must still work, ending at the wiki link.
        let mut term = Terminal::new(TestBackend::new(80, 16)).unwrap();
        let mut a = setup_view_app();
        a.focus = Focus::Content;
        a.setup_scroll = 0;
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(text.contains("more below"), "clipped content is flagged:\n{text}");
        // Scroll to the bottom: the marker flips and the last line (the
        // wiki link) shows.
        a.scroll_view(i32::from(a.max_scroll()));
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(text.contains("more above"), "the scrolled state is flagged:\n{text}");
        assert!(!text.contains("more below"), "nothing is clipped below any more:\n{text}");
        assert!(
            text.contains("Full game compatibility list"),
            "scrolling reaches the wiki link at the bottom:\n{text}"
        );
    }

    #[test]
    fn a_tall_terminal_needs_no_scroll_marker() {
        let mut term = Terminal::new(TestBackend::new(100, 60)).unwrap();
        let a = setup_view_app();
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(!text.contains("more below") && !text.contains("more above"), "{text}");
        assert!(text.contains("Your games"), "everything fits:\n{text}");
    }

    #[test]
    fn info_view_scrolls_down_to_the_button_tester() {
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut a = wheel_app();
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Info).unwrap();
        a.reload();
        a.test.dev = Some(logi_wheel_core::evtest::WheelInput {
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

    #[test]
    fn test_plan_shows_every_row_pending_the_instant_it_is_confirmed() {
        // The task's core acceptance check: confirming a simulation shows
        // the FULL ordered list of steps, with durations, before anything
        // plays - not a single line naming only the current step. Tall
        // enough (and wide enough for the longest label) that nothing
        // wraps or scrolls out of the unclipped screen buffer.
        let mut term = Terminal::new(TestBackend::new(140, 90)).unwrap();
        let mut a = wheel_app();
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Info).unwrap();
        a.reload();
        a.test.dev = Some(logi_wheel_core::evtest::WheelInput {
            event_path: "/nonexistent/event99".to_string(),
            name: "Logitech RS50 Base".to_string(),
        });
        a.on_key(crossterm::event::KeyCode::Char('f'));
        a.on_key(crossterm::event::KeyCode::Char('y'));
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        for step in logi_wheel_core::fftest::FORCE_SEQUENCE {
            assert!(text.contains(step.label), "row for {:?} missing:\n{text}", step.label);
        }
        assert!(text.contains("pending"), "at least one row starts pending:\n{text}");
    }

    #[test]
    fn test_plan_rows_reflect_the_progress_state_machine_and_stay_after_a_run() {
        // Drive the state machine directly (no real device, no real
        // sleeping) the way the shared `fftest::SequenceProgress` renders
        // it: one row done, one skipped, one still pending. A finished
        // row must stay on screen, not disappear once its state moves
        // past "playing". Tall enough that the whole page (including the
        // trailing "Force feedback test" panel) renders unclipped.
        let mut term = Terminal::new(TestBackend::new(140, 90)).unwrap();
        let mut a = wheel_app();
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Info).unwrap();
        a.reload();
        a.test.dev = Some(logi_wheel_core::evtest::WheelInput {
            event_path: "/nonexistent/event99".to_string(),
            name: "Logitech RS50 Base".to_string(),
        });
        a.on_key(crossterm::event::KeyCode::Char('f'));
        a.on_key(crossterm::event::KeyCode::Char('y'));

        // Wait out the background thread (a nonexistent device fails to
        // open almost immediately), then overwrite the progress by hand
        // to exercise a mid-run-looking state without any real FF I/O.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while a.test.sim_running() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let mut progress = logi_wheel_core::fftest::SequenceProgress::new(logi_wheel_core::fftest::FORCE_SEQUENCE);
        progress.apply(&logi_wheel_core::fftest::SequenceEvent::Done { row: 0, total: 10 });
        progress.apply(&logi_wheel_core::fftest::SequenceEvent::Skipped(&[(1, "skip-me")]));
        *a.test.sim_progress_for_test() = progress;

        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(text.contains("done"), "the finished row must still be shown:\n{text}");
        assert!(text.contains("skipped"), "the skipped row must still be shown:\n{text}");
        assert!(text.contains("pending"), "the untouched rows stay pending:\n{text}");
    }

    /// The "Edit onboard slot" flow's synthetic rows have no registry spec
    /// behind them, so the generic renderer's `Device::spec` lookup would
    /// otherwise fall through to the "?" placeholder used for a row this
    /// view genuinely cannot make sense of; both the picker and the active
    /// editor's own rows must show their real hint text instead.
    #[test]
    fn onboard_flow_rows_render_their_hints_not_a_placeholder() {
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let fs = FakeSysfs::new();
        fs.set("wheel_mode", "desktop");
        fs.set("wheel_profile", "0");
        fs.set("wheel_profile_names", "1: AC EVO\n2: GT7\n3: PROFILE 3\n4: PROFILE 4\n5: PROFILE 5");
        fs.set("wheel_range", "900");
        fs.set("wheel_strength", "80");
        let mut a = App::new(logi_wheel_core::Device::with_io(fs));
        a.focus = Focus::Content;
        // An empty, unique temp dir: the real default profile store must
        // never leak into this render (its content is host-specific, not
        // something this test can predict).
        a.profiles_dir = std::env::temp_dir().join(format!(
            "logi-wheel-ui-test-onboard-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        a.cat_idx = Category::ALL.iter().position(|c| *c == Category::Profiles).unwrap();
        a.reload();

        // The picker.
        a.row_idx = a.rows.iter().position(|r| r.attr == crate::app::ONBOARD_EDIT_ATTR).unwrap();
        a.on_key(crossterm::event::KeyCode::Enter);
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(text.contains("Slot 1"), "the picker rows must render:\n{text}");
        assert!(text.contains("AC EVO"), "slot 1's name must show in the picker, not a placeholder:\n{text}");
        assert!(text.contains("PROFILE 4"), "an unnamed slot's default label must show:\n{text}");

        // Pick slot 2 and check the active editor's rows.
        a.row_idx = 1;
        a.on_key(crossterm::event::KeyCode::Enter);
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(text.contains("Slot name"), "the name row must render:\n{text}");
        assert!(text.contains("GT7"), "slot 2's current name must show, not a placeholder:\n{text}");
        assert!(text.contains("Revert this slot"), "the revert action must render:\n{text}");
        assert!(
            text.contains("Copy from computer profile"),
            "the copy action must render:\n{text}"
        );
    }

    /// Drive the Setup view down to the Simulated TrueForce section and
    /// enter it, which is what makes its controls visible.
    fn sim_tf_app() -> App<FakeSysfs> {
        use crossterm::event::KeyCode;
        let mut a = setup_view_app();
        // Pin the config these tests render, rather than inheriting whatever
        // is in the developer's own ~/.config/logi-wheel/tf-sim.conf. App::new
        // loads that file, so with `effects=0` in it the layer list correctly
        // stays hidden and these assertions failed - on this machine only,
        // and only after something happened to write that key. A test whose
        // result depends on the home directory it runs in is not testing the
        // code.
        a.tf_cfg = logi_wheel_core::tfsim::Config { effects: true, ..Default::default() };
        a.focus = Focus::Content;
        for _ in 0..3 {
            a.on_key(KeyCode::Down);
        }
        assert_eq!(a.setup_section(), crate::app::SetupSection::SimTf);
        a.on_key(KeyCode::Enter);
        a
    }

    #[test]
    fn the_effects_layer_and_its_scope_are_on_screen() {
        let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let a = sim_tf_app();
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(text.contains("extra effects:"), "the layer switch is shown:\n{text}");
        // The scope has to be visible without opening anything, or somebody
        // tunes ten levels and wonders why their sim feels the same.
        assert!(
            text.contains("only for games you enabled above"),
            "the scope is stated:\n{text}"
        );
        assert!(
            text.contains("built-in"),
            "and that built-in TrueForce is unaffected:\n{text}"
        );
    }

    #[test]
    fn the_layer_list_appears_only_once_asked_for() {
        use crossterm::event::KeyCode;
        let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut a = sim_tf_app();
        term.draw(|f| draw(f, &a)).unwrap();
        assert!(!screen(&term).contains("Rev limiter"), "ten layers stay folded");

        a.on_key(KeyCode::Char('l'));
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        for label in ["Engine", "Rev limiter", "Gear shifts", "ABS", "Impacts", "DRS"] {
            assert!(text.contains(label), "missing layer {label}:\n{text}");
        }
    }

    #[test]
    fn the_selected_layer_is_marked_and_carries_its_caveat() {
        use crossterm::event::KeyCode;
        let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut a = sim_tf_app();
        a.on_key(KeyCode::Char('l'));
        // Engine leads and works everywhere, so it has nothing to warn about.
        term.draw(|f| draw(f, &a)).unwrap();
        assert!(screen(&term).contains("> Engine"), "the selection is marked");

        // Walk to a layer only one game feeds.
        for _ in 0..2 {
            a.on_key(KeyCode::Char(']'));
        }
        term.draw(|f| draw(f, &a)).unwrap();
        let text = screen(&term);
        assert!(text.contains("> Pit limiter"), "selection moved:\n{text}");
        assert!(
            text.contains("Only BeamNG"),
            "a layer one game feeds says so:\n{text}"
        );
    }
}
