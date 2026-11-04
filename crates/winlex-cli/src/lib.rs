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
  references TYPE         Reverse type uses across loaded sources
  error-code CODE         Query Windows messages (no input file required)

Options:
  --source SOURCE         Zero-based input index or unique assembly name
  --arch x86|x64|arm64     Required for layout/vtable/references
  --json                  Versioned machine-readable output
  --name                  Exact name or trailing '.'/'+' scope search
  --value                 Integer bits or target-type floating equality
  --guid                  GUID text fragment or byte sequence
  --limit N               Page size, 0..1000 (default: 50)
  --offset N              Matching rows to skip
  --help, --version       No input required

Input files precede the command. Use .\search for a file named search.
Tools: winlex error-code --help
Declaration selectors accept SOURCE::NAME or SOURCE::0xTOKEN.
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
    References,
}
impl Command {
    const NAMES: &[(Self, &str)] = &[
        (Self::Summary, "summary"),
        (Self::Search, "search"),
        (Self::Show, "show"),
        (Self::Layout, "layout"),
        (Self::Vtable, "vtable"),
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
        Command::Layout | Command::Vtable | Command::References
    ) && arch.is_none()
    {
        return Err(format!("{name} requires --arch"));
    }
    if source.is_some()
        && (command == Command::Summary || positional.get(1).is_some_and(|q| q.contains("::")))
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
        return if out.write_all(text.as_bytes()).is_ok() {
            0
        } else {
            4
        };
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
    let selector = o
        .source
        .as_ref()
        .map_or_else(|| q.to_owned(), |s| format!("{s}::{q}"));
    let result: winlex_core::Result<Query> = (|| {
        Ok(match o.command {
            Command::Summary => Query::Summary,
            Command::Search => {
                let options = SearchOptions {
                    source: o
                        .source
                        .as_deref()
                        .map(|s| source_index(&db, s))
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
            Command::Show => Query::Show(select(&db, &selector, o.arch, false)?),
            Command::Layout => {
                Query::Layout(db.layout(select(&db, &selector, o.arch, true)?, o.arch.unwrap())?)
            }
            Command::Vtable => {
                Query::Vtable(db.vtable(select(&db, &selector, o.arch, true)?, o.arch.unwrap())?)
            }
            Command::References => {
                let target = select(&db, &selector, o.arch, true)?;
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
        serde_json::to_writer_pretty(&mut *out, &response)
            .map_err(std::io::Error::other)
            .and_then(|_| writeln!(out))
    } else {
        match text::render(&result, &db, o.arch) {
            Ok(text) => write!(out, "{text}"),
            Err(e) => return error(out, err, false, 3, "query_error", &e.to_string()),
        }
    };
    finish_output(written, err, 0)
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
    Show(winlex_core::EntityId),
    Layout(winlex_core::Layout),
    Vtable(winlex_core::Vtable),
    References(winlex_core::EntityId, Page, usize),
}
impl Query {
    fn json(&self, db: &Catalog, arch: Option<Architecture>) -> winlex_core::Result<Value> {
        Ok(match self {
            Self::Summary => winlex_format::summary(db),
            Self::Matches(p) => serde_json::to_value(p)?,
            Self::Show(id) => winlex_format::describe(db, *id, arch)?,
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
fn source_index(db: &Catalog, s: &str) -> winlex_core::Result<usize> {
    if let Ok(index) = s.parse::<usize>() {
        return if index < db.sources().len() {
            Ok(index)
        } else {
            Err(winlex_core::Error::msg("source index out of range"))
        };
    }
    let found: Vec<_> = db
        .sources()
        .iter()
        .filter(|source| source.assembly.as_ref().is_some_and(|a| a.name == s))
        .collect();
    if found.len() != 1 {
        return Err(winlex_core::Error::msg(format!(
            "source selector {s:?} matches {} files; use a source index",
            found.len()
        )));
    }
    Ok(found[0].id)
}
/// Resolve the CLI's source/name/token syntax to a catalog-local identity.
pub fn select(
    db: &Catalog,
    s: &str,
    arch: Option<Architecture>,
    types_only: bool,
) -> winlex_core::Result<winlex_core::EntityId> {
    let (source, name) = if let Some((source, name)) = s.split_once("::") {
        (Some(source_index(db, source)?), name)
    } else {
        (None, s)
    };
    if let Some(token) = name.strip_prefix("0x") {
        if source.is_none() && db.sources().len() != 1 {
            return Err(winlex_core::Error::msg(
                "token requires SOURCE::TOKEN with multiple sources",
            ));
        }
        let id = winlex_core::EntityId {
            source_id: source.unwrap_or(0),
            token: u32::from_str_radix(token, 16)?,
        };
        let row = db.declaration(id, arch)?;
        if types_only && !matches!(row, winlex_core::Declaration::Type(_)) {
            return Err(winlex_core::Error::msg("expected a type"));
        }
        return Ok(id);
    }
    let mut matches = db.search_exact(
        name,
        SearchOptions {
            source,
            case_sensitive: true,
            architecture: arch,
            categories: Some(if types_only {
                winlex_core::EntityCategory::Type as u32
            } else {
                winlex_core::EntityCategory::ALL
                    & !(winlex_core::EntityCategory::TypeReference as u32)
            }),
            ..Default::default()
        },
    )?;
    let first = matches.next().transpose()?;
    let second = matches.next().transpose()?;
    match (first, second) {
        (Some(item), None) => Ok(item.id),
        (_, second) => Err(winlex_core::Error::msg(format!(
            "expected one entity, found {}; specify --arch, a qualified name or token",
            if second.is_some() { "2 or more" } else { "0" }
        ))),
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

    fn invoke(args: impl IntoIterator<Item = OsString>) -> (u8, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let exit = run(args, &mut out, &mut err);
        (
            exit,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

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
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            for option in ["--where", "--source", "--arch", "--limit", "--offset"] {
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
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../inputs/Windows.Win32.winmd")
            .into_os_string();
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
        let db = Catalog::open(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../inputs/Windows.Win32.winmd"),
        )
        .unwrap();
        let name = "Windows.Win32.Security.SECURITY_ATTRIBUTES";
        let ty = select(&db, name, None, true).unwrap();
        assert!(matches!(
            db.declaration(ty, None).unwrap(),
            winlex_core::Declaration::Type(_)
        ));
        assert_eq!(select(&db, name, None, false).unwrap(), ty);
        for token in [format!("0x{:08X}", ty.token), ty.to_string()] {
            assert_eq!(select(&db, &token, None, true).unwrap(), ty);
        }

        let name = "Windows.Win32.Security.SECURITY_ATTRIBUTES.nLength";
        let field = select(&db, name, None, false).unwrap();
        let token = format!("0x{:08X}", field.token);
        assert_eq!(select(&db, &token, None, false).unwrap(), field);
        assert!(select(&db, &token, None, true).is_err());
        assert!(select(&db, name, None, true).is_err());
    }
}
