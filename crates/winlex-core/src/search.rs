mod condition;
mod guid;
use condition::{Candidate, Condition, Hit, LiteralQuery, TextQuery};
pub use condition::{EvidenceField, MatchEvidence};
mod number;
mod syntax;
pub use syntax::{Literal, Operator, Predicate, Query, QueryError, QueryErrorKind};
#[cfg(test)]
mod tests_metadata;
use crate::{Architecture, Declaration, EntitySummary, SearchMode};
use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{Atom, AtomKind, CaseMatching, Normalization},
};
pub(crate) use number::Comparison;
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchField {
    Name,
    FullName,
}
impl MatchField {
    fn for_query(text: &str) -> Self {
        if text.contains(['.', '+']) {
            Self::FullName
        } else {
            Self::Name
        }
    }
    fn text<'a>(self, name: &'a str, full_name: &'a str) -> &'a str {
        match self {
            Self::Name => name,
            Self::FullName => full_name,
        }
    }
}

fn hex_digits(text: &str) -> Option<&str> {
    text.strip_prefix("0x").or_else(|| text.strip_prefix("0X"))
}

#[derive(Debug, serde::Serialize)]
pub struct SearchMatch {
    #[serde(flatten)]
    pub entity: EntitySummary,
    #[serde(skip)]
    pub quality: MatchQuality,
    pub evidence: Vec<MatchEvidence>,
}
impl std::ops::Deref for SearchMatch {
    type Target = EntitySummary;
    fn deref(&self) -> &Self::Target {
        &self.entity
    }
}
impl SearchMatch {
    pub fn compare(&self, other: &Self) -> Ordering {
        self.quality
            .compare(&other.quality)
            .then(self.id.cmp(&other.id))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MatchQuality {
    NameExact(MatchField),
    Integer,
    GuidExact,
    Float,
    GuidFragment,
    NameScope,
    Fuzzy { score: u16, field: MatchField },
    Reference,
    StringExact,
    Text,
    Filter,
}
impl MatchQuality {
    pub fn compare(&self, other: &Self) -> Ordering {
        let rank = |quality: &Self| match quality {
            Self::NameExact(_)
            | Self::Integer
            | Self::Float
            | Self::GuidExact
            | Self::Reference
            | Self::StringExact => 0,
            Self::Text => 3,
            Self::Filter => 5,
            Self::GuidFragment => 2,
            Self::NameScope => 3,
            Self::Fuzzy { .. } => 4,
        };
        rank(self)
            .cmp(&rank(other))
            .then_with(|| match (self, other) {
                (Self::Fuzzy { score: a, .. }, Self::Fuzzy { score: b, .. }) => b.cmp(a),
                _ => Ordering::Equal,
            })
    }
}

pub(super) struct NameQuery {
    literal: LiteralQuery,
    field: MatchField,
    scope: bool,
    fuzzy: Option<TextQuery>,
}
impl NameQuery {
    fn compile(text: &str, fuzzy: bool, case_sensitive: bool) -> Self {
        let scope = text.ends_with(['.', '+']);
        Self {
            literal: LiteralQuery::new(
                text,
                if scope {
                    Operator::StartsWith
                } else {
                    Operator::Equal
                },
                case_sensitive,
            ),
            field: MatchField::for_query(text),
            scope,
            fuzzy: (fuzzy && !scope).then(|| TextQuery::new(text, Operator::Fuzzy, case_sensitive)),
        }
    }
    fn matches(&mut self, name: &str, full_name: &str) -> Option<Hit> {
        let text = self.field.text(name, full_name);
        let field = if self.field == MatchField::Name {
            EvidenceField::Name
        } else {
            EvidenceField::FullName
        };
        if let Some(mut hit) = self.literal.matches(text, field) {
            if self.scope {
                if self.literal.text.len() <= 1 {
                    return None;
                }
                hit.quality = MatchQuality::NameScope;
            }
            return Some(hit);
        }
        self.fuzzy.as_mut()?.matches(text, field)
    }
}

pub struct SearchQuery {
    condition: Condition,
    filter_quality: bool,
    pub(crate) context_needed: bool,
    value_needed: bool,
    guid_needed: bool,
}
impl SearchQuery {
    pub fn compile(text: &str, mode: SearchMode, case_sensitive: bool) -> anyhow::Result<Self> {
        anyhow::ensure!(!text.is_empty(), "empty search query");
        anyhow::ensure!(text.len() <= syntax::MAX_QUERY_BYTES, "query is too long");
        let condition = match mode {
            SearchMode::Combined => {
                let mut terms = vec![Condition::Name(NameQuery::compile(
                    text,
                    true,
                    case_sensitive,
                ))];
                if let Ok(number) = number::NumberQuery::compile(text) {
                    terms.push(Condition::Number(number, Comparison::Equal));
                }
                terms.push(Condition::Text(
                    EvidenceField::Value,
                    TextQuery::new(text, Operator::Contains, case_sensitive),
                ));
                if let Ok(guid) = guid::GuidQuery::compile(text)
                    && guid.digit_count() >= 4
                {
                    terms.push(Condition::Guid(guid));
                }
                Condition::Any(terms)
            }
            SearchMode::Name => {
                anyhow::ensure!(
                    text != "." && text != "+",
                    "name scope requires a parent name"
                );
                Condition::Name(NameQuery::compile(text, false, case_sensitive))
            }
            SearchMode::Value => {
                Condition::Number(number::NumberQuery::compile(text)?, Comparison::Equal)
            }
            SearchMode::Guid => Condition::Guid(guid::GuidQuery::compile(text)?),
            SearchMode::Condition => return Self::from_query(&Query::parse(text)?, case_sensitive),
        };
        Ok(Self::new(condition, false))
    }
    pub fn from_query(query: &Query, case_sensitive: bool) -> anyhow::Result<Self> {
        let condition = Condition::compile(query, case_sensitive)?;
        let filter_quality = !matches!(condition, Condition::Text(_, _));
        Ok(Self::new(condition, filter_quality))
    }
    fn new(condition: Condition, filter_quality: bool) -> Self {
        Self {
            context_needed: condition.needs(EvidenceField::FullName)
                || condition.needs(EvidenceField::Namespace),
            value_needed: condition.needs(EvidenceField::Value),
            guid_needed: condition.needs(EvidenceField::Guid),
            condition,
            filter_quality,
        }
    }
    pub(crate) fn matches(
        &mut self,
        row: &Declaration<'_>,
        full_name: &str,
        namespace: &str,
        source: usize,
        architecture: Option<Architecture>,
    ) -> Option<(MatchQuality, Vec<MatchEvidence>)> {
        let value = self.value_needed.then(|| row.constant_value()).flatten();
        let guid = self.guid_needed.then(|| row.guid()).flatten();
        let candidate = Candidate {
            row,
            full_name,
            namespace,
            source,
            architecture,
            value,
            guid,
        };
        self.condition.matches(&candidate).map(|hit| {
            let quality = if self.filter_quality {
                MatchQuality::Filter
            } else {
                hit.quality
            };
            (quality, hit.evidence)
        })
    }
}

struct NameMatcher {
    atom: Atom,
    matcher: Matcher,
    buffer: Vec<char>,
}
impl NameMatcher {
    fn new(query: &str, case_sensitive: bool) -> Self {
        Self {
            atom: Atom::new(
                query,
                if case_sensitive {
                    CaseMatching::Respect
                } else {
                    CaseMatching::Ignore
                },
                Normalization::Never,
                AtomKind::Fuzzy,
                false,
            ),
            matcher: Matcher::new(Config::DEFAULT),
            buffer: Vec::new(),
        }
    }
    /// Character indices in the original matched field, not UTF-8 or UTF-16 offsets.
    fn indices(&mut self, text: &str, indices: &mut Vec<u32>) -> Option<u16> {
        indices.clear();
        self.atom.indices(
            Utf32Str::new(text, &mut self.buffer),
            &mut self.matcher,
            indices,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_fuzzy_matching_and_highlight_agree() {
        let mut matcher = NameMatcher::new("CW", false);
        let hit = NameQuery::compile("CW", true, false)
            .matches("CreateWindow", "Win32.CreateWindow")
            .unwrap();
        let MatchQuality::Fuzzy { score, field } = hit.quality else {
            panic!("expected a fuzzy match")
        };
        assert_eq!(field, MatchField::Name);
        let text = "CreateWindow";
        let mut indices = vec![999];
        assert_eq!(matcher.indices(text, &mut indices), Some(score));
        assert_eq!(hit.evidence[0].indices.as_deref(), Some(indices.as_slice()));
        assert_eq!(
            indices
                .iter()
                .map(|&i| text.chars().nth(i as usize).unwrap())
                .collect::<String>(),
            "CW"
        );
        assert!(matcher.indices("MessageBox", &mut indices).is_none());
        let mut literal = NameMatcher::new("^File", false);
        assert!(literal.indices("File", &mut indices).is_none());
        assert!(literal.indices("^File", &mut indices).is_some());
    }
    #[test]
    fn unicode_indices() {
        let mut matcher = NameMatcher::new("窗口", true);
        let mut indices = vec![];
        assert!(matcher.indices("窗口", &mut indices).is_some());
        assert_eq!(indices, [0, 1]);
        let mut matcher = NameMatcher::new("😀W", false);
        assert!(matcher.indices("😀窗口W", &mut indices).is_some());
        assert_eq!(indices, [0, 3]);
        let mut matcher = NameMatcher::new("W", false);
        assert!(matcher.indices("e\u{301}W", &mut indices).is_some());
        assert_eq!(indices, [2]);
    }
    #[test]
    fn continuous_and_boundary_matches_outrank_scattered_characters() {
        let mut matcher = NameMatcher::new("CreateFileW", false);
        let mut indices = vec![];
        let exact = matcher.indices("CreateFileW", &mut indices).unwrap();
        let scattered = matcher
            .indices(
                "CxxxxrxxxxexxxxaxxxxtxxxxexxxxFxxxxixxxxlxxxxexxxxW",
                &mut indices,
            )
            .unwrap();
        assert!(exact > scattered);
        let mut matcher = NameMatcher::new("CW", false);
        let boundary = matcher.indices("CreateWindow", &mut indices).unwrap();
        let scattered = matcher.indices("acxxxxxxxxw", &mut indices).unwrap();
        assert!(boundary > scattered);
        let mut query = NameQuery::compile("Win32.File", true, false);
        assert_eq!(
            query.matches("File", "Win32.File").unwrap().quality,
            MatchQuality::NameExact(MatchField::FullName)
        );
        let mut matcher = NameMatcher::new("createfilew", true);
        assert!(matcher.indices("CreateFileW", &mut indices).is_none());
    }

    #[test]
    fn unqualified_queries_never_fall_back_to_the_full_name() {
        let mut query = NameQuery::compile("NwCW", true, false);
        assert!(
            query
                .matches("CreateWindow", "Namespace.CreateWindow")
                .is_none()
        );
        let mut query = NameQuery::compile("Namespace", true, false);
        assert!(query.matches("File", "Namespace.File").is_none());
        let mut query = NameQuery::compile("CW", true, false);
        let hit = query.matches("CreateWindow", "CW.CreateWindow").unwrap();
        assert!(matches!(
            hit.quality,
            MatchQuality::Fuzzy {
                field: MatchField::Name,
                ..
            }
        ));
        let other = query.matches("CreateWindow", "Other.CreateWindow").unwrap();
        assert_eq!(other.quality, hit.quality);
        assert_eq!(other.evidence, hit.evidence);
    }

    #[test]
    fn qualified_queries_use_only_the_full_name_and_highlight_it() {
        for (query, text) in [
            ("N.CW", "Namespace.CreateWindow"),
            ("O+I", "Namespace.Outer+Inner"),
        ] {
            let mut matcher = NameMatcher::new(query, false);
            let mut name_query = NameQuery::compile(query, true, false);
            let hit = name_query.matches("Unrelated", text).unwrap();
            let MatchQuality::Fuzzy { score, field } = hit.quality else {
                panic!("expected a fuzzy match")
            };
            assert_eq!(field, MatchField::FullName);
            let mut indices = vec![];
            assert_eq!(matcher.indices(text, &mut indices), Some(score));
            assert_eq!(hit.evidence[0].field, EvidenceField::FullName);
            assert_eq!(hit.evidence[0].indices.as_deref(), Some(indices.as_slice()));
            assert_eq!(
                indices
                    .iter()
                    .map(|&i| text.chars().nth(i as usize).unwrap())
                    .collect::<String>(),
                query
            );
            assert!(name_query.matches(query, "Unrelated").is_none());
        }
    }
}
