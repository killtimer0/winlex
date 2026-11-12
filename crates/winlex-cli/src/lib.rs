//! CLI adapter; no process exit, global output or terminal dependency in the core.
use serde_json::Value;
use std::{ffi::OsString, io::Write, path::PathBuf};
use winlex_core::{Architecture, Catalog, SearchMode, SearchOptions};

pub(crate) const SCHEMA: &str = "winlex-cli-1";
pub const HELP: &str = r#"Winlex - offline Win32 metadata queries

Usage: winlex FILE... COMMAND [QUERY] [options]

Commands:
  summary                 Sources, assembly identities and row counts
  search TEXT             Quick name, constant value and GUID search
  search --where EXPR     Conditional search with AND, OR, NOT and parentheses
  show NAME|ID            Declaration, members and typed references
  layout TYPE             Compute size/alignment/offsets (bits)
  vtable INTERFACE        Compute inherited method slots
  iid INTERFACE           WinRT interface/delegate instance IID
  references TYPE         Reverse type uses across loaded sources
  error-code CODE         Query Windows messages (no input file required)
  demangle SYMBOL...      Undecorate Visual C++ and Itanium symbols
  com TARGET              Resolve COM/WinRT registrations from this system

Options:
  --source SOURCE         Zero-based input index or unique assembly name
  --arch x86|x64|arm64     Required for layout/vtable/iid/references and instances
  --json                  Versioned machine-readable output
  --name                  Exact name or trailing '.'/'+' scope search
  --value                 Integer bits or target-type floating equality
  --guid                  GUID text fragment or byte sequence
  --limit N               Page size, 0..1000 (default: 50)
  --offset N              Matching rows to skip
  --help, --version       No input required

Input files precede the command. Use .\search for a file named search.
Tools: winlex error-code --help, winlex demangle --help, winlex com --help
Declaration selectors accept SOURCE::NAME or SOURCE::0xTOKEN.
Closed types use NAME<ARG,...>, for example 'Windows.Foundation.Collections.IVector<int32>'.
Instance show and iid require --arch. Arguments may be nested or SOURCE:: qualified.
Bare tokens require a single input source. Names can be namespace-qualified.
Source indices follow input order and belong to this catalog only.
Search modes and --where are mutually exclusive.
Name queries containing '.' or '+' match qualified names; otherwise short names.
Integers accept signs and 0b/leading-zero/0o/decimal/0x notation, using width-local bits.
Decimal points/exponents select floating equality: 1.5 and 1.50 are equivalent.
Special floats: nan (any NaN), inf, +inf, -inf; NaN cannot be an ordered bound.
Conditions: name contains "access" and value = 5
            value >= 1.45 and value < 1.55
Fields: name, fullname, value, guid, namespace, category, source, architecture.
Text operators: =, contains, starts_with; name/fullname also support fuzzy.
namespace within "Windows.Win32" includes child namespaces.
source = 0 selects an input ID; architecture = "x86" means applicable to x86.
Numeric comparisons: =, <, <=, >, >=. Integers and floats remain distinct.
Quoted values are strings; escapes: \" \\ \n \r \t \0. Keywords ignore ASCII case.
Missing fields do not match a predicate; NOT also includes entries missing that field.
GUID bytes: 239,254 (decimal), FF 0A (hex), or 0xBC,0xAB (explicit hex).
"#;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Summary,
    Search,
    Show,
    Layout,
    Vtable,
    Iid,
    References,
}
impl Command {
    const NAMES: &[(Self, &str)] = &[
        (Self::Summary, "summary"),
        (Self::Search, "search"),
        (Self::Show, "show"),
        (Self::Layout, "layout"),
        (Self::Vtable, "vtable"),
        (Self::Iid, "iid"),
        (Self::References, "references"),
    ];

