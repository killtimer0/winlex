//! CLI adapter; no process exit, global output or terminal dependency in the core.
use serde_json::{Value, json};
use std::{ffi::OsString, io::Write, path::PathBuf};
use winlex_core::{Architecture, Catalog, SearchMode, SearchOptions};

pub const HELP: &str = "Winlex - offline Win32 metadata queries\n\nUsage: winlex [options] COMMAND [QUERY]\n\nCommands:\n  summary                 Input identity and row counts\n  search TEXT             Case-insensitive partial name search\n  value INTEGER           Decimal or 0x hexadecimal integer search\n  guid GUID               GUID lookup (optional braces)\n  show NAME|TOKEN         Declaration, members and typed references\n  layout TYPE             Compute size/alignment/offsets (bits)\n  vtable INTERFACE        Compute inherited method slots\n  references TYPE         Reverse type uses in signatures and bases\n\nOptions:\n  --winmd PATH            Input file; otherwise WINLEX_WINMD or beside exe\n  --arch x86|x64|arm64     Required for layout/vtable/references\n  --json                  Versioned machine-readable output\n  --exact | --prefix      Name search mode (default: contains)\n  --limit N               Page size, 0..1000 (default: 50)\n  --offset N              Matching rows to skip\n  --help, --version\n\nNames can be qualified. Ambiguous show queries require --arch or a token.\nToken IDs belong to the input WinMD SHA256. Numeric queries compare mathematical\nintegers: -1 is not the same key as 0xffffffff.\n";

