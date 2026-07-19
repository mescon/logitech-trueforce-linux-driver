fn main() {
    // Build with the Fluent widget style so the app looks the same on every
    // distribution regardless of the builder's environment. A packager who
    // wants a different look can change the style string here.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).unwrap();
}