    fn as_str(self) -> &'static str {
        Self::NAMES.iter().find(|(c, _)| *c == self).unwrap().1
    }
    fn parse(value: &str) -> Option<Self> {
        Self::NAMES
            .iter()
            .find(|(_, name)| *name == value)
            .map(|(c, _)| *c)
    }
}
struct Options {
    command: Command,
    query: Option<String>,
    paths: Vec<PathBuf>,
    source: Option<String>,
    arch: Option<Architecture>,
    json: bool,
    mode: SearchMode,
    limit: usize,
    offset: usize,
}
fn parse(args: &[OsString]) -> Result<Options, String> {
    let mut paths = vec![];
    let mut source = None;
    let mut arch = None;
    let mut json = false;
    let mut mode = None;
    let mut limit = 50;
    let mut offset = 0;
    let mut positional = vec![];
    let mut expression = None;
    let mut args = args.iter();
    let text = |s: &OsString| {
        s.to_str()
            .map(str::to_owned)
            .ok_or("argument must be valid Unicode".to_owned())
    };
    while let Some(arg) = args.next() {
        let a = arg.to_str().unwrap_or("");
        match a {
            "--source" | "--arch" | "--limit" | "--offset" => {
                let v = text(
                    args.next()
                        .ok_or_else(|| format!("missing value for {a}"))?,
                )?;
                match a {
                    "--source" => source = Some(v),
                    "--arch" => {
                        arch = Some(v.parse().map_err(|e: winlex_core::Error| e.to_string())?)
                    }
                    "--limit" => limit = v.parse().map_err(|_| "invalid limit")?,
                    _ => offset = v.parse().map_err(|_| "invalid offset")?,
                }
            }
            "--json" => json = true,
            "--name" | "--value" | "--guid" | "--where" => {
                if mode.is_some() {
                    return Err("choose only one search mode".into());
                }
                mode = Some(match a {
                    "--name" => SearchMode::Name,
                    "--value" => SearchMode::Value,
                    "--guid" => SearchMode::Guid,
                    _ => {
                        expression = Some(text(args.next().ok_or("missing value for --where")?)?);
                        SearchMode::Condition
                    }
                });
            }
            _ if a.starts_with("--") => return Err(format!("unknown option {a}")),
            _ if positional.is_empty() && Command::parse(a).is_none() => {
                paths.push(PathBuf::from(arg))
            }
            _ => positional.push(text(arg)?),
        }
    }
    if limit > 1000 {
        return Err("limit must be <= 1000".into());
    }
    let name = positional.first().ok_or("missing command")?;
    let command = Command::parse(name).ok_or("missing command")?;
    let count = if command == Command::Summary || expression.is_some() {
        1
    } else {
        2
    };
    if positional.len() != count {
        return Err(format!("{name} expects {} query argument(s)", count - 1));
    }
    if mode.is_some() && command != Command::Search {
        return Err("--name/--value/--guid/--where apply only to search".into());
    }
    if matches!(
        command,
        Command::Layout | Command::Vtable | Command::References | Command::Iid
    ) && arch.is_none()
    {
        return Err(format!("{name} requires --arch"));
    }
    if command == Command::Show
        && arch.is_none()
        && positional.get(1).is_some_and(|q| q.contains('<'))
    {
        return Err("instance show requires --arch".into());
    }
    if source.is_some()
        && (command == Command::Summary
            || positional
                .get(1)
                .is_some_and(|q| q.split('<').next().is_some_and(|root| root.contains("::"))))
    {
        return Err(
            "--source applies to queries; do not combine it with SOURCE:: selectors".into(),
        );
    }
    if paths.is_empty() {
        return Err("at least one input file is required before the command".into());
    }
    Ok(Options {
        command,
        query: expression.or_else(|| positional.get(1).cloned()),
        paths,
        source,
        arch,
        json,
        mode: mode.unwrap_or(SearchMode::Combined),
        limit,
        offset,
    })
}

