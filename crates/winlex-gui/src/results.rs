//! Search presentation is prepared on the worker, never during list painting.
use crate::{
    icons::Icon,
    list::{Display, FoldKind, TextSegment, tooltip_field},
    resources::*,
};
use std::{ops::Deref, ops::Range, os::windows::ffi::OsStrExt};
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
impl Deref for ResultItem {
    type Target = SearchMatch;
    fn deref(&self) -> &Self::Target {
        &self.matched
    }
}
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
    indices.dedup();
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

#[cfg(test)]
mod tests {
    use super::*;
    use winlex_core::{
        EntityId, EntitySummary, MatchEvidence, MatchField, MatchQuality, windows_metadata::Value,
    };

    fn evidence(field: EvidenceField, indices: Option<Vec<u32>>) -> MatchEvidence {
        MatchEvidence { field, indices }
    }

    fn item(name: &str, full_name: &str, kind: &str, quality: MatchQuality) -> SearchMatch {
        SearchMatch {
            evidence: match &quality {
                MatchQuality::NameExact(field) => vec![evidence(
                    if *field == MatchField::Name {
                        EvidenceField::Name
                    } else {
                        EvidenceField::FullName
                    },
                    None,
                )],
                MatchQuality::Integer => vec![evidence(EvidenceField::Value, None)],
                MatchQuality::GuidFragment => vec![evidence(EvidenceField::Guid, None)],
                MatchQuality::NameScope => {
                    vec![evidence(EvidenceField::FullName, Some(vec![0, 1]))]
                }
                _ => Vec::new(),
            },
            entity: EntitySummary {
                id: EntityId {
                    source_id: 0,
                    token: 0x06000001,
                },
                name: name.into(),
                full_name: full_name.into(),
                kind: kind.into(),
                architecture_mask: 7,
            },
            quality,
        }
    }
    fn context() -> EntityContext {
        EntityContext {
            namespace: "N".into(),
            owner: Some("Outer+Inner".into()),
            constant: None,
            guid: None,
        }
    }
    fn with_indices(mut hit: SearchMatch, field: EvidenceField, indices: Vec<u32>) -> SearchMatch {
        hit.evidence = vec![evidence(field, Some(indices))];
        hit
    }
    fn text(value: &crate::list::TextLine) -> String {
        String::from_utf16(&value.text()).unwrap()
    }
    fn matches(value: &crate::list::TextLine) -> Vec<Range<usize>> {
        value.fit(i32::MAX, |s| s.len() as i32).1
    }

