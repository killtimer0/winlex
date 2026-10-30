//! Search presentation is prepared on the worker, never during list painting.
use crate::{icons::Icon, list::Display};
use std::{ops::Deref, ops::Range};
use winlex_core::{
    Architecture, Catalog, EntityContext, MatchField, MatchQuality, NameMatcher, SearchMatch,
};

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
    matcher: NameMatcher,
    query: String,
    indices: Vec<u32>,
}
impl Builder {
    pub fn new(query: &str) -> Self {
        Self {
            matcher: NameMatcher::new(query, false),
            query: query.to_lowercase(),
            indices: Vec::new(),
        }
    }
    pub fn prepare(
        &mut self,
        catalog: &Catalog,
        matched: SearchMatch,
    ) -> anyhow::Result<Box<ResultItem>> {
        let context = catalog.entity_context(matched.id)?;
        Ok(Box::new(self.present(matched, context)))
    }
    fn present(&mut self, matched: SearchMatch, context: EntityContext) -> ResultItem {
        self.indices.clear();
        let full = matched.quality.field() == Some(MatchField::FullName);
        let text = if full {
            &matched.full_name
        } else {
            &matched.name
        };
        match matched.quality {
            MatchQuality::NameExact(_) => self.indices.extend(0..text.chars().count() as u32),
            MatchQuality::NameScope => {
                let mut length = 0;
                for (index, ch) in text.chars().enumerate() {
                    if length >= self.query.len() {
                        break;
                    }
                    self.indices.push(index as u32);
                    length += ch.to_lowercase().map(char::len_utf8).sum::<usize>();
                }
            }
            MatchQuality::Fuzzy { .. } => {
                self.matcher.indices(text, &mut self.indices);
            }
            _ => {}
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
        let guid_match = matches!(
            matched.quality,
            MatchQuality::GuidExact | MatchQuality::GuidFragment
        );
        let value = if guid_match || context.constant.is_none() && context.guid.is_some() {
            Some("{...}".to_owned())
        } else {
            context.constant.as_ref().map(winlex_format::constant)
        };
        if let Some(value) = value {
            display.detail.extend("= ".encode_utf16());
            let start = display.detail.len();
            display.detail.extend(value.encode_utf16());
            if guid_match
                || matches!(
                    matched.quality,
                    MatchQuality::Integer | MatchQuality::Float(_)
                )
            {
                display.detail_matches.push(start..display.detail.len());
            }
        } else if let Some(owner) = &context.owner {
            let offset =
                context.namespace.chars().count() + usize::from(!context.namespace.is_empty());
            if full {
                display.detail_matches = project(owner, offset, &self.indices);
            }
            display.detail.extend(owner.encode_utf16());
        }
        let arches = if matched.architecture_mask == Architecture::ALL_MASK {
            String::new()
        } else {
            [Architecture::X86, Architecture::X64, Architecture::Arm64]
                .into_iter()
                .filter(|arch| arch.supports(matched.architecture_mask))
                .map(Architecture::name)
                .collect::<Vec<_>>()
                .join(" ")
        };
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
            append(&mut display.detail, &context.namespace);
        }
        ResultItem { matched, display }
    }
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
    use winlex_core::{EntityId, EntitySummary, windows_metadata::Value};

    fn item(name: &str, full_name: &str, kind: &str, quality: MatchQuality) -> SearchMatch {
        SearchMatch {
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
    fn text(value: &[u16]) -> String {
        String::from_utf16(value).unwrap()
    }

    #[test]
    fn names_and_qualified_matches_project_to_utf16_display_fields() {
        let mut builder = Builder::new("😀W");
        let result = builder.present(
            item(
                "😀窗口W",
                "N.Outer+Inner.😀窗口W",
                "method",
                MatchQuality::Fuzzy {
                    score: 1,
                    field: MatchField::Name,
                },
            ),
            context(),
        );
        assert_eq!(result.display.title_matches, [0..2, 4..5]);
        assert!(result.display.detail_matches.is_empty());
        assert_eq!(text(&result.display.detail), "Outer+Inner · N");
        assert_eq!(result.display.icon, Some(Icon::Method));

        let mut builder = Builder::new("N.O+I.W");
        let result = builder.present(
            item(
                "Window",
                "N.Outer+Inner.Window",
                "method",
                MatchQuality::Fuzzy {
                    score: 1,
                    field: MatchField::FullName,
                },
            ),
            context(),
        );
        assert_eq!(
            result.display.title_matches.as_slice(),
            std::slice::from_ref(&(0..1))
        );
        assert_eq!(result.display.detail_matches, [0..1, 5..7, 14..15]);

        let mut builder = Builder::new("N.Outer+");
        let result = builder.present(
            item(
                "Window",
                "N.Outer+Inner.Window",
                "method",
                MatchQuality::NameScope,
            ),
            context(),
        );
        assert!(result.display.title_matches.is_empty());
        assert_eq!(result.display.detail_matches, [0..6, 14..15]);
    }

    #[test]
    fn values_and_guid_placeholders_only_highlight_the_winning_branch() {
        let mut builder = Builder::new("5");
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
        assert_eq!(text(&result.display.detail), "= {...} · N");
        assert_eq!(
            result.display.detail_matches.as_slice(),
            std::slice::from_ref(&(2..7))
        );
        assert_eq!(result.display.icon, Some(Icon::Interface));

        let mut builder = Builder::new("VALUE");
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
            let result = Builder::new("N.").present(hit, ctx);
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
            let result = Builder::new("Method").present(hit, ctx);
            assert_eq!(text(&result.display.detail), format!("{owner} · x64"));
        }
    }
}
