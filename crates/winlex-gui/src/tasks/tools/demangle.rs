//! Batch symbol restoration and presentation, without native control access.
use crate::{
    resources::{self, *},
    rich_text::Content,
    worker::Ticket,
};
use anyhow::ensure;
use std::fmt::Write;
use winlex_core::tools::demangle::{self, Abi, SymbolKind};
use winlex_format::declaration::{Region, Rendered, SymbolRole};

pub const MAX_INPUT_UNITS: usize = 256 * 1024;
const MAX_BATCH_BYTES: usize = 8 * 1024 * 1024;

pub struct Output {
    pub content: Content,
    pub copy: String,
    pub status: String,
}

pub fn run(input: &str, abi: Abi, ticket: &Ticket) -> anyhow::Result<Output> {
    ensure!(
        input.encode_utf16().count() <= MAX_INPUT_UNITS,
        "{}",
        resources::text(IDS_DEMANGLE_INPUT_LIMIT)
    );
    let mut results = Vec::new();
    let mut size = 0;
    for symbol in input.lines().map(str::trim).filter(|line| !line.is_empty()) {
        ensure!(!ticket.cancelled(), "cancelled");
        let result = demangle::demangle(symbol, abi);
        ensure!(!ticket.cancelled(), "cancelled");
        size += symbol.len()
            + match &result {
                Ok(value) => value.text.len(),
                Err(error) => error.to_string().len(),
            };
        ensure!(
            size <= MAX_BATCH_BYTES,
            "{}",
            resources::text(IDS_DEMANGLE_OUTPUT_LIMIT)
        );
        results.push((symbol, result));
    }
    let mut body = Rendered::default();
    let mut copy = String::new();
    let (mut restored, mut failed) = (0, 0);
    let mut first_error = None;
    for (index, (symbol, result)) in results.into_iter().enumerate() {
        ensure!(!ticket.cancelled(), "cancelled");
        if index != 0 {
            writeln!(body)?;
        }
        writeln!(body, "{symbol}")?;
        match result {
            Ok(result) => {
                restored += 1;
                let start = body.position();
                writeln!(body, "{}", result.text)?;
                for span in &result.spans {
                    body.regions.push(Region::symbol(
                        start + span.start..start + span.end,
                        SymbolRole::Method,
                    ));
                }
                let abi_label = match result.abi {
                    Abi::Msvc => IDS_DEMANGLE_MSVC,
                    Abi::Itanium => IDS_DEMANGLE_ITANIUM,
                    Abi::Auto => unreachable!("resolved ABI"),
                };
                let kind_label = match result.kind {
                    SymbolKind::Function => IDS_FUNCTION,
                    SymbolKind::Data => IDS_DEMANGLE_DATA,
                    SymbolKind::Special => IDS_DEMANGLE_SPECIAL,
                    SymbolKind::Unknown => IDS_DEMANGLE_UNKNOWN,
                };
                write!(
                    body,
                    "  {} · {}",
                    resources::text(abi_label),
                    resources::text(kind_label)
                )?;
                if let Some(convention) = result.convention {
                    write!(body, " · {}", convention.name())?;
                } else if result.abi == Abi::Itanium && result.kind == SymbolKind::Function {
                    write!(body, " · {}", resources::text(IDS_DEMANGLE_NO_RETURN))?;
                }
                writeln!(body)?;
                if !copy.is_empty() {
                    copy.push('\n');
                }
                copy.push_str(&result.text);
            }
            Err(error) => {
                failed += 1;
                let message = format!("{}: {error}", resources::text(IDS_DEMANGLE_ERROR));
                first_error.get_or_insert_with(|| format!("{symbol}: {message}"));
                writeln!(body, "  {message}")?;
            }
        }
        ensure!(
            body.text.len() <= MAX_BATCH_BYTES,
            "{}",
            resources::text(IDS_DEMANGLE_OUTPUT_LIMIT)
        );
    }
    let mut status = resources::format(
        IDS_DEMANGLE_STATUS,
        &[
            &(restored + failed).to_string(),
            &restored.to_string(),
            &failed.to_string(),
        ],
    );
    if let Some(error) = first_error {
        status.push(' ');
        status.push_str(&error);
    }
    if restored + failed == 0 {
        status = resources::text(IDS_DEMANGLE_EMPTY);
        body = Rendered::from(status.clone());
    }
    ensure!(!ticket.cancelled(), "cancelled");
    Ok(Output {
        content: body.into(),
        copy,
        status,
    })
}
