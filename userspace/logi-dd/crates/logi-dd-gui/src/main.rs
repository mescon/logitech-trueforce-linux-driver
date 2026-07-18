slint::include_modules!();

mod viewmodel;

fn main() -> Result<(), slint::PlatformError> {
    let app = App::new()?;
    app.run()
}
