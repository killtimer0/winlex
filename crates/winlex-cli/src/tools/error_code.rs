use std::{ffi::OsString, fmt::Write as _, io::Write, path::PathBuf};
use winlex_core::{
    number::IntegerLiteral,
    tools::error_code::{self, Message, MessageStatus, QueryResult, Request, Scope},
};

const HELP: &str = "Usage: winlex error-code CODE [options]

Options:
  --scope auto|win32|hresult|ntstatus|errno  Query plan (default: auto)
  --module PATH             EXE, DLL or MUI resource; excludes preset scopes
  --lang auto|LANGID        Strict language ID; auto or 0 uses source defaults
  --language auto|LANGID    Alias for --lang
  --json                    Versioned machine-readable output
  --help, -h                Show this help

Codes accept -2147483648..4294967295. Signs and 0x/0o/0b prefixes are
accepted; a leading zero means octal (010 = 8). LANGID is unsigned 16-bit.
Module queries use full message IDs, preserve templates and never fall back to
system resources. Module results report the selected resource language; system
queries cannot report their automatic language choice.
CRT errno queries use the system msvcrt.dll, not UCRT or POSIX error tables.
Messages are English (0x0409); other explicit languages return no language match.
Automatic scope does not query CRT errno.
";

fn parse(args: &[OsString]) -> Result<Request, String> {
    let mut code = None;
    let mut scope = None;
    let mut language = 0;
    let mut module = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let text = arg.to_str().ok_or("argument must be valid Unicode")?;
        match text {
            "--scope" | "--lang" | "--language" | "--module" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("missing value for {text}"))?;
                if text == "--module" {
                    module = Some(PathBuf::from(value));
                    continue;
                }
                let value = value.to_str().ok_or("argument must be valid Unicode")?;
                if text == "--scope" {
                    scope = Some(value);
                } else {
                    language = if value == "auto" {
                        0
                    } else {
                        IntegerLiteral::parse(value.trim())
                            .and_then(IntegerLiteral::to_u16)
                            .map_err(|e| format!("invalid language: {e:#}"))?
                    };
                }
            }
            "--json" => {}
            _ if text.starts_with("--") => return Err(format!("unknown option {text}")),
            _ => {
                if code.is_some() {
                    return Err("error-code expects one code argument".into());
                }
                code = Some(
                    IntegerLiteral::parse(text.trim())
                        .and_then(IntegerLiteral::to_code_u32)
                        .map_err(|e| format!("invalid code: {e:#}"))?,
                );
            }
        }
    }
    let scope = match scope.unwrap_or(if module.is_some() { "module" } else { "auto" }) {
        "auto" => Scope::Auto,
        "win32" => Scope::Win32,
        "hresult" => Scope::HResult,
        "ntstatus" => Scope::NtStatus,
        "errno" => Scope::Errno,
        "module" => Scope::Module(module.take().ok_or("module scope requires --module PATH")?),
        _ => return Err("scope must be auto, win32, hresult, ntstatus, errno or module".into()),
    };
    if module.is_some() {
        return Err("--module cannot be combined with a preset --scope".into());
    }
    let request = Request {
        code: code.ok_or("missing code")?,
        scope,
        language,
    };
    request.validate().map_err(|e| e.to_string())?;
    Ok(request)
}

pub(super) fn run(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    if matches!(args, [arg] if arg == "--help" || arg == "-h") {
        return if out.write_all(HELP.as_bytes()).is_ok() {
            0
        } else {
            4
        };
    }
    let json = args.iter().any(|a| a == "--json");
    let request = match parse(args) {
        Ok(request) => request,
        Err(e) => return crate::error(out, err, json, 2, "invalid_arguments", &e),
    };
    let result = match error_code::query(&request) {
        Ok(result) => result,
        Err(e) => return crate::error(out, err, json, 3, "query_error", &format!("{e:#}")),
    };
    let failed = result.has_failures();
    let written = if json {
        serde_json::to_writer_pretty(
            &mut *out,
            &serde_json::json!({
                "schema": "winlex-cli-7",
                "ok": !failed,
                "command": "error-code",
                "result": result,
            }),
        )
        .map_err(std::io::Error::other)
        .and_then(|_| writeln!(out))
    } else {
        write!(out, "{}", render(&result))
    };
    crate::finish_output(written, err, if failed { 3 } else { 0 })
}

fn render(result: &QueryResult) -> String {
    let mut text = format!(
        "0x{:08X} = {} (signed {})\nScope: {:?}\nRequested language: 0x{:04X}{}\n",
        result.request.code,
        result.request.code,
        result.request.code as i32,
        result.request.scope,
        result.request.language,
        if result.request.language == 0 {
            " (automatic selection)"
        } else {
            " (strict)"
        }
    );
    if result.candidates.is_empty() {
        text.push_str(if result.has_failures() {
            "No confirmed candidate; one or more sources failed.\n"
        } else {
            "No message found.\n"
        });
    }
    for candidate in &result.candidates {
        let _ = writeln!(text, "\n{:?}", candidate.scheme);
        if let Some(bits) = &candidate.encoding {
            let _ = writeln!(
                text,
                "Severity: {}; customer: {}; facility: {}; code: {}; NT mapping: {}",
                bits.severity, bits.customer, bits.facility, bits.code, bits.nt_mapping
            );
        }
        render_message(&mut text, &candidate.message);
        for related in &candidate.related {
            let _ = writeln!(text, "Related Win32 code ({})", related.relationship);
            render_message(&mut text, &related.message);
        }
    }
    for attempt in &result.attempts {
        render_message(&mut text, attempt);
    }
    text
}

