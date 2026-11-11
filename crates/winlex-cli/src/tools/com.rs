use crate::SCHEMA;
use std::{ffi::OsString, fmt::Write as _, io::Write};
use winlex_core::tools::com::{
    self, Attribute, Field, Group, GroupKind, Report, Request, ResolvedKind, TargetKind, Views,
    Warning,
};

const HELP: &str = "Usage: winlex com TARGET [options]

Options:
  --kind auto|progid|guid   How to interpret TARGET (default: auto)
  --view auto|32|64|both    Registry views to query (default: both)
  --json                    Versioned machine-readable output
  --help, -h                Show this help

Resolves ProgID, CLSID, IID, LIBID and AppID registrations from the local
registry. Both machine and user hives are read. A GUID is checked against all
four registration kinds instead of guessing one.
";

#[derive(Debug)]
struct Options {
    text: String,
    kind: TargetKind,
    views: Views,
    json: bool,
}

fn parse(args: &[OsString]) -> Result<Options, String> {
    let mut kind = TargetKind::Auto;
    let mut views = Views::Both;
    let mut json = false;
    let mut text = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let value = arg.to_str().ok_or("argument must be valid Unicode")?;
        match value {
            "--kind" => {
                let value = args
                    .next()
                    .ok_or("missing value for --kind")?
                    .to_str()
                    .ok_or("argument must be valid Unicode")?;
                kind = TargetKind::parse(value).ok_or("--kind must be auto, progid or guid")?;
            }
            "--view" => {
                let value = args
                    .next()
                    .ok_or("missing value for --view")?
                    .to_str()
                    .ok_or("argument must be valid Unicode")?;
                views = Views::parse(value).ok_or("--view must be auto, 32, 64 or both")?;
            }
            "--json" => json = true,
            _ if value.starts_with("--") => return Err(format!("unknown option {value}")),
            _ if text.is_some() => return Err(format!("unexpected argument {value}")),
            _ => text = Some(value.to_owned()),
        }
    }
    let text = text.ok_or("missing target")?;
    Ok(Options {
        text,
        kind,
        views,
        json,
    })
}

pub(super) fn run(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    if matches!(args, [arg] if arg == "--help" || arg == "-h") {
        return if out.write_all(HELP.as_bytes()).is_ok() {
            0
        } else {
            4
        };
    }
    let json = args.iter().any(|arg| arg == "--json");
    let options = match parse(args) {
        Ok(options) => options,
        Err(error) => return crate::error(out, err, json, 2, "invalid_arguments", &error),
    };
    let report = com::query(
        &com::registry::Live,
        &Request {
            text: options.text,
            kind: options.kind,
            views: options.views,
        },
    );
    let failed = report.kind.is_none();
    let written = if options.json {
        serde_json::to_writer_pretty(
            &mut *out,
            &serde_json::json!({
                "schema": SCHEMA,
                "ok": !failed,
                "command": "com",
                "result": report,
            }),
        )
        .map_err(std::io::Error::other)
        .and_then(|_| writeln!(out))
    } else {
        write!(out, "{}", render(&report))
    };
    crate::finish_output(written, err, if failed { 3 } else { 0 })
}

fn render(report: &Report) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "Input: {}", report.input);
    if let Some(kind) = report.kind {
        let label = match kind {
            ResolvedKind::Guid => "GUID",
            ResolvedKind::ProgId => "ProgID",
        };
        let normalized = report.normalized.as_deref().unwrap_or_default();
        let _ = writeln!(text, "Target: {label} {normalized}");
    }
    for group in &report.groups {
        render_group(&mut text, group, 0);
    }
    if !report.warnings.is_empty() {
        let _ = writeln!(text, "Warnings:");
        for warning in &report.warnings {
            let _ = writeln!(text, "  {}", warning_text(warning));
        }
    }
    text
}

fn render_group(out: &mut String, group: &Group, depth: usize) {
    let indent = "  ".repeat(depth);
    let _ = writeln!(
        out,
        "{indent}{} {}",
        group_kind_name(group.kind),
        group.identity.as_deref().unwrap_or_default()
    );
    for field in &group.fields {
        render_field(out, field, depth + 1);
    }
    for child in &group.children {
        render_group(out, child, depth + 1);
    }
}

