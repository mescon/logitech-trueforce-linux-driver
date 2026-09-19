// SPDX-License-Identifier: GPL-2.0-only
use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("{}", logi_build_id::banner("logi-ffb"));
        return ExitCode::SUCCESS;
    }

    let args: Vec<String> = std::env::args().collect();
    match logi_ffb::cli::dispatch(&args) {
        Ok(code) => code,
        Err(e) => {
            logi_ffb::note(&format!("logi-ffb: {e}"));
            ExitCode::FAILURE
        }
    }
}
