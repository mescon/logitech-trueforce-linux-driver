slint::include_modules!();

mod bridge;
mod viewmodel;
mod worker;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use logi_dd_core::curve::Curve;
use logi_dd_core::{Category, Color, Mode, Value, REGISTRY};
use slint::Model as _;
use viewmodel::WidgetInput;
use worker::{Request, Response, Worker};

/// The curve editor's in-flight state: which attribute it is editing (so
/// `commit` knows what to write and which category to refresh) and the
/// `Curve` being shaped. Lives on the UI thread only; the worker never sees
/// it until `commit` sends the finished `Curve` as a `WidgetInput::Curve`.
struct CurveEditorState {
    attr: String,
    category: Category,
    curve: Curve,
}

/// The RGB strip editor's in-flight state: same shape as `CurveEditorState`,
/// but the shaped value is the strip's `Vec<Color>` (one per LED). Lives on
/// the UI thread only; the worker never sees it until `commit` sends the
/// finished list as a `WidgetInput::Rgb`.
struct RgbEditorState {
    attr: String,
    category: Category,
    colors: Vec<Color>,
}

/// The slot-text editor's in-flight state: which attribute it is editing
/// (so a write knows what to send and which category to refresh) and the
/// slot names currently shown. Unlike `CurveEditorState`/`RgbEditorState`,
/// nothing here is staged for a final commit: `Kind::SlotText` writes one
/// slot at a time, so each `set-slot-name` call sends its own
/// `WidgetInput::SlotText` immediately. `names` is only kept so the overlay
/// can be redrawn (optimistically) after each apply; lives on the UI thread
/// only, same rules as the other two editor states.
struct SlotTextEditorState {
    attr: String,
    category: Category,
    names: Vec<String>,
}

/// Push `curve`'s current shape to the Slint side: the composed plot line,
/// the draggable control points, and the two deadzone slider values. Called
/// after every curve-editor edit so the overlay's preview stays live.
fn push_curve_editor(app: &App, curve: &Curve) {
    app.set_curve_plot_commands(bridge::curve_plot_commands(curve).into());
    app.set_curve_control_points(bridge::curve_control_points(curve));
    app.set_curve_lower_deadzone(i32::from(curve.lower_deadzone()));
    app.set_curve_upper_deadzone(i32::from(curve.upper_deadzone()));
}

/// Push `colors`' current shape to the Slint side: the swatch list. Called
/// after every RGB-editor edit so the row of swatches stays live.
fn push_rgb_editor(app: &App, colors: &[Color]) {
    app.set_rgb_leds(bridge::rgb_leds_model(colors));
}

/// Push `names`' current shape to the Slint side: the slot-name row list.
/// Called after every slot-text edit so the fields stay in sync with what
/// was just sent to the worker.
fn push_slot_text_editor(app: &App, names: &[String]) {
    app.set_slot_text_names(bridge::slot_names_model(names));
}

/// Replace the persistent rows model's contents with a whole category's
/// worth of fresh rows (`LoadCategory`, `Refresh`, the no-wheel screen's
/// Retry, or a mode switch's follow-up refresh). `app.set_rows` is only
/// ever called once, at startup (`main`), installing a `VecModel` this
/// function and `update_row` mutate in place from then on; that way the
/// `SettingsList`'s repeater never sees the model itself change identity,
/// only its contents, which is what keeps a widget that is not part of
/// this reload (e.g. an open `ComboBox` popup) from being torn down for no
/// reason.
///
/// When the new content is the same length as what is already shown (the
/// common case: the same category re-read after an unrelated refresh),
/// each row is replaced in place via `set_row_data`, which only re-renders
/// rows whose value actually differs. A different length (a category
/// switch) falls back to replacing the whole content in one go: every row
/// is for a different setting there anyway, so there is nothing to
/// preserve.
fn load_rows(app: &App, rows: &[viewmodel::Row]) {
    let items = bridge::setting_rows(rows);
    let model = app.get_rows();
    if model.row_count() == items.len() {
        for (i, item) in items.into_iter().enumerate() {
            model.set_row_data(i, item);
        }
        return;
    }
    match model.as_any().downcast_ref::<slint::VecModel<SettingRow>>() {
        Some(vec_model) => vec_model.set_vec(items),
        // Should not happen: `main` installs a `VecModel` before the first
        // response can arrive. Fall back to installing a fresh one rather
        // than silently dropping the reload.
        None => app.set_rows(slint::ModelRc::new(slint::VecModel::from(items))),
    }
}

