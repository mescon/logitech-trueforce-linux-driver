//! Command-line front end.
use std::process::ExitCode;

pub fn dispatch(args: &[String]) -> crate::Result<ExitCode> {
    let _ = args;
    eprintln!("usage: logi-ffb <game command>\n       logi-ffb --daemon");
    Ok(ExitCode::SUCCESS)
}
