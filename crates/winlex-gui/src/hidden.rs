//! Hidden child mode used to activate COM objects outside the UI process.
//!
//! The parent passes `--inspect-com <GUID>` and reads one JSON object from
//! stdout. Every failure is reported in that object so this mode never shows
//! an error dialog.
use std::{
    ffi::OsString,
    io::{self, Write},
};
use winlex_core::tools::com::{self, activation};

const FLAG: &str = "--inspect-com";

pub(crate) fn requested(args: &[OsString]) -> bool {
    args.first().and_then(|value| value.to_str()) == Some(FLAG)
}

pub(crate) fn run(args: &[OsString]) {
    let payload = response(args.get(1));
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    // A closed pipe must not turn into a startup dialog in the child process.
    let _ = lock.write_all(payload.as_bytes());
    let _ = lock.write_all(b"\n");
    let _ = lock.flush();
}

fn response(guid: Option<&OsString>) -> String {
    let Some(guid) = guid.and_then(|value| value.to_str()) else {
        return error("The GUID argument is missing.");
    };
    let Some(guid) = com::Guid::parse(guid) else {
        return error("The GUID argument is invalid.");
    };
    match activation::inspect(guid) {
        Ok(instance) => serde_json::json!({ "ok": true, "instance": instance }).to_string(),
        Err(reason) => error(&reason.to_string()),
    }
}

fn error(message: &str) -> String {
    serde_json::json!({ "ok": false, "error": message }).to_string()
}