fn render_message(text: &mut String, message: &Message) {
    let _ = writeln!(
        text,
        "Source: {:?}; message ID: 0x{:08X} ({})",
        message.source, message.code, message.code
    );
    if message.source == error_code::Source::Msvcrt {
        text.push_str("Microsoft CRT errno (system msvcrt.dll); English messages only.\n");
    }
    if message.status == MessageStatus::Found {
        match message.actual_language {
            Some(0) => text.push_str("Actual language: neutral resource (0x0000)\n"),
            Some(id) => {
                let _ = writeln!(text, "Actual language: 0x{id:04X}");
            }
            None => text.push_str("Actual language: unknown (Windows selection)\n"),
        }
    }
    match message.status {
        MessageStatus::Found => {
            let _ = writeln!(text, "{}", message.text.as_deref().unwrap_or(""));
        }
        MessageStatus::NotFound => text.push_str("Message not found.\n"),
        MessageStatus::LanguageNotFound => {
            text.push_str("Requested language not found; retry language 0 for source defaults.\n")
        }
        MessageStatus::NoMessageTable => text.push_str("No message-table resource found.\n"),
        MessageStatus::Failed => {}
    }
    if let Some(failure) = &message.failure {
        let _ = writeln!(
            text,
            "Query failed: {} ({:?}): {}",
            failure.operation, failure.os_code, failure.detail
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn result_projects_numeric_text_from_the_request_without_json_mirrors() {
        let result = QueryResult {
            request: Request {
                code: 0x80070005,
                scope: Scope::HResult,
                language: 0x409,
            },
            candidates: vec![],
            attempts: vec![],
        };
        assert!(render(&result).starts_with("0x80070005 = 2147942405 (signed -2147024891)\n"));
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["request"]["code"], 0x80070005u32);
        for key in ["hex", "unsigned", "signed"] {
            assert!(json.get(key).is_none());
        }
    }
    #[test]
    fn scope_options_preserve_defaults_conflicts_and_diagnostic_precedence() {
        let parse_args =
            |args: &[&str]| parse(&args.iter().map(OsString::from).collect::<Vec<_>>());
        for args in [vec!["5"], vec!["5", "--scope", "auto"]] {
            assert_eq!(parse_args(&args).unwrap().scope, Scope::Auto);
        }
        for args in [
            vec!["5", "--module", "last.dll"],
            vec!["5", "--scope", "module", "--module", "last.dll"],
            vec!["5", "--module", "last.dll", "--scope", "module"],
            vec![
                "5", "--scope", "auto", "--scope", "module", "--module", "last.dll",
            ],
            vec!["5", "--module", "first.dll", "--module", "last.dll"],
        ] {
            assert_eq!(
                parse_args(&args).unwrap().scope,
                Scope::Module(PathBuf::from("last.dll"))
            );
        }
        for scope in ["auto", "win32", "hresult", "ntstatus", "errno"] {
            for args in [
                vec!["5", "--scope", scope, "--module", "test.dll"],
                vec!["5", "--module", "test.dll", "--scope", scope],
                vec![
                    "--module", "test.dll", "--scope", "module", "--scope", scope,
                ],
            ] {
                assert_eq!(
                    parse_args(&args).unwrap_err(),
                    "--module cannot be combined with a preset --scope"
                );
            }
        }
        for (args, error) in [
            (
                vec!["--scope", "invalid"],
                "scope must be auto, win32, hresult, ntstatus, errno or module",
            ),
            (
                vec!["--scope", "module"],
                "module scope requires --module PATH",
            ),
            (vec!["--module", "test.dll"], "missing code"),
        ] {
            assert_eq!(parse_args(&args).unwrap_err(), error);
        }
    }
    #[test]
    fn typed_request_without_metadata_and_checked_inputs() {
        let parse_args =
            |args: &[&str]| parse(&args.iter().map(OsString::from).collect::<Vec<_>>());
        let request =
            parse_args(&["-2147024891", "--scope", "hresult", "--language", "0x409"]).unwrap();
        assert_eq!(
            request,
            Request {
                code: 0x80070005,
                scope: Scope::HResult,
                language: 0x409
            }
        );
        assert_eq!(
            parse_args(&["0", "--module", "messages.dll", "--lang", "auto"]).unwrap(),
            Request {
                code: 0,
                scope: Scope::Module(PathBuf::from("messages.dll")),
                language: 0
            }
        );
        for args in [
            vec!["4294967296"],
            vec!["5", "--language", "-1"],
            vec!["5", "--language", "65536"],
            vec!["5", "--module", "test.dll", "--scope", "win32"],
            vec!["5", "--scope", "module"],
        ] {
            assert!(parse_args(&args).is_err());
        }
    }
}