fn render_field(out: &mut String, field: &Field, depth: usize) {
    let indent = "  ".repeat(depth);
    let name = attribute_name(field.attribute);
    let padded = " ".repeat(name.len());
    // Identical values from both hives stay unannotated; a user-only
    // registration is worth marking because machine registrations are typical.
    let annotate = field.values.len() > 1
        || field
            .values
            .iter()
            .flat_map(|value| &value.sources)
            .all(|source| source.origin == com::Origin::User);
    for (index, value) in field.values.iter().enumerate() {
        let label = if index == 0 { name } else { &padded };
        let _ = write!(out, "{indent}{label}: ");
        if value.value.is_empty() {
            let _ = write!(out, "(empty)");
        } else {
            let _ = write!(out, "{}", value.value);
        }
        if annotate {
            let sources: Vec<String> = value
                .sources
                .iter()
                .map(|source| format!("{} {}", view_name(source.view), origin_name(source.origin)))
                .collect();
            let _ = write!(out, " [{}]", sources.join(", "));
        }
        let _ = writeln!(out);
    }
}

fn group_kind_name(kind: GroupKind) -> &'static str {
    match kind {
        GroupKind::ProgId => "ProgID",
        GroupKind::Class => "CLSID",
        GroupKind::Interface => "IID",
        GroupKind::TypeLib => "LIBID",
        GroupKind::TypeLibVersion => "Version",
        GroupKind::TypeLibLocale => "Locale",
        GroupKind::AppId => "AppID",
    }
}

fn attribute_name(attribute: Attribute) -> &'static str {
    attribute.name()
}

fn view_name(view: com::View) -> &'static str {
    match view {
        com::View::X86 => "x86",
        com::View::X64 => "x64",
        com::View::Native => "native",
    }
}

fn origin_name(origin: com::Origin) -> &'static str {
    match origin {
        com::Origin::Machine => "machine",
        com::Origin::User => "user",
    }
}

fn warning_text(warning: &Warning) -> String {
    match warning {
        Warning::EmptyInput => "missing target".into(),
        Warning::InvalidGuid(text) => format!("invalid GUID {text:?}"),
        Warning::InvalidProgId(text) => format!("invalid ProgID {text:?}"),
        Warning::Unrecognized(text) => {
            format!("cannot tell whether {text:?} is a ProgID or GUID; pass --kind")
        }
        Warning::NotFound(text) => format!("no COM registration found for {text}"),
        Warning::UnsupportedView(view) => format!(
            "the {} registry view is not available on this system",
            view_name(*view)
        ),
        Warning::Registry(error) => format!("registry read failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Options, String> {
        parse(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn options_default_to_auto_detection_and_both_views() {
        let options = parse_args(&["Shell.Application"]).unwrap();
        assert_eq!(options.text, "Shell.Application");
        assert_eq!(options.kind, TargetKind::Auto);
        assert_eq!(options.views, Views::Both);
        assert!(!options.json);
        assert_eq!(
            parse_args(&["x", "--view", "32"]).unwrap().views,
            Views::X86
        );
        assert_eq!(
            parse_args(&["x", "--kind", "guid"]).unwrap().kind,
            TargetKind::Guid
        );
        for args in [
            vec![],
            vec!["x", "y"],
            vec!["x", "--kind", "wat"],
            vec!["x", "--view", "128"],
            vec!["x", "--unknown"],
        ] {
            assert!(parse_args(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn text_output_annotates_only_differing_values() {
        let report = Report {
            input: "{00000000-0000-0000-0000-000000000001}".into(),
            kind: Some(ResolvedKind::Guid),
            normalized: Some("{00000000-0000-0000-0000-000000000001}".into()),
            groups: vec![Group {
                kind: GroupKind::Class,
                identity: Some("{00000000-0000-0000-0000-000000000001}".into()),
                fields: vec![
                    Field {
                        attribute: Attribute::Description,
                        values: vec![com::Sourced {
                            value: "Example".into(),
                            sources: vec![com::Provenance {
                                view: com::View::X86,
                                origin: com::Origin::Machine,
                            }],
                        }],
                    },
                    Field {
                        attribute: Attribute::InprocServer32,
                        values: vec![
                            com::Sourced {
                                value: "C:\\x86\\a.dll".into(),
                                sources: vec![com::Provenance {
                                    view: com::View::X86,
                                    origin: com::Origin::Machine,
                                }],
                            },
                            com::Sourced {
                                value: "C:\\x64\\a.dll".into(),
                                sources: vec![com::Provenance {
                                    view: com::View::X64,
                                    origin: com::Origin::Machine,
                                }],
                            },
                        ],
                    },
                ],
                children: vec![],
            }],
            warnings: vec![Warning::UnsupportedView(com::View::X64)],
        };
        let text = render(&report);
        assert!(text.contains("Target: GUID {00000000-0000-0000-0000-000000000001}"));
        assert!(text.contains("CLSID {00000000-0000-0000-0000-000000000001}"));
        assert!(text.contains("  Description: Example\n"));
        assert!(text.contains("C:\\x86\\a.dll [x86 machine]"));
        assert!(text.contains("C:\\x64\\a.dll [x64 machine]"));
        assert!(text.contains("Warnings:\n  the x64 registry view"));
    }
}
