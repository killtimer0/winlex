use std::{ffi::OsString, io::Write};
mod com;
mod demangle;
mod error_code;

pub(super) fn run(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> Option<u8> {
    match args.first().and_then(|arg| arg.to_str()) {
        Some("com") => Some(com::run(&args[1..], out, err)),
        Some("demangle") => Some(demangle::run(&args[1..], out, err)),
        Some("error-code") => Some(error_code::run(&args[1..], out, err)),
        _ => None,
    }
}
