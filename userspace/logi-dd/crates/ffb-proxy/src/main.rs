use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("logi-ffb {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let args: Vec<String> = std::env::args().collect();
    match ffb_proxy::cli::dispatch(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("logi-ffb: {e}");
            ExitCode::FAILURE
        }
    }
}
