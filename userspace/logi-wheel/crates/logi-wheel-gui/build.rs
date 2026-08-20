fn main() {
    // Build with the Fluent widget style so the app looks the same on every
    // distribution regardless of the builder's environment. A packager who
    // wants a different look can change the style string here.
    // Send every call to these two through our own forwarders, which ask
    // for the old version of each symbol. Built on a rolling distribution
    // they otherwise bind to a glibc newer than a frozen one has, and the
    // binary will not start there at all (SteamOS, issue #68). See
    // src/glibc_compat.rs; tools/check-glibc-floor.sh fails the build if
    // another symbol ever does the same thing.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu")
    {
        println!("cargo:rustc-link-arg=-Wl,--wrap=acosf");
        println!("cargo:rustc-link-arg=-Wl,--wrap=atan2f");
    }

    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).unwrap();
}