/// Returns an exit code (0 success, 2 syntax, 3 input/query error, 4 output error).
/// Tests and future embedded callers can pass their own writers.
pub fn run(
    args: impl IntoIterator<Item = OsString>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let args: Vec<_> = args.into_iter().collect();
    if let Some(exit) = tools::run(&args, out, err) {
        return exit;
    }
    let want_json = args.iter().any(|a| a == "--json");
    let immediate = match args.as_slice() {
        [] => Some(HELP),
        [arg] if arg == "--help" || arg == "-h" => Some(HELP),
        [arg] if arg == "--version" || arg == "-V" => {
            Some(concat!("winlex ", env!("CARGO_PKG_VERSION"), "\n"))
        }
        _ => None,
    };
    if let Some(text) = immediate {
        return emit(out, text);
    }
    let o = match parse(&args) {
        Ok(v) => v,
        Err(e) => return error(out, err, want_json, 2, "invalid_arguments", &e),
    };
    let db = match Catalog::open_many(&o.paths) {
        Ok(v) => v,
        Err(e) => {
            return error(
                out,
                err,
                o.json,
                3,
                "input_error",
                &format!("{:?}: {e}", o.paths),
            );
        }
    };
    let q = o.query.as_deref().unwrap_or("");
    let selector = o.source.as_ref().map(|s| format!("{s}::{q}"));
    let selector = selector.as_deref().unwrap_or(q);
    let result: winlex_core::Result<Query> = (|| {
        Ok(match o.command {
            Command::Summary => Query::Summary,
            Command::Search => {
                let options = SearchOptions {
                    source: o
                        .source
                        .as_deref()
                        .map(|s| db.source_index(s))
                        .transpose()?,
                    mode: o.mode,
                    architecture: o.arch,
                    ..Default::default()
                };
                let matches = db.search(q, options)?;
                let mut matches = matches.collect::<winlex_core::Result<Vec<_>>>()?;
                matches.sort_unstable_by(winlex_core::SearchMatch::compare);
                let result = page(matches.into_iter().map(Ok), o.offset, o.limit)?;
                Query::Matches(result)
            }
            Command::Show => Query::Show(select_target(&db, selector, o.arch, false)?),
            Command::Layout => Query::Layout(
                db.layout(db.select_entity(selector, o.arch, true)?, o.arch.unwrap())?,
            ),
            Command::Vtable => Query::Vtable(db.vtable_target(
                &select_target(&db, selector, o.arch, true)?,
                o.arch.unwrap(),
            )?),
            Command::Iid => Query::Iid(db.interface_identity(
                &select_target(&db, selector, o.arch, true)?,
                o.arch.unwrap(),
            )?),
            Command::References => {
                let target = db.select_entity(selector, o.arch, true)?;
                let arch = o.arch.unwrap();
                let mut results = db.references(target, arch, None)?;
                let page = page(results.by_ref(), o.offset, o.limit)?;
                Query::References(target, page, results.unresolved_type_uses())
            }
        })
    })();
    let result = match result {
        Ok(r) => r,
        Err(e) => {
            return error_detail(
                out,
                err,
                o.json,
                3,
                "query_error",
                &format!("{e:#}"),
                e.downcast_ref::<winlex_core::QueryError>(),
            );
        }
    };
    let written = if o.json {
        let result = match result.json(&db, o.arch) {
            Ok(result) => result,
            Err(e) => return error(out, err, true, 3, "query_error", &e.to_string()),
        };
        let response = serde_json::json!({
            "schema": SCHEMA,
            "ok": true,
            "sources": db.sources(),
            "command": o.command.as_str(),
            "result": result,
        });
        write_json(out, &response)
    } else {
        match text::render(&result, &db, o.arch) {
            Ok(text) => write!(out, "{text}"),
            Err(e) => return error(out, err, false, 3, "query_error", &e.to_string()),
        }
    };
    finish_output(written, err, 0)
}

/// Handle a lone `--help`/`-h` before option parsing; `None` continues normally.
fn help(args: &[OsString], out: &mut dyn Write, text: &str) -> Option<u8> {
    matches!(args, [arg] if arg == "--help" || arg == "-h").then(|| emit(out, text))
}

fn emit(out: &mut dyn Write, text: &str) -> u8 {
    if out.write_all(text.as_bytes()).is_ok() {
        0
    } else {
        4
    }
}