struct Options {
    command: String,
    query: Option<String>,
    path: PathBuf,
    arch: Option<Architecture>,
    json: bool,
    mode: SearchMode,
    limit: usize,
    offset: usize,
}
fn parse(args: &[OsString]) -> Result<Options, String> {
    let mut path = None;
    let mut arch = None;
    let mut json = false;
    let mut mode = SearchMode::Contains;
    let mut selected_mode = false;
    let mut limit = 50;
    let mut offset = 0;
    let mut positional = vec![];
    let mut i = 0;
    let text = |s: &OsString| {
        s.to_str()
            .map(str::to_owned)
            .ok_or("argument must be valid Unicode".to_owned())
    };
    while i < args.len() {
        let a = text(&args[i])?;
        i += 1;
        match a.as_str() {
            "--winmd" | "--arch" | "--limit" | "--offset" => {
                let v = args
                    .get(i)
                    .ok_or_else(|| format!("missing value for {a}"))?;
                i += 1;
                match a.as_str() {
                    "--winmd" => path = Some(PathBuf::from(v)),
                    "--arch" => {
                        arch = Some(
                            text(v)?
                                .parse()
                                .map_err(|e: winlex_core::Error| e.to_string())?,
                        )
                    }
                    "--limit" => limit = text(v)?.parse().map_err(|_| "invalid limit")?,
                    _ => offset = text(v)?.parse().map_err(|_| "invalid offset")?,
                }
            }
            "--json" => json = true,
            "--exact" | "--prefix" => {
                if selected_mode {
                    return Err("choose only one name search mode".into());
                }
                selected_mode = true;
                mode = if a == "--exact" {
                    SearchMode::Exact
                } else {
                    SearchMode::Prefix
                };
            }
            _ if a.starts_with("--") => return Err(format!("unknown option {a}")),
            _ => positional.push(a),
        }
    }
    if limit > 1000 {
        return Err("limit must be <= 1000".into());
    }
    let command = positional.first().ok_or("missing command")?.clone();
    if ![
        "summary",
        "search",
        "value",
        "guid",
        "show",
        "layout",
        "vtable",
        "references",
    ]
    .contains(&command.as_str())
    {
        return Err(format!("unknown command {command}"));
    }
    let count = if command == "summary" { 1 } else { 2 };
    if positional.len() != count {
        return Err(format!("{command} expects {} query argument(s)", count - 1));
    }
    if selected_mode && command != "search" {
        return Err("--exact/--prefix apply only to search".into());
    }
    if ["layout", "vtable", "references"].contains(&command.as_str()) && arch.is_none() {
        return Err(format!("{command} requires --arch"));
    }
    let path = path
        .or_else(|| std::env::var_os("WINLEX_WINMD").map(PathBuf::from))
        .unwrap_or_else(|| {
            std::env::current_exe()
                .unwrap_or_default()
                .with_file_name("Windows.Win32.winmd")
        });
    Ok(Options {
        command,
        query: positional.get(1).cloned(),
        path,
        arch,
        json,
        mode,
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
    let want_json = args.iter().any(|a| a == "--json");
    if args.is_empty() || args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        return if out.write_all(HELP.as_bytes()).is_ok() {
            0
        } else {
            4
        };
    }
    if args.len() == 1 && (args[0] == "--version" || args[0] == "-V") {
        return if writeln!(out, "winlex {}", env!("CARGO_PKG_VERSION")).is_ok() {
            0
        } else {
            4
        };
    }
    let o = match parse(&args) {
        Ok(v) => v,
        Err(e) => return error(out, err, want_json, 2, "invalid_arguments", &e),
    };
    let db = match Catalog::open(&o.path) {
        Ok(v) => v,
        Err(e) => {
            return error(
                out,
                err,
                o.json,
                3,
                "input_error",
                &format!("{}: {e}", o.path.display()),
            );
        }
    };
    let q = o.query.as_deref().unwrap_or("");
    let options = || SearchOptions {
        case_sensitive: false,
        mode: o.mode,
        architecture: o.arch,
        offset: o.offset,
        limit: o.limit,
        cancel: None,
    };
    let result: winlex_core::Result<Value> = (|| {
        Ok(match o.command.as_str() {
            "summary" => db.summary(),
            "search" | "value" | "guid" => {
                let mut opt = options();
                if o.command == "value" {
                    opt.mode = SearchMode::Integer;
                }
                if o.command == "guid" {
                    opt.mode = SearchMode::Guid;
                }
                serde_json::to_value(db.search(q, opt)?)?
            }
            "show" => db.describe(q, o.arch)?,
            "layout" => db.layout(q, o.arch.unwrap())?,
            "vtable" => db.vtable(q, o.arch.unwrap())?,
            "references" => db.references(q, o.arch.unwrap(), options())?,
            _ => unreachable!(),
        })
    })();
    let result = match result {
        Ok(r) => r,
        Err(e) => return error(out, err, o.json, 3, "query_error", &e.to_string()),
    };
    let written = if o.json {
        serde_json::to_writer_pretty(&mut *out,&json!({"schema":"winlex-cli-1","ok":true,"source_sha256":db.source_sha256(),"command":o.command,"result":result})).map_err(std::io::Error::other).and_then(|_|writeln!(out))
    } else {
        render(&result, out, 0)
    };
    if let Err(e) = written {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            return 0;
        }
        let _ = writeln!(err, "output error: {e}");
        return 4;
    }
    0
}
fn error(
    out: &mut dyn Write,
    err: &mut dyn Write,
    json_mode: bool,
    exit: u8,
    code: &str,
    message: &str,
) -> u8 {
    let result = if json_mode {
        serde_json::to_writer(
            &mut *out,
            &json!({"schema":"winlex-cli-1","ok":false,"error":{"code":code,"message":message}}),
        )
        .map_err(std::io::Error::other)
        .and_then(|_| writeln!(out))
    } else {
        writeln!(err, "{code}: {message}")
    };
    if result.is_err() { 4 } else { exit }
}
fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
fn ty(v: &Value) -> String {
    match s(&v["kind"]) {
        "primitive" => s(&v["name"]).into(),
        "named" => format!("{} -> {}", s(&v["name"]), s(&v["reference_token"])),
        "pointer" => format!(
            "{}{}{}",
            if v["const"] == true { "const " } else { "" },
            ty(&v["element"]),
            "*".repeat(v["depth"].as_u64().unwrap_or(1).min(256) as usize)
        ),
        "array" => format!("{}[{}]", ty(&v["element"]), v["length"]),
        "byref" => format!("{}&", ty(&v["element"])),
        "const" => format!("const {}", ty(&v["element"])),
        _ => v.to_string(),
    }
}
fn render(v: &Value, out: &mut dyn Write, depth: usize) -> std::io::Result<()> {
    let pad = "  ".repeat(depth);
    if let Some(items) = v["matches"].as_array() {
        writeln!(
            out,
            "{} match(es); offset {}, limit {}",
            v["total"], v["offset"], v["limit"]
        )?;
        for item in items {
            writeln!(
                out,
                "{}  {:10} arch={}  {}",
                s(&item["token"]),
                s(&item["kind"]),
                item["architecture_mask"],
                s(&item["full_name"])
            )?;
        }
        return Ok(());
    }
    if !v["page"].is_null() {
        writeln!(
            out,
            "References to {} (unresolved uses: {})",
            s(&v["target_token"]),
            v["unresolved_type_uses"]
        )?;
        return render(&v["page"], out, depth);
    }
    if let Some(slots) = v["slots"].as_array() {
        writeln!(out, "{} [{}]", s(&v["name"]), s(&v["arch"]))?;
        for slot in slots {
            writeln!(
                out,
                "{:>3}  {}.{}  {}  byte-offset={}",
                slot["index"],
                s(&slot["declaring_type"]),
                s(&slot["method"]),
                s(&slot["token"]),
                slot["byte_offset"]
            )?;
        }
        return Ok(());
    }
    if !v["size_bits"].is_null() && v["offset_bits"].is_null() {
        writeln!(
            out,
            "{} [{}] size={} bits, alignment={} bits",
            s(&v["name"]),
            s(&v["arch"]),
            v["size_bits"],
            v["alignment_bits"]
        )?;
    }
    if !v["offset_bits"].is_null() {
        writeln!(
            out,
            "{pad}{} offset={} bits, size={} bits",
            s(&v["name"]),
            v["offset_bits"],
            v["size_bits"]
        )?;
    }
    if let Some(bits) = v["bitfields"].as_array() {
        for b in bits {
            writeln!(
                out,
                "{pad}  bitfield {}: offset={} bits, width={} bits",
                s(&b["name"]),
                b.get("offset_bits").unwrap_or(&b["relative_offset_bits"]),
                b["width_bits"]
            )?;
        }
    }
    if let Some(e) = v.get("entity") {
        writeln!(
            out,
            "{pad}{} {} {}",
            s(&e["token"]),
            s(&e["kind"]),
            e["full_name"].as_str().unwrap_or(s(&e["name"]))
        )?;
        if !v["type"].is_null() {
            writeln!(out, "{pad}  type: {}", ty(&v["type"]))?;
        }
        if !v["declared_offset_bytes"].is_null() {
            writeln!(
                out,
                "{pad}  declared offset: {} bytes",
                v["declared_offset_bytes"]
            )?;
        }
        if !v["value"].is_null() {
            writeln!(out, "{pad}  value: {}", v["value"])?;
        }
        if !v["guid"].is_null() {
            writeln!(out, "{pad}  GUID: {}", s(&v["guid"]))?;
        }
        if !v["layout_kind"].is_null() {
            writeln!(
                out,
                "{pad}  layout: {} {}",
                s(&v["layout_kind"]),
                v["layout_constraints"]
            )?;
        }
        if !v["scope_token"].is_null() {
            writeln!(
                out,
                "{pad}  scope: {}; resolution: {}",
                s(&v["scope_token"]),
                v["resolution"]
            )?;
        }
        if !v["return_type"].is_null() {
            writeln!(out, "{pad}  returns: {}", ty(&v["return_type"]))?;
        }
        if !v["import"].is_null() {
            writeln!(out, "{pad}  import: {}", v["import"])?;
        }
        if let Some(ps) = v["parameters"].as_array() {
            for p in ps {
                writeln!(
                    out,
                    "{pad}  [{}] {}: {} ({})",
                    p["sequence"],
                    s(&p["name"]),
                    ty(&p["type"]),
                    s(&p["direction"])
                )?;
            }
        }
        if let Some(attrs) = v["attributes"].as_array() {
            for a in attrs {
                writeln!(out, "{pad}  @{} {}", s(&a["name"]), a["arguments"])?;
            }
        }
        if let Some(bases) = v["interfaces"].as_array() {
            for b in bases {
                writeln!(out, "{pad}  base: {}", ty(b))?;
            }
        }
    }
    for key in ["fields", "methods"] {
        if let Some(items) = v[key].as_array() {
            for item in items {
                render(item, out, depth + 1)?;
            }
        }
    }
    if v.get("entity").is_none() && v.get("fields").is_none() && v.get("offset_bits").is_none() {
        writeln!(out, "{pad}{v}")?;
    }
    Ok(())
}
