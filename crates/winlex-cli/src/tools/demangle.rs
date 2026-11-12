use crate::SCHEMA;
use std::{ffi::OsString, fmt::Write as _, io::Write, path::PathBuf};
use winlex_core::tools::demangle::{self, Abi, Demangled, SymbolKind};

const HELP: &str = "Usage: winlex demangle SYMBOL... [options]

Options:
  --abi auto|msvc|itanium  Symbol ABI (default: auto)
  --file PATH              Read symbols from a UTF-8 file, one per line
  --json                   Versioned machine-readable output
  --help, -h               Show this help

AUTO detects '?' as Visual C++, '_Z' as Itanium, and '_name@N' or '@name@N'
as x86 C decorations. Rust v0 symbols are not supported.
";

#[derive(Debug)]
struct Options {
    abi: Abi,
    symbols: Vec<String>,
    file: Option<PathBuf>,
}

fn parse(args: &[OsString]) -> Result<Options, String> {
    let mut options = Options {
        abi: Abi::Auto,
        symbols: Vec::new(),
        file: None,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let text = arg.to_str().ok_or("argument must be valid Unicode")?;
        match text {
            "--abi" => {
                let value = args
                    .next()
                    .ok_or("missing value for --abi")?
                    .to_str()
                    .ok_or("argument must be valid Unicode")?;
                options.abi = Abi::parse(value).ok_or("--abi must be auto, msvc or itanium")?;
            }
            "--file" => {
                let value = args.next().ok_or("missing value for --file")?;
                options.file = Some(PathBuf::from(value));
            }
            "--json" => {}
            _ if text.starts_with("--") => return Err(format!("unknown option {text}")),
            _ => options.symbols.push(text.to_owned()),
        }
    }
    Ok(options)
}

fn symbols(options: &Options) -> Result<Vec<String>, String> {
    let mut symbols = Vec::new();
    if let Some(path) = &options.file {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        symbols.extend(
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned),
        );
    }
    symbols.extend(options.symbols.iter().cloned());
    if symbols.is_empty() {
        return Err("missing symbol".into());
    }
    Ok(symbols)
}

type Line = Result<Demangled, (String, String)>;

pub(super) fn run(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    if let Some(code) = crate::help(args, out, HELP) {
        return code;
    }
    let json = args.iter().any(|arg| arg == "--json");
    let options = match parse(args).and_then(|options| {
        let symbols = symbols(&options)?;
        Ok((options, symbols))
    }) {
        Ok(value) => value,
        Err(error) => return crate::error(out, err, json, 2, "invalid_arguments", &error),
    };
    let (options, symbols) = options;
    let mut lines = Vec::with_capacity(symbols.len());
    let mut failed = false;
    for symbol in &symbols {
        match demangle::demangle(symbol, options.abi) {
            Ok(result) => lines.push(Ok(result)),
            Err(error) => {
                failed = true;
                lines.push(Err((symbol.clone(), format!("{error:#}"))));
            }
        }
    }
    let written = if json {
        let results: Vec<_> = lines
            .iter()
            .map(|line| match line {
                Ok(result) => serde_json::json!({
                    "symbol": result.symbol,
                    "ok": true,
                    "result": result,
                }),
                Err((symbol, error)) => serde_json::json!({
                    "symbol": symbol,
                    "ok": false,
                    "error": error,
                }),
            })
            .collect();
        crate::write_json(
            out,
            &serde_json::json!({
                "schema": SCHEMA,
                "ok": !failed,
                "command": "demangle",
                "results": results,
            }),
        )
    } else {
        write!(out, "{}", render(&lines))
    };
    crate::finish_output(written, err, if failed { 3 } else { 0 })
}

fn render(lines: &[Line]) -> String {
    let mut text = String::new();
    for line in lines {
        match line {
            Ok(result) => {
                let mut meta = format!("{}, {}", result.abi.name(), kind_name(result.kind));
                if let Some(convention) = result.convention {
                    meta.push_str(", ");
                    meta.push_str(convention.name());
                }
                let _ = writeln!(text, "{} -> {} [{}]", result.symbol, result.text, meta);
            }
            Err((symbol, error)) => {
                let _ = writeln!(text, "{symbol}: {error}");
            }
        }
    }
    text
}

fn kind_name(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "function",
        SymbolKind::Data => "data",
        SymbolKind::Special => "special",
        SymbolKind::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Options, String> {
        parse(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn abi_defaults_and_rejects_unknown_values() {
        assert_eq!(parse_args(&["?x@@YAHXZ"]).unwrap().abi, Abi::Auto);
        assert_eq!(
            parse_args(&["_Z3fooi", "--abi", "itanium"]).unwrap().abi,
            Abi::Itanium
        );
        assert_eq!(
            parse_args(&["?x@@YAHXZ", "--abi", "msvc"]).unwrap().abi,
            Abi::Msvc
        );
        assert_eq!(
            parse_args(&["?x@@YAHXZ", "--abi", "wat"]).unwrap_err(),
            "--abi must be auto, msvc or itanium"
        );
    }

    #[test]
    fn symbols_keep_argument_order_and_require_input() {
        let options = parse_args(&["_Z3fooi", "?x@@YAHXZ"]).unwrap();
        assert_eq!(symbols(&options).unwrap(), ["_Z3fooi", "?x@@YAHXZ"]);
        assert_eq!(
            symbols(&parse_args(&[]).unwrap()).unwrap_err(),
            "missing symbol"
        );
    }

    #[test]
    fn text_output_pairs_each_symbol_with_metadata_and_errors() {
        let lines = vec![
            Ok(demangle::demangle("?foo@@YAHH@Z", Abi::Auto).unwrap()),
            Err(("?broken".into(), "invalid symbol".into())),
        ];
        let text = render(&lines);
        assert_eq!(text.lines().count(), 2);
        assert!(text.contains("?foo@@YAHH@Z -> int __cdecl foo(int) [msvc, function, cdecl]"));
        assert!(text.contains("?broken: invalid symbol"));
    }

    #[test]
    fn json_output_carries_spans_and_per_symbol_status() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            &["_ZN5space3fooEibc", "?", "--json"]
                .iter()
                .map(OsString::from)
                .collect::<Vec<_>>(),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 3);
        assert!(err.is_empty());
        let response: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(response["schema"], SCHEMA);
        assert_eq!(response["command"], "demangle");
        assert_eq!(response["ok"], false);
        assert_eq!(response["results"][0]["ok"], true);
        assert_eq!(response["results"][0]["result"]["abi"], "itanium");
        assert_eq!(
            response["results"][0]["result"]["spans"],
            serde_json::json!([{ "start": 0, "end": 10 }])
        );
        assert_eq!(response["results"][1]["ok"], false);
    }
}