fn write_json(out: &mut dyn Write, value: &Value) -> std::io::Result<()> {
    serde_json::to_writer_pretty(&mut *out, value)
        .map_err(std::io::Error::other)
        .and_then(|_| writeln!(out))
}

fn finish_output(written: std::io::Result<()>, err: &mut dyn Write, exit: u8) -> u8 {
    if let Err(e) = written
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        let _ = writeln!(err, "output error: {e}");
        return 4;
    }
    exit
}

mod text;
mod tools;
#[derive(serde::Serialize)]
struct Page {
    total: usize,
    offset: usize,
    limit: usize,
    matches: Vec<winlex_core::SearchMatch>,
}
enum Query {
    Summary,
    Matches(Page),
    Show(winlex_core::DeclarationTarget),
    Iid(winlex_core::InterfaceIdentity),
    Layout(winlex_core::Layout),
    Vtable(winlex_core::Vtable),
    References(winlex_core::EntityId, Page, usize),
}
impl Query {
    fn json(&self, db: &Catalog, arch: Option<Architecture>) -> winlex_core::Result<Value> {
        Ok(match self {
            Self::Summary => winlex_format::summary(db),
            Self::Matches(p) => serde_json::to_value(p)?,
            Self::Show(target) => winlex_format::describe_target(db, target, arch)?,
            Self::Iid(identity) => serde_json::to_value(identity)?,
            Self::Layout(l) => winlex_format::layout(l),
            Self::Vtable(v) => winlex_format::vtable(v),
            Self::References(id, p, unresolved) => serde_json::json!({
                "target_id": id,
                "architecture": arch.unwrap(),
                "page": p,
                "unresolved_type_uses": unresolved,
                "scope": "field/method/property/event signatures and interface requirements",
            }),
        })
    }
}
fn page(
    items: impl IntoIterator<Item = winlex_core::Result<winlex_core::SearchMatch>>,
    offset: usize,
    limit: usize,
) -> winlex_core::Result<Page> {
    let mut total = 0;
    let mut matches = Vec::new();
    for item in items {
        let item = item?;
        if total >= offset && matches.len() < limit {
            matches.push(item);
        }
        total += 1;
    }
    Ok(Page {
        total,
        offset,
        limit,
        matches,
    })
}
/// Resolve the CLI's source/name/token syntax to a catalog-local identity.
fn select_target(
    db: &Catalog,
    expression: &str,
    arch: Option<Architecture>,
    types_only: bool,
) -> winlex_core::Result<winlex_core::DeclarationTarget> {
    if expression.contains('<') {
        db.parse_target(
            expression,
            arch.ok_or_else(|| winlex_core::Error::msg("instance query requires --arch"))?,
        )
    } else {
        Ok(db.select_entity(expression, arch, types_only)?.into())
    }
}

