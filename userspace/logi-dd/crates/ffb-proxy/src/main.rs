use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match ffb_proxy::cli::dispatch(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("logi-ffb: {e}");
            ExitCode::FAILURE
        }
    }
}