/// Update just the row named by `row.attr` in place (a successful or
/// failed single-field edit), without touching any other row's widget.
/// `error` is the edit's failure message, or `None` on success; either way
/// `row` itself already reflects a fresh read (see `Response::RowUpdated`'s
/// doc comment), so this never has to guess at what reverting looks like.
fn update_row(app: &App, row: &viewmodel::Row, error: Option<&str>) {
    let model = app.get_rows();
    let Some(index) = (0..model.row_count()).find(|&i| model.row_data(i).is_some_and(|r| r.attr == row.attr))
    else {
        return;
    };
    model.set_row_data(index, bridge::to_setting_row_with_error(row, error));
}

/// Read the `Category`/`Mode` out of one of the `Arc<Mutex<_>>` cells below.
/// Both are `Copy`, so the lock never needs to outlive this call.
fn get<T: Copy>(cell: &Arc<Mutex<T>>) -> T {
    *cell.lock().unwrap()
}

fn set<T>(cell: &Arc<Mutex<T>>, value: T) {
    *cell.lock().unwrap() = value;
}

/// Open `url` in the user's default browser via `xdg-open`, detached. Best
/// effort: a spawn failure (no `xdg-open` on a minimal system) is ignored
/// rather than taking the app down, since this is a convenience link.
fn open_in_browser(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

fn main() -> Result<(), slint::PlatformError> {
    let app = App::new()?;
    app.set_category_labels(bridge::category_labels_model());
    app.set_project_url(logi_dd_core::PROJECT_URL.into());
    app.on_open_url(|url| open_in_browser(&url));
    // Installed once, here, and never replaced: `load_rows`/`update_row`
    // mutate this same `VecModel`'s contents for the rest of the app's
    // life (see `load_rows`'s doc comment for why that matters).
    app.set_rows(slint::ModelRc::new(slint::VecModel::<SettingRow>::default()));

    // UI-side state the worker's responses need to be checked against, or
    // the mode toggle needs to compute its target: which category is on
    // screen, and which mode the header last reported. Both start at a
    // reasonable default and get corrected by the worker's first replies.
    // `Arc<Mutex<_>>`, not `Rc<Cell<_>>`, because the worker's response
    // callback runs on the worker thread and must be `Send`.
    let current_category = Arc::new(Mutex::new(Category::ALL[0]));
    let current_mode = Arc::new(Mutex::new(Mode::Desktop));
    app.set_category_label(get(&current_category).label().into());
    app.set_selected_category(bridge::index_of(get(&current_category)));

    // The current category's row values, keyed by attr, refreshed on every
    // `Response::Rows`. The curve editor needs this to seed a `Curve` from
    // the attribute's live value when a row is activated: the worker only
    // hands back `Vec<Row>` inside that one response, so nothing else in
    // this file holds onto it once `bridge::rows_model` has built the
    // Slint-facing rows.
    let known_values: Arc<Mutex<HashMap<String, Value>>> = Arc::new(Mutex::new(HashMap::new()));
    // The curve editor's in-flight state, `None` while the overlay is
    // closed. UI-thread only (see `CurveEditorState`'s own doc comment).
    let curve_editor: Arc<Mutex<Option<CurveEditorState>>> = Arc::new(Mutex::new(None));
    // The RGB strip editor's in-flight state, same lifetime/thread rules as
    // `curve_editor` (see `RgbEditorState`'s own doc comment).
    let rgb_editor: Arc<Mutex<Option<RgbEditorState>>> = Arc::new(Mutex::new(None));
    // The slot-text editor's in-flight state, same lifetime/thread rules as
    // `curve_editor` and `rgb_editor` (see `SlotTextEditorState`'s own doc
    // comment).
    let slot_text_editor: Arc<Mutex<Option<SlotTextEditorState>>> = Arc::new(Mutex::new(None));

    let worker = {
        let app_weak = app.as_weak();
        let current_category = current_category.clone();
        let current_mode = current_mode.clone();
        let known_values = known_values.clone();
        Worker::spawn(move |response| {
            let app_weak = app_weak.clone();
            let current_category = current_category.clone();
            let current_mode = current_mode.clone();
            let known_values = known_values.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = app_weak.upgrade() else { return };
                match response {
                    Response::Rows { category, rows } => {
                        // A category switch (or a slow edit reply racing a
                        // later switch) can make this response stale;
                        // only the category currently on screen matters.
                        if category != get(&current_category) {
                            return;
                        }
                        {
                            let mut kv = known_values.lock().unwrap();
                            for row in &rows {
                                if let Some(v) = &row.value {
                                    kv.insert(row.attr.to_string(), v.clone());
                                }
                            }
                        }
                        load_rows(&app, &rows);
                    }
                    Response::RowUpdated { category, row, error } => {
                        // Same staleness guard as `Rows`: a reply for a
                        // category the user has since navigated away from
                        // should not touch what is on screen now.
                        if category != get(&current_category) {
                            return;
                        }
                        if let Some(v) = &row.value {
                            known_values.lock().unwrap().insert(row.attr.to_string(), v.clone());
                        }
                        update_row(&app, &row, error.as_deref());
                    }
                    Response::Info(info) => {
                        set(&current_mode, info.mode);
                        app.set_no_wheel(false);
                        app.set_no_wheel_message("".into());
                        app.set_mode_onboard(matches!(info.mode, Mode::Onboard));
                    }
                    Response::NoWheel(message) => {
                        app.set_no_wheel(true);
                        app.set_no_wheel_message(message.into());
                    }
                }
            });
        })
    };

    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        let app_weak = app.as_weak();
        app.on_select_category(move |index| {
            let cat = bridge::category_at(index);
            set(&current_category, cat);
            if let Some(app) = app_weak.upgrade() {
                app.set_selected_category(bridge::index_of(cat));
                app.set_category_label(cat.label().into());
            }
            worker.request(Request::LoadCategory(cat));
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_retry_discover(move || {
            worker.request(Request::Discover);
            worker.request(Request::LoadCategory(get(&current_category)));
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_refresh(move || {
            worker.request(Request::Refresh(get(&current_category)));
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        let current_mode = current_mode.clone();
        app.on_toggle_mode(move || {
            let target = match get(&current_mode) {
                Mode::Desktop => Mode::Onboard,
                Mode::Onboard => Mode::Desktop,
            };
            worker.request(Request::SetMode(target));
            worker.request(Request::Refresh(get(&current_category)));
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_edit_slider(move |attr, value| {
            worker.request(Request::Edit {
                category: get(&current_category),
                attr: attr.to_string(),
                input: WidgetInput::Slider(i64::from(value)),
            });
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_edit_choice(move |attr, index| {
            worker.request(Request::Edit {
                category: get(&current_category),
                attr: attr.to_string(),
                input: WidgetInput::Choice(index.max(0) as usize),
            });
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_edit_switch(move |attr, value| {
            worker.request(Request::Edit {
                category: get(&current_category),
                attr: attr.to_string(),
                input: WidgetInput::Switch(value),
            });
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_edit_text(move |attr, text| {
            worker.request(Request::Edit {
                category: get(&current_category),
                attr: attr.to_string(),
                input: WidgetInput::Text(text.to_string()),
            });
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_trigger(move |attr| {
            worker.request(Request::Edit {
                category: get(&current_category),
                attr: attr.to_string(),
                input: WidgetInput::Trigger,
            });
        });
    }
    {
        let worker = worker.clone();
        let current_category = current_category.clone();
        app.on_edit_pair(move |attr, lower, upper| {
            worker.request(Request::Edit {
                category: get(&current_category),
                attr: attr.to_string(),
                input: WidgetInput::Pair(lower.clamp(0, 255) as u8, upper.clamp(0, 255) as u8),
            });
        });
    }

    {
        let known_values = known_values.clone();
        let curve_editor = curve_editor.clone();
        let current_category = current_category.clone();
        let app_weak = app.as_weak();
        app.on_edit_curve(move |attr| {
            let Some(app) = app_weak.upgrade() else { return };
            let attr = attr.to_string();
            let value = known_values.lock().unwrap().get(&attr).cloned().unwrap_or(Value::Curve(vec![]));
            let curve = Curve::from_value(&attr, &value);
            push_curve_editor(&app, &curve);
            let label = REGISTRY.iter().find(|s| s.attr == attr).map(|s| s.label).unwrap_or(attr.as_str());
            app.set_curve_label(label.into());
            app.set_curve_editor_open(true);
            *curve_editor.lock().unwrap() = Some(CurveEditorState { attr, category: get(&current_category), curve });
        });
    }
    {
        let curve_editor = curve_editor.clone();
        let app_weak = app.as_weak();
        app.on_curve_move_point(move |index, x, y| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = curve_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            bridge::apply_move_point(&mut state.curve, index.max(0) as usize, x, y);
            push_curve_editor(&app, &state.curve);
        });
    }
    {
        let curve_editor = curve_editor.clone();
        let app_weak = app.as_weak();
        app.on_curve_add_point(move |x| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = curve_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            bridge::apply_add_point(&mut state.curve, x);
            push_curve_editor(&app, &state.curve);
        });
    }
    {
        let curve_editor = curve_editor.clone();
        let app_weak = app.as_weak();
        app.on_curve_remove_point(move |index| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = curve_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            state.curve.remove_point(index.max(0) as usize);
            push_curve_editor(&app, &state.curve);
        });
    }
    {
        let curve_editor = curve_editor.clone();
        let app_weak = app.as_weak();
        app.on_curve_set_lower_deadzone(move |v| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = curve_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            bridge::apply_lower_deadzone(&mut state.curve, v);
            push_curve_editor(&app, &state.curve);
        });
    }
    {
        let curve_editor = curve_editor.clone();
        let app_weak = app.as_weak();
        app.on_curve_set_upper_deadzone(move |v| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = curve_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            bridge::apply_upper_deadzone(&mut state.curve, v);
            push_curve_editor(&app, &state.curve);
        });
    }
    {
        let worker = worker.clone();
        let curve_editor = curve_editor.clone();
        let app_weak = app.as_weak();
        app.on_curve_commit(move || {
            if let Some(state) = curve_editor.lock().unwrap().take() {
                worker.request(Request::Edit {
                    category: state.category,
                    attr: state.attr,
                    input: WidgetInput::Curve(state.curve),
                });
            }
            if let Some(app) = app_weak.upgrade() {
                app.set_curve_editor_open(false);
            }
        });
    }
    {
        let curve_editor = curve_editor.clone();
        let app_weak = app.as_weak();
        app.on_curve_cancel(move || {
            *curve_editor.lock().unwrap() = None;
            if let Some(app) = app_weak.upgrade() {
                app.set_curve_editor_open(false);
            }
        });
    }

    {
        let known_values = known_values.clone();
        let rgb_editor = rgb_editor.clone();
        let current_category = current_category.clone();
        let app_weak = app.as_weak();
        app.on_edit_rgb(move |attr| {
            let Some(app) = app_weak.upgrade() else { return };
            let attr = attr.to_string();
            let colors = match known_values.lock().unwrap().get(&attr) {
                Some(Value::Rgb(cs)) => cs.clone(),
                _ => bridge::default_rgb(&attr),
            };
            push_rgb_editor(&app, &colors);
            app.set_rgb_selected_hex("".into());
            let label = REGISTRY.iter().find(|s| s.attr == attr).map(|s| s.label).unwrap_or(attr.as_str());
            app.set_rgb_label(label.into());
            app.set_rgb_editor_open(true);
            *rgb_editor.lock().unwrap() = Some(RgbEditorState { attr, category: get(&current_category), colors });
        });
    }
    {
        let rgb_editor = rgb_editor.clone();
        let app_weak = app.as_weak();
        app.on_rgb_select_led(move |i| {
            let Some(app) = app_weak.upgrade() else { return };
            let guard = rgb_editor.lock().unwrap();
            let Some(state) = guard.as_ref() else { return };
            if let Some(c) = state.colors.get(i.max(0) as usize) {
                app.set_rgb_selected_hex(c.to_hex().into());
            }
        });
    }
    {
        let rgb_editor = rgb_editor.clone();
        let app_weak = app.as_weak();
        app.on_rgb_set_color(move |i, r, g, b| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = rgb_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            let index = i.max(0) as usize;
            bridge::apply_set_color(&mut state.colors, index, r, g, b);
            push_rgb_editor(&app, &state.colors);
            if let Some(c) = state.colors.get(index) {
                app.set_rgb_selected_hex(c.to_hex().into());
            }
        });
    }
    {
        let rgb_editor = rgb_editor.clone();
        let app_weak = app.as_weak();
        app.on_rgb_set_hex(move |i, hex| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = rgb_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            let index = i.max(0) as usize;
            if bridge::apply_set_hex(&mut state.colors, index, &hex).is_ok() {
                push_rgb_editor(&app, &state.colors);
                if let Some(c) = state.colors.get(index) {
                    app.set_rgb_selected_hex(c.to_hex().into());
                }
            }
        });
    }
    {
        let rgb_editor = rgb_editor.clone();
        let app_weak = app.as_weak();
        app.on_rgb_apply_to_all(move |r, g, b| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = rgb_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            bridge::apply_to_all(&mut state.colors, r, g, b);
            push_rgb_editor(&app, &state.colors);
            app.set_rgb_selected_hex(
                Color { r: r.clamp(0, 255) as u8, g: g.clamp(0, 255) as u8, b: b.clamp(0, 255) as u8 }
                    .to_hex()
                    .into(),
            );
        });
    }
    {
        let worker = worker.clone();
        let rgb_editor = rgb_editor.clone();
        let app_weak = app.as_weak();
        app.on_rgb_commit(move || {
            if let Some(state) = rgb_editor.lock().unwrap().take() {
                worker.request(Request::Edit {
                    category: state.category,
                    attr: state.attr,
                    input: WidgetInput::Rgb(state.colors),
                });
            }
            if let Some(app) = app_weak.upgrade() {
                app.set_rgb_editor_open(false);
            }
        });
    }
    {
        let rgb_editor = rgb_editor.clone();
        let app_weak = app.as_weak();
        app.on_rgb_cancel(move || {
            *rgb_editor.lock().unwrap() = None;
            if let Some(app) = app_weak.upgrade() {
                app.set_rgb_editor_open(false);
            }
        });
    }

    {
        let known_values = known_values.clone();
        let slot_text_editor = slot_text_editor.clone();
        let current_category = current_category.clone();
        let app_weak = app.as_weak();
        app.on_edit_slot_text(move |attr| {
            let Some(app) = app_weak.upgrade() else { return };
            let attr = attr.to_string();
            let names = match known_values.lock().unwrap().get(&attr) {
                Some(Value::SlotNames(ns)) => ns.clone(),
                _ => bridge::default_slot_names(&attr),
            };
            push_slot_text_editor(&app, &names);
            app.set_slot_text_max_len(bridge::slot_text_max_len(&attr));
            let label = REGISTRY.iter().find(|s| s.attr == attr).map(|s| s.label).unwrap_or(attr.as_str());
            app.set_slot_text_label(label.into());
            app.set_slot_text_editor_open(true);
            *slot_text_editor.lock().unwrap() =
                Some(SlotTextEditorState { attr, category: get(&current_category), names });
        });
    }
    {
        let worker = worker.clone();
        let slot_text_editor = slot_text_editor.clone();
        let app_weak = app.as_weak();
        app.on_slot_text_set_name(move |slot, name| {
            let Some(app) = app_weak.upgrade() else { return };
            let mut guard = slot_text_editor.lock().unwrap();
            let Some(state) = guard.as_mut() else { return };
            let slot = slot.max(0) as u8;
            bridge::apply_set_slot_name(&mut state.names, slot, &name);
            push_slot_text_editor(&app, &state.names);
            // Kind::SlotText writes one slot at a time and reads back the
            // whole list, so this is the same "send an Edit, let the
            // category's next Rows response refresh everything" pattern the
            // other immediate-apply widgets (slider/choice/switch/text/
            // trigger) use, not the curve/RGB overlays' staged commit.
            worker.request(Request::Edit {
                category: state.category,
                attr: state.attr.clone(),
                input: WidgetInput::SlotText { slot, text: name.to_string() },
            });
        });
    }
    {
        let slot_text_editor = slot_text_editor.clone();
        let app_weak = app.as_weak();
        app.on_slot_text_close(move || {
            *slot_text_editor.lock().unwrap() = None;
            if let Some(app) = app_weak.upgrade() {
                app.set_slot_text_editor_open(false);
            }
        });
    }

    worker.request(Request::LoadCategory(get(&current_category)));

    app.run()
}
