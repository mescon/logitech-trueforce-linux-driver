//! Command-line front end.
use std::process::ExitCode;

pub fn dispatch(args: &[String]) -> crate::Result<ExitCode> {
    let _ = args;
    eprintln!("usage: logi-dd-ffb run -- <game command>\n       logi-dd-ffb daemon");
    Ok(ExitCode::SUCCESS)
}
