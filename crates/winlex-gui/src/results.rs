//! Search presentation is prepared on the worker, never during list painting.
use crate::{
    icons::Icon,
    list::{Display, FoldKind, TextSegment, tooltip_field},
    resources::*,
};
use std::{ops::Range, os::windows::ffi::OsStrExt};
use winlex_core::{Architecture, Catalog, EntityContext, EvidenceField, SearchMatch};

pub struct ResultItem {
    pub matched: SearchMatch,
    pub display: Display,
}
#[allow(
    clippy::vec_box,
    reason = "Keep incremental sorting and merging pointer-sized."
)]
pub type Items = Vec<Box<ResultItem>>;
pub fn prepare(catalog: &Catalog, matched: SearchMatch) -> anyhow::Result<Box<ResultItem>> {
    let context = catalog.entity_context(matched.id)?;
    let source = &catalog.sources()[matched.id.source_id];
    let tip = tooltip(&matched, &context);
    let mut item = present(matched, context);
    item.display.tooltip = tip;
    tooltip_field(
        &mut item.display.tooltip,
        IDS_TIP_PATH,
        source.path.as_os_str().encode_wide(),
    );
    Ok(Box::new(item))
}
fn present(matched: SearchMatch, context: EntityContext) -> ResultItem {
    let title_offset = matched.full_name.chars().count() - matched.name.chars().count();
    let mut display = Display {
        title: matched.name.encode_utf16().collect(),
        title_matches: project(
            &matched.name,
            &matched,
            &[
                (EvidenceField::Name, 0),
                (EvidenceField::FullName, title_offset),
            ],
        ),
        icon: Some(Icon::for_kind(&matched.kind)),
        ..Default::default()
    };
    let guid_match = matched
        .evidence
        .iter()
        .any(|e| e.field == EvidenceField::Guid);
    let value_match = matched
        .evidence
        .iter()
        .any(|e| e.field == EvidenceField::Value);
    let show_guid =
        guid_match && !value_match || context.constant.is_none() && context.guid.is_some();
    let value = if show_guid {
        context.guid.as_ref().map(|guid| format!("{{{guid}}}"))
    } else {
        context.constant.as_ref().map(winlex_format::constant)
    };
    if let Some(value) = value {
        let mut value = TextSegment::new(
            value.encode_utf16(),
            if show_guid {
                FoldKind::Guid
            } else {
                FoldKind::Text
            },
        );
        if !show_guid {
            display.detail.push(TextSegment::plain("= "));
        }
        if guid_match && show_guid {
            value.highlight();
        }
        for evidence in &matched.evidence {
            if evidence.field != EvidenceField::Value || show_guid {
                continue;
            }
            match (&evidence.indices, &context.constant) {
                (
                    Some(indices),
                    Some(
                        winlex_core::windows_metadata::Value::Utf8(text)
                        | winlex_core::windows_metadata::Value::Utf16(text),
                    ),
                ) => {
                    let mut offset = 1;
                    for (index, ch) in text.chars().enumerate() {
                        let width = if ch == '\'' {
                            1
                        } else {
                            ch.escape_debug().map(char::len_utf16).sum::<usize>()
                        };
                        if indices.binary_search(&(index as u32)).is_ok() {
                            value.matches.push(offset..offset + width);
                        }
                        offset += width;
                    }
                }
                _ => value.highlight(),
            }
        }
        display.detail.push(value);
    } else if let Some(owner) = &context.owner {
        let offset = context.namespace.chars().count() + usize::from(!context.namespace.is_empty());
        let mut segment = TextSegment::plain(owner);
        segment.matches = project(owner, &matched, &[(EvidenceField::FullName, offset)]);
        display.detail.push(segment);
    }
    if guid_match
        && !show_guid
        && let Some(guid) = &context.guid
    {
        let mut guid = TextSegment::new(format!("{{{guid}}}").encode_utf16(), FoldKind::Guid);
        guid.highlight();
        display.detail.field(guid);
    }
    let arches = architectures(matched.architecture_mask);
    display.detail.field(TextSegment::plain(&arches));
    if !context.namespace.is_empty() {
        let mut namespace = TextSegment::plain(&context.namespace);
        namespace.matches = project(
            &context.namespace,
            &matched,
            &[(EvidenceField::FullName, 0), (EvidenceField::Namespace, 0)],
        );
        display.detail.field(namespace);
    }
    ResultItem { matched, display }
}

fn architectures(mask: i32) -> String {
    if mask == Architecture::ALL_MASK {
        String::new()
    } else {
        Architecture::supported(mask)
            .map(Architecture::name)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn tooltip(matched: &SearchMatch, context: &EntityContext) -> Vec<u16> {
    let mut text: Vec<u16> = matched.full_name.encode_utf16().collect();
    text.extend("\r\n".encode_utf16());
    let constant = context.constant.as_ref().map(winlex_format::constant);
    let arches = architectures(matched.architecture_mask);
    for (label, value) in [
        (IDS_TIP_KIND, Some(matched.kind.as_str())),
        (
            IDS_TIP_NAMESPACE,
            Some(context.namespace.as_str()).filter(|s| !s.is_empty()),
        ),
        (IDS_TIP_OWNER, context.owner.as_deref()),
        (IDS_TIP_VALUE, constant.as_deref()),
        (IDS_TIP_GUID, context.guid.as_deref()),
        (
            IDS_TIP_ARCHITECTURE,
            Some(arches.as_str()).filter(|s| !s.is_empty()),
        ),
    ] {
        if let Some(value) = value {
            tooltip_field(&mut text, label, value.encode_utf16());
        }
    }
    text
}

fn project(
    text: &str,
    matched: &SearchMatch,
    fields: &[(EvidenceField, usize)],
) -> Vec<Range<usize>> {
    let mut indices = Vec::new();
    for &(field, offset) in fields {
        for evidence in matched.evidence.iter().filter(|e| e.field == field) {
            if let Some(found) = &evidence.indices {
                indices.extend(
                    found
                        .iter()
                        .filter_map(|&i| (i as usize).checked_sub(offset)),
                );
            } else if field != EvidenceField::Namespace {
                indices.extend(0..text.chars().count());
            }
        }
    }
    indices.sort_unstable();
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut position = 0;
    for (index, ch) in text.chars().enumerate() {
        let end = position + ch.len_utf16();
        if indices.binary_search(&index).is_ok() {
            if let Some(previous) = ranges.last_mut()
                && previous.end == position
            {
                previous.end = end;
            } else {
                ranges.push(position..end);
            }
        }
        position = end;
    }
    ranges
}
