use std::{ffi::OsString, io::Write};
mod error_code;

pub(super) fn run(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> Option<u8> {
    match args.first().and_then(|arg| arg.to_str()) {
        Some("error-code") => Some(error_code::run(&args[1..], out, err)),
        _ => None,
    }
}
