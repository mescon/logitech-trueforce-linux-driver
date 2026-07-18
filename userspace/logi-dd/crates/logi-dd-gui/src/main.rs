slint::include_modules!();

mod bridge;
mod viewmodel;
mod worker;

use logi_dd_core::Category;
use viewmodel::WidgetInput;
use worker::{Request, Response, Worker};

// Only the Force feedback category is wired to live widgets so far; later
// tasks add a category picker and the rest of the registry's categories.
const CATEGORY: Category = Category::Ffb;

fn main() -> Result<(), slint::PlatformError> {
    let app = App::new()?;
    app.set_category_label(CATEGORY.label().into());

    let worker = {
        let app_weak = app.as_weak();
        Worker::spawn(move |response| {
            let app_weak = app_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = app_weak.upgrade() else { return };
                match response {
                    Response::Rows { category, rows, edit_error } => {
                        // Only one category is on screen right now; drop a
                        // response for any other one rather than let it
                        // clobber what is currently shown (matters once a
                        // category switch can race a slow load).
                        if category != CATEGORY {
                            return;
                        }
                        let err = edit_error.as_ref().map(|(a, m)| (a.as_str(), m.as_str()));
                        app.set_rows(bridge::rows_model(&rows, err));
                    }
                    Response::Error(message) => {
                        // No wheel reachable; nothing to render. Logged so a
                        // run from a terminal shows why the list is empty.
                        eprintln!("logi-dd-gui: {message}");
                    }
                }
            });
        })
    };

    {
        let worker = worker.clone();
        app.on_edit_slider(move |attr, value| {
            worker.request(Request::Edit {
                category: CATEGORY,
                attr: attr.to_string(),
                input: WidgetInput::Slider(i64::from(value)),
            });
        });
    }
    {
        let worker = worker.clone();
        app.on_edit_choice(move |attr, index| {
            worker.request(Request::Edit {
                category: CATEGORY,
                attr: attr.to_string(),
                input: WidgetInput::Choice(index.max(0) as usize),
            });
        });
    }
    {
        let worker = worker.clone();
        app.on_edit_switch(move |attr, value| {
            worker.request(Request::Edit {
                category: CATEGORY,
                attr: attr.to_string(),
                input: WidgetInput::Switch(value),
            });
        });
    }
    {
        let worker = worker.clone();
        app.on_edit_text(move |attr, text| {
            worker.request(Request::Edit {
                category: CATEGORY,
                attr: attr.to_string(),
                input: WidgetInput::Text(text.to_string()),
            });
        });
    }
    {
        let worker = worker.clone();
        app.on_trigger(move |attr| {
            worker.request(Request::Edit {
                category: CATEGORY,
                attr: attr.to_string(),
                input: WidgetInput::Trigger,
            });
        });
    }

    worker.request(Request::LoadCategory(CATEGORY));

    app.run()
}