    #[test]
    fn overlapping_name_and_namespace_evidence_projects_once_per_display_character() {
        let mut hit = item(
            "😀W",
            "中😀.Outer+Inner.😀W",
            "method",
            MatchQuality::Filter,
        );
        hit.evidence = vec![
            evidence(
                EvidenceField::FullName,
                Some(vec![15, 9, 0, 1, 3, 8, 14, 15, u32::MAX]),
            ),
            evidence(EvidenceField::Name, Some(vec![1, 1])),
            evidence(EvidenceField::Namespace, Some(vec![1])),
        ];
        let mut ctx = context();
        ctx.namespace = "中😀".into();
        let result = present(hit, ctx);
        assert_eq!(
            result.display.title,
            "😀W".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(result.display.title_matches, vec![0..3]);
        assert_eq!(text(&result.display.detail), "Outer+Inner · 中😀");
        assert_eq!(matches(&result.display.detail), [0..1, 5..7, 14..17]);

        let mut hit = item(
            "😀W",
            "中😀.Outer+Inner.😀W",
            "method",
            MatchQuality::Filter,
        );
        hit.evidence = vec![evidence(EvidenceField::FullName, None)];
        let mut ctx = context();
        ctx.namespace = "中😀".into();
        let result = present(hit, ctx);
        assert_eq!(result.display.title_matches, vec![0..3]);
        assert_eq!(matches(&result.display.detail), [0..11, 14..17]);
    }

    #[test]
    fn names_and_qualified_matches_project_to_utf16_display_fields() {
        let result = present(
            with_indices(
                item(
                    "😀窗口W",
                    "N.Outer+Inner.😀窗口W",
                    "method",
                    MatchQuality::Fuzzy {
                        score: 1,
                        field: MatchField::Name,
                    },
                ),
                EvidenceField::Name,
                vec![0, 3],
            ),
            context(),
        );
        assert_eq!(result.display.title_matches, [0..2, 4..5]);
        assert!(matches(&result.display.detail).is_empty());
        assert_eq!(text(&result.display.detail), "Outer+Inner · N");

        let result = present(
            with_indices(
                item(
                    "Window",
                    "N.Outer+Inner.Window",
                    "method",
                    MatchQuality::Fuzzy {
                        score: 1,
                        field: MatchField::FullName,
                    },
                ),
                EvidenceField::FullName,
                vec![0, 1, 2, 7, 8, 13, 14],
            ),
            context(),
        );
        assert_eq!(
            result.display.title_matches.as_slice(),
            std::slice::from_ref(&(0..1))
        );
        assert_eq!(matches(&result.display.detail), [0..1, 5..7, 14..15]);

        let result = present(
            with_indices(
                item(
                    "Window",
                    "N.Outer+Inner.Window",
                    "method",
                    MatchQuality::NameScope,
                ),
                EvidenceField::FullName,
                (0..8).collect(),
            ),
            context(),
        );
        assert!(result.display.title_matches.is_empty());
        assert_eq!(matches(&result.display.detail), [0..6, 14..15]);
    }

    #[test]
    fn values_and_guids_highlight_reported_evidence() {
        let mut ctx = context();
        ctx.constant = Some(Value::I32(5));
        let result = present(
            item(
                "VALUE",
                "N.Outer+Inner.VALUE",
                "constant",
                MatchQuality::Integer,
            ),
            ctx,
        );
        assert_eq!(text(&result.display.detail), "= 5 · N");
        assert_eq!(
            matches(&result.display.detail).as_slice(),
            std::slice::from_ref(&(2..3))
        );
        assert!(result.display.title_matches.is_empty());

        let mut ctx = context();
        ctx.guid = Some("12345678-9abc-def0-1234-56789abcdef0".into());
        let result = present(
            item("IXxx", "N.IXxx", "interface", MatchQuality::GuidFragment),
            ctx,
        );
        assert_eq!(
            text(&result.display.detail),
            "{12345678-9abc-def0-1234-56789abcdef0} · N"
        );
        assert_eq!(
            matches(&result.display.detail).as_slice(),
            std::slice::from_ref(&(0..38))
        );

        let mut ctx = context();
        ctx.constant = Some(Value::I32(5));
        ctx.guid = Some("12345678-9abc-def0-1234-56789abcdef0".into());
        let result = present(
            item(
                "VALUE",
                "N.Outer+Inner.VALUE",
                "constant",
                MatchQuality::NameExact(MatchField::Name),
            ),
            ctx,
        );
        assert_eq!(
            result.display.title_matches.as_slice(),
            std::slice::from_ref(&(0..5))
        );
        assert!(matches(&result.display.detail).is_empty());
    }

    #[test]
    fn multiple_fields_and_escaped_strings_keep_independent_highlights() {
        let mut hit = item("ACCESS", "N.ACCESS", "constant", MatchQuality::Filter);
        hit.evidence = vec![
            evidence(EvidenceField::Name, Some(vec![0, 1, 2, 3, 4, 5])),
            evidence(EvidenceField::Value, Some(vec![1, 2, 3, 4])),
        ];
        let mut ctx = context();
        ctx.constant = Some(Value::Utf16("a\n😀'\\z".into()));
        let result = present(hit, ctx);
        assert_eq!(text(&result.display.detail), "= \"a\\n😀'\\\\z\" · N");
        assert_eq!(result.display.title_matches, vec![0..6]);
        assert_eq!(matches(&result.display.detail), vec![4..11]);
        let full = result.display.detail.text();
        for width in 0..full.len() {
            let (shown, matches) = result.display.detail.fit(width as i32, |s| s.len() as i32);
            assert!(String::from_utf16(&shown).is_ok());
            assert!(
                matches
                    .iter()
                    .all(|r| r.start < r.end && r.end <= shown.len())
            );
            assert_eq!(result.display.detail.text(), full);
        }

        let mut hit = item("ACCESS", "N.ACCESS", "constant", MatchQuality::Filter);
        hit.evidence = vec![
            evidence(EvidenceField::Value, None),
            evidence(EvidenceField::Guid, None),
        ];
        let mut ctx = context();
        ctx.constant = Some(Value::I32(5));
        ctx.guid = Some("12345678-9abc-def0-1234-56789abcdef0".into());
        let result = present(hit, ctx);
        assert_eq!(
            text(&result.display.detail),
            "= 5 · {12345678-9abc-def0-1234-56789abcdef0} · N"
        );
        assert_eq!(matches(&result.display.detail), vec![2..3, 6..44]);
        let (shown, matches) = result.display.detail.fit(14, |s| s.len() as i32);
        assert_eq!(String::from_utf16(&shown).unwrap(), "= 5 · {…} · N");
        assert_eq!(matches, [2..3, 6..7, 8..9]);
    }

    #[test]
    fn all_architectures_is_omitted_only_for_the_full_mask() {
        for (mask, label) in [
            (7, ""),
            (1, "x86"),
            (2, "x64"),
            (4, "arm64"),
            (3, "x86 x64"),
            (5, "x86 arm64"),
            (6, "x64 arm64"),
        ] {
            let mut ctx = context();
            ctx.owner = None;
            let mut hit = item("Type", "N.Type", "struct", MatchQuality::NameScope);
            hit.entity.architecture_mask = mask;
            let result = present(hit, ctx);
            let expected = if label.is_empty() {
                "N".to_owned()
            } else {
                format!("{label} · N")
            };
            assert_eq!(text(&result.display.detail), expected);
            let offset = if label.is_empty() {
                0
            } else {
                label.encode_utf16().count() + 3
            };
            assert_eq!(
                matches(&result.display.detail).as_slice(),
                std::slice::from_ref(&(offset..offset + 1))
            );
        }
    }

    #[test]
    fn owner_names_are_not_special_cased_and_architectures_are_explicit() {
        for owner in ["Apis", "<Module>", "Outer+Inner"] {
            let mut ctx = context();
            ctx.owner = Some(owner.into());
            ctx.namespace.clear();
            let mut hit = item(
                "Method",
                "Method",
                "method",
                MatchQuality::NameExact(MatchField::Name),
            );
            hit.entity.architecture_mask = Architecture::X64.bit();
            let result = present(hit, ctx);
            assert_eq!(text(&result.display.detail), format!("{owner} · x64"));
        }
    }
}