fn error(
    out: &mut dyn Write,
    err: &mut dyn Write,
    json_mode: bool,
    exit: u8,
    code: &str,
    message: &str,
) -> u8 {
    error_detail(out, err, json_mode, exit, code, message, None)
}
fn error_detail(
    out: &mut dyn Write,
    err: &mut dyn Write,
    json_mode: bool,
    exit: u8,
    code: &str,
    message: &str,
    query: Option<&winlex_core::QueryError>,
) -> u8 {
    let result = if json_mode {
        let mut detail = serde_json::json!({
            "code": code,
            "message": message,
        });
        if let Some(query) = query {
            detail["span"] = serde_json::json!({
                "start": query.span.start,
                "end": query.span.end,
            });
            detail["incomplete"] = query.incomplete.into();
        }
        serde_json::to_writer(
            &mut *out,
            &serde_json::json!({
                "schema": SCHEMA,
                "ok": false,
                "error": detail,
            }),
        )
        .map_err(std::io::Error::other)
        .and_then(|_| writeln!(out))
    } else {
        writeln!(err, "{code}: {message}")
    };
    if result.is_err() { 4 } else { exit }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate as winlex_cli;

    include!("../../../tests/rust_metadata.rs");

    include!("../tests/support/invoke.rs");

    #[test]
    fn immediate_output_keeps_aliases_and_requires_a_standalone_option() {
        for args in [vec![], vec!["--help"], vec!["-h"]] {
            assert_eq!(
                invoke(args.into_iter().map(Into::into)),
                (0, HELP.into(), String::new())
            );
        }
        for option in ["--version", "-V"] {
            assert_eq!(
                invoke([option.into()]),
                (
                    0,
                    format!("winlex {}\n", env!("CARGO_PKG_VERSION")),
                    String::new()
                )
            );
        }
        for option in ["--help", "-h", "--version", "-V"] {
            assert_eq!(invoke([option.into(), "--json".into()]).0, 2);
        }
    }

    #[test]
    fn search_mode_options_remain_mutually_exclusive_in_both_orders() {
        for first in ["--name", "--value", "--guid", "--where"] {
            for second in ["--name", "--value", "--guid", "--where"] {
                let mut args = vec!["missing.winmd", "search", first];
                if first == "--where" {
                    args.push("name = \"X\"");
                }
                args.push(second);
                let (exit, out, err) = invoke(args.into_iter().map(Into::into));
                assert_eq!(exit, 2, "{first} {second}");
                assert!(out.is_empty());
                assert_eq!(err, "invalid_arguments: choose only one search mode\n");
            }
        }
    }

    #[test]
    fn value_options_consume_one_argument_and_preserve_diagnostics() {
        for option in ["--where", "--source", "--arch", "--limit", "--offset"] {
            let (exit, _, err) = invoke(["missing.winmd", "search", option].map(Into::into));
            assert_eq!(exit, 2);
            assert_eq!(
                err,
                format!("invalid_arguments: missing value for {option}\n")
            );
            let (exit, out, err) =
                invoke(["missing.winmd", "search", option, "--json"].map(Into::into));
            assert_eq!(exit, if option == "--where" { 3 } else { 2 });
            if option == "--where" {
                assert!(out.is_empty());
                assert!(err.starts_with("input_error: "));
            } else {
                assert!(err.is_empty());
                let out: Value = serde_json::from_str(&out).unwrap();
                assert_eq!(out["ok"], false);
            }
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStringExt;
                let (exit, _, err) = invoke([
                    "missing.winmd".into(),
                    "search".into(),
                    option.into(),
                    OsString::from_wide(&[0xd800]),
                ]);
                assert_eq!(exit, 2);
                assert_eq!(err, "invalid_arguments: argument must be valid Unicode\n");
            }
        }
    }

    #[test]
    fn conditional_search_options_may_precede_or_follow_the_command() {
        let path = metadata_path().into_os_string();
        let query = "name = \"CreateFileW\"";
        let before = invoke([
            "--where".into(),
            query.into(),
            path.clone(),
            "search".into(),
            "--json".into(),
        ]);
        let after = invoke([
            path,
            "search".into(),
            "--json".into(),
            "--where".into(),
            query.into(),
        ]);
        assert_eq!(before.0, 0, "{}", before.2);
        assert_eq!(before, after);
    }

    #[test]
    fn type_selection_uses_the_same_category_for_names_and_tokens() {
        let db = Catalog::open_many([metadata_path()]).unwrap();
        let name = "Windows.Win32.Security.SECURITY_ATTRIBUTES";
        let ty = db.select_entity(name, None, true).unwrap();
        assert!(matches!(
            db.declaration(ty, None).unwrap(),
            winlex_core::Declaration::Type(_)
        ));
        assert_eq!(db.select_entity(name, None, false).unwrap(), ty);
        for token in [format!("0x{:08X}", ty.token), ty.to_string()] {
            assert_eq!(db.select_entity(&token, None, true).unwrap(), ty);
        }

        let name = "Windows.Win32.Security.SECURITY_ATTRIBUTES.nLength";
        let field = db.select_entity(name, None, false).unwrap();
        let token = format!("0x{:08X}", field.token);
        assert_eq!(db.select_entity(&token, None, false).unwrap(), field);
        assert!(db.select_entity(&token, None, true).is_err());
        assert!(db.select_entity(name, None, true).is_err());
    }
}
