slint::include_modules!();

mod bridge;
mod viewmodel;
mod worker;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use logi_dd_core::curve::Curve;
use logi_dd_core::{Category, Color, Mode, Value, REGISTRY};
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

/// Read the `Category`/`Mode` out of one of the `Arc<Mutex<_>>` cells below.
/// Both are `Copy`, so the lock never needs to outlive this call.
fn get<T: Copy>(cell: &Arc<Mutex<T>>) -> T {
    *cell.lock().unwrap()
}

fn set<T>(cell: &Arc<Mutex<T>>, value: T) {
    *cell.lock().unwrap() = value;
}

fn main() -> Result<(), slint::PlatformError> {
    let app = App::new()?;
    app.set_category_labels(bridge::category_labels_model());

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
                    Response::Rows { category, rows, edit_error } => {
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
                        let err = edit_error.as_ref().map(|(a, m)| (a.as_str(), m.as_str()));
                        app.set_rows(bridge::rows_model(&rows, err));
                    }
                    Response::Info(info) => {
                        set(&current_mode, info.mode);
                        app.set_no_wheel(false);
                        app.set_no_wheel_message("".into());
                        let (serial, firmware, onboard) = bridge::header_fields(&info);
                        app.set_device_serial(serial.into());
                        app.set_device_firmware(firmware.into());
                        app.set_mode_onboard(onboard);
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

    worker.request(Request::LoadCategory(get(&current_category)));

    app.run()
}
