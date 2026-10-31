//! Search presentation is prepared on the worker, never during list painting.
use crate::{
    icons::Icon,
    list::{Display, Fold, FoldKind, tooltip_field},
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
impl ResultItem {
    pub fn compare(&self, other: &Self) -> std::cmp::Ordering {
        self.matched.compare(&other.matched)
    }
}

pub struct Builder {
    indices: Vec<u32>,
}
impl Builder {
    pub fn new() -> Self {
        Self {
            indices: Vec::new(),
        }
    }
    pub fn prepare(
        &mut self,
        catalog: &Catalog,
        matched: SearchMatch,
    ) -> anyhow::Result<Box<ResultItem>> {
        let context = catalog.entity_context(matched.id)?;
        let source = &catalog.sources()[matched.id.source_id];
        let tip = tooltip(&matched, &context);
        let mut item = self.present(matched, context);
        item.display.tooltip = tip;
        tooltip_field(
            &mut item.display.tooltip,
            IDS_TIP_PATH,
            source.path.as_os_str().encode_wide(),
        );
        Ok(Box::new(item))
    }
    fn present(&mut self, matched: SearchMatch, context: EntityContext) -> ResultItem {
        self.indices.clear();
        let full = matched
            .evidence
            .iter()
            .any(|e| e.field == EvidenceField::FullName);
        let text = if full {
            &matched.full_name
        } else {
            &matched.name
        };
        if !matched.evidence.is_empty() {
            let field = if full {
                EvidenceField::FullName
            } else {
                EvidenceField::Name
            };
            for evidence in &matched.evidence {
                if evidence.field == field {
                    if let Some(indices) = &evidence.indices {
                        self.indices.extend(indices);
                    } else {
                        self.indices.extend(0..text.chars().count() as u32);
                    }
                } else if full && evidence.field == EvidenceField::Name {
                    let offset =
                        (matched.full_name.chars().count() - matched.name.chars().count()) as u32;
                    if let Some(indices) = &evidence.indices {
                        self.indices.extend(indices.iter().map(|i| i + offset));
                    } else {
                        self.indices.extend(offset..text.chars().count() as u32);
                    }
                }
            }
            self.indices.sort_unstable();
            self.indices.dedup();
        }
        let title_offset = if full {
            matched.full_name.chars().count() - matched.name.chars().count()
        } else {
            0
        };
        let mut display = Display {
            title: matched.name.encode_utf16().collect(),
            title_matches: project(&matched.name, title_offset, &self.indices),
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
            if !show_guid {
                display.detail.extend("= ".encode_utf16());
            }
            let start = display.detail.len();
            display.detail.extend(value.encode_utf16());
            if show_guid {
                display.detail_folds.push(Fold {
                    range: start..display.detail.len(),
                    kind: FoldKind::Guid,
                });
            }
            if guid_match && show_guid {
                display.detail_matches.push(start..display.detail.len());
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
                        let mut offset = start + 1;
                        for (index, ch) in text.chars().enumerate() {
                            let width = if ch == '\'' {
                                1
                            } else {
                                ch.escape_debug().map(char::len_utf16).sum::<usize>()
                            };
                            if indices.binary_search(&(index as u32)).is_ok() {
                                display.detail_matches.push(offset..offset + width);
                            }
                            offset += width;
                        }
                    }
                    _ => display.detail_matches.push(start..display.detail.len()),
                }
            }
        } else if let Some(owner) = &context.owner {
            let offset =
                context.namespace.chars().count() + usize::from(!context.namespace.is_empty());
            if full {
                display.detail_matches = project(owner, offset, &self.indices);
            }
            display.detail.extend(owner.encode_utf16());
        }
        if guid_match
            && !show_guid
            && let Some(guid) = &context.guid
        {
            let start = display.detail.len() + usize::from(!display.detail.is_empty()) * 3;
            append(&mut display.detail, &format!("{{{guid}}}"));
            display.detail_matches.push(start..display.detail.len());
            display.detail_folds.push(Fold {
                range: start..display.detail.len(),
                kind: FoldKind::Guid,
            });
        }
        let arches = architectures(matched.architecture_mask);
        append(&mut display.detail, &arches);
        if !context.namespace.is_empty() {
            let start = display.detail.len() + usize::from(!display.detail.is_empty()) * 3;
            if full {
                display.detail_matches.extend(
                    project(&context.namespace, 0, &self.indices)
                        .into_iter()
                        .map(|r| start + r.start..start + r.end),
                );
            }
            for evidence in &matched.evidence {
                if evidence.field == EvidenceField::Namespace
                    && let Some(indices) = &evidence.indices
                {
                    display.detail_matches.extend(
                        project(&context.namespace, 0, indices)
                            .into_iter()
                            .map(|r| start + r.start..start + r.end),
                    );
                }
            }
            append(&mut display.detail, &context.namespace);
        }
        display
            .detail_matches
            .sort_unstable_by_key(|r| (r.start, r.end));
        let mut merged: Vec<Range<usize>> = Vec::new();
        for range in display.detail_matches.drain(..) {
            if let Some(last) = merged.last_mut()
                && range.start <= last.end
            {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        display.detail_matches = merged;
        ResultItem { matched, display }
    }
}

fn architectures(mask: i32) -> String {
    if mask == Architecture::ALL_MASK {
        String::new()
    } else {
        [Architecture::X86, Architecture::X64, Architecture::Arm64]
            .into_iter()
            .filter(|arch| arch.supports(mask))
            .map(Architecture::name)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn tooltip(matched: &SearchMatch, context: &EntityContext) -> Vec<u16> {
    let mut text: Vec<u16> = matched.full_name.encode_utf16().collect();
    text.extend("\r\n".encode_utf16());
    tooltip_field(&mut text, IDS_TIP_KIND, matched.kind.encode_utf16());
    if !context.namespace.is_empty() {
        tooltip_field(
            &mut text,
            IDS_TIP_NAMESPACE,
            context.namespace.encode_utf16(),
        );
    }
    if let Some(owner) = &context.owner {
        tooltip_field(&mut text, IDS_TIP_OWNER, owner.encode_utf16());
    }
    if let Some(value) = &context.constant {
        tooltip_field(
            &mut text,
            IDS_TIP_VALUE,
            winlex_format::constant(value).encode_utf16(),
        );
    }
    if let Some(guid) = &context.guid {
        tooltip_field(&mut text, IDS_TIP_GUID, guid.encode_utf16());
    }
    let arches = architectures(matched.architecture_mask);
    if !arches.is_empty() {
        tooltip_field(&mut text, IDS_TIP_ARCHITECTURE, arches.encode_utf16());
    }
    text
}

fn append(line: &mut Vec<u16>, text: &str) {
    if text.is_empty() {
        return;
    }
    if !line.is_empty() {
        line.extend(" · ".encode_utf16());
    }
    line.extend(text.encode_utf16());
}

fn project(text: &str, offset: usize, indices: &[u32]) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut position = 0;
    for (index, ch) in text.chars().enumerate() {
        let end = position + ch.len_utf16();
        if indices.binary_search(&((offset + index) as u32)).is_ok() {
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

    fn item(name: &str, full_name: &str, kind: &str, quality: MatchQuality) -> SearchMatch {
        SearchMatch {
            evidence: match &quality {
                MatchQuality::NameExact(field) => vec![MatchEvidence {
                    field: if *field == MatchField::Name {
                        EvidenceField::Name
                    } else {
                        EvidenceField::FullName
                    },
                    indices: None,
                }],
                MatchQuality::Integer => vec![MatchEvidence {
                    field: EvidenceField::Value,
                    indices: None,
                }],
                MatchQuality::GuidFragment => vec![MatchEvidence {
                    field: EvidenceField::Guid,
                    indices: None,
                }],
                MatchQuality::NameScope => vec![MatchEvidence {
                    field: EvidenceField::FullName,
                    indices: Some(vec![0, 1]),
                }],
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
        hit.evidence = vec![MatchEvidence {
            field,
            indices: Some(indices),
        }];
        hit
    }
    fn text(value: &[u16]) -> String {
        String::from_utf16(value).unwrap()
    }

    #[test]
    fn names_and_qualified_matches_project_to_utf16_display_fields() {
        let mut builder = Builder::new();
        let result = builder.present(
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
        assert!(result.display.detail_matches.is_empty());
        assert_eq!(text(&result.display.detail), "Outer+Inner · N");
        assert_eq!(result.display.icon, Some(Icon::Method));

        let mut builder = Builder::new();
        let result = builder.present(
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
        assert_eq!(result.display.detail_matches, [0..1, 5..7, 14..15]);

        let mut builder = Builder::new();
        let result = builder.present(
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
        assert_eq!(result.display.detail_matches, [0..6, 14..15]);
    }

    #[test]
    fn values_and_guids_highlight_reported_evidence() {
        let mut builder = Builder::new();
        let mut ctx = context();
        ctx.constant = Some(Value::I32(5));
        let result = builder.present(
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
            result.display.detail_matches.as_slice(),
            std::slice::from_ref(&(2..3))
        );
        assert!(result.display.title_matches.is_empty());
        assert_eq!(result.display.icon, Some(Icon::Constant));

        let mut ctx = context();
        ctx.guid = Some("12345678-9abc-def0-1234-56789abcdef0".into());
        let result = builder.present(
            item("IXxx", "N.IXxx", "interface", MatchQuality::GuidFragment),
            ctx,
        );
        assert_eq!(
            text(&result.display.detail),
            "{12345678-9abc-def0-1234-56789abcdef0} · N"
        );
        assert_eq!(
            result.display.detail_matches.as_slice(),
            std::slice::from_ref(&(0..38))
        );
        assert_eq!(result.display.icon, Some(Icon::Interface));

        let mut builder = Builder::new();
        let mut ctx = context();
        ctx.constant = Some(Value::I32(5));
        ctx.guid = Some("12345678-9abc-def0-1234-56789abcdef0".into());
        let result = builder.present(
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
        assert!(result.display.detail_matches.is_empty());
    }

    #[test]
    fn multiple_fields_and_escaped_strings_keep_independent_highlights() {
        let mut hit = item("ACCESS", "N.ACCESS", "constant", MatchQuality::Filter);
        hit.evidence = vec![
            MatchEvidence {
                field: EvidenceField::Name,
                indices: Some(vec![0, 1, 2, 3, 4, 5]),
            },
            MatchEvidence {
                field: EvidenceField::Value,
                indices: Some(vec![1, 2, 3, 4]),
            },
        ];
        let mut ctx = context();
        ctx.constant = Some(Value::Utf16("a\n😀'\\z".into()));
        let result = Builder::new().present(hit, ctx);
        assert_eq!(text(&result.display.detail), "= \"a\\n😀'\\\\z\" · N");
        assert_eq!(result.display.title_matches, vec![0..6]);
        assert_eq!(result.display.detail_matches, vec![4..11]);

        let mut hit = item("ACCESS", "N.ACCESS", "constant", MatchQuality::Filter);
        hit.evidence = vec![
            MatchEvidence {
                field: EvidenceField::Value,
                indices: None,
            },
            MatchEvidence {
                field: EvidenceField::Guid,
                indices: None,
            },
        ];
        let mut ctx = context();
        ctx.constant = Some(Value::I32(5));
        ctx.guid = Some("12345678-9abc-def0-1234-56789abcdef0".into());
        let result = Builder::new().present(hit, ctx);
        assert_eq!(
            text(&result.display.detail),
            "= 5 · {12345678-9abc-def0-1234-56789abcdef0} · N"
        );
        assert_eq!(result.display.detail_matches, vec![2..3, 6..44]);
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
            let result = Builder::new().present(hit, ctx);
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
                result.display.detail_matches.as_slice(),
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
            let result = Builder::new().present(hit, ctx);
            assert_eq!(text(&result.display.detail), format!("{owner} · x64"));
        }
    }
}
