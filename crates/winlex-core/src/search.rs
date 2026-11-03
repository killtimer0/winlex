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
pub use number::Comparison;
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

fn parse_unsigned(digits: &str, radix: u32) -> anyhow::Result<u64> {
    crate::number::parse_unsigned(digits, radix)
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
    pub fn field(&self) -> Option<MatchField> {
        match self {
            Self::NameExact(field) | Self::Fuzzy { field, .. } => Some(*field),
            Self::NameScope => Some(MatchField::FullName),
            _ => None,
        }
    }
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
    context_needed: bool,
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
    pub fn needs_full_name(&self) -> bool {
        self.context_needed
    }
    pub fn matches(
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

pub struct NameMatcher {
    field: MatchField,
    atom: Atom,
    matcher: Matcher,
    buffer: Vec<char>,
}
impl NameMatcher {
    pub fn new(query: &str, case_sensitive: bool) -> Self {
        Self {
            field: MatchField::for_query(query),
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
    pub fn score(&mut self, name: &str, full_name: &str) -> Option<(u16, MatchField)> {
        let text = self.field.text(name, full_name);
        self.atom
            .score(Utf32Str::new(text, &mut self.buffer), &mut self.matcher)
            .map(|score| (score, self.field))
    }
    /// Character indices in the original matched field, not UTF-8 or UTF-16 offsets.
    pub fn indices(&mut self, text: &str, indices: &mut Vec<u32>) -> Option<u16> {
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
        let (score, field) = matcher.score("CreateWindow", "Win32.CreateWindow").unwrap();
        let text = if field == MatchField::Name {
            "CreateWindow"
        } else {
            "Win32.CreateWindow"
        };
        let mut indices = vec![999];
        assert_eq!(matcher.indices(text, &mut indices), Some(score));
        assert_eq!(
            indices
                .iter()
                .map(|&i| text.chars().nth(i as usize).unwrap())
                .collect::<String>(),
            "CW"
        );
        assert!(matcher.score("MessageBox", "Win32.MessageBox").is_none());
        let mut literal = NameMatcher::new("^File", false);
        assert!(literal.score("File", "File").is_none());
        assert!(literal.score("^File", "^File").is_some());
    }
    #[test]
    fn unicode_indices() {
        let mut matcher = NameMatcher::new("窗口", true);
        assert!(matcher.score("窗口", "命名空间.窗口").is_some());
        let mut matcher = NameMatcher::new("😀W", false);
        let mut indices = vec![];
        assert!(matcher.indices("😀窗口W", &mut indices).is_some());
        assert_eq!(indices, [0, 3]);
        let mut matcher = NameMatcher::new("W", false);
        assert!(matcher.indices("e\u{301}W", &mut indices).is_some());
        assert_eq!(indices, [2]);
    }
    #[test]
    fn continuous_and_boundary_matches_outrank_scattered_characters() {
        let mut matcher = NameMatcher::new("CreateFileW", false);
        let exact = matcher.score("CreateFileW", "CreateFileW").unwrap().0;
        let scattered = matcher
            .score(
                "CxxxxrxxxxexxxxaxxxxtxxxxexxxxFxxxxixxxxlxxxxexxxxW",
                "CxxxxrxxxxexxxxaxxxxtxxxxexxxxFxxxxixxxxlxxxxexxxxW",
            )
            .unwrap()
            .0;
        assert!(exact > scattered);
        let mut matcher = NameMatcher::new("CW", false);
        let boundary = matcher.score("CreateWindow", "CreateWindow").unwrap().0;
        let scattered = matcher.score("acxxxxxxxxw", "acxxxxxxxxw").unwrap().0;
        assert!(boundary > scattered);
        let mut matcher = NameMatcher::new("Win32.File", false);
        assert_eq!(
            matcher.score("File", "Win32.File").unwrap().1,
            MatchField::FullName
        );
        let mut matcher = NameMatcher::new("createfilew", true);
        assert!(matcher.score("CreateFileW", "CreateFileW").is_none());
    }

    #[test]
    fn unqualified_queries_never_fall_back_to_the_full_name() {
        let mut matcher = NameMatcher::new("NwCW", false);
        assert!(
            matcher
                .score("CreateWindow", "Namespace.CreateWindow")
                .is_none()
        );
        let mut matcher = NameMatcher::new("Namespace", false);
        assert!(matcher.score("File", "Namespace.File").is_none());
        let mut matcher = NameMatcher::new("CW", false);
        let hit = matcher.score("CreateWindow", "CW.CreateWindow").unwrap();
        assert_eq!(hit.1, MatchField::Name);
        assert_eq!(
            matcher.score("CreateWindow", "Other.CreateWindow"),
            Some(hit)
        );
    }

    #[test]
    fn qualified_queries_use_only_the_full_name_and_highlight_it() {
        for (query, text) in [
            ("N.CW", "Namespace.CreateWindow"),
            ("O+I", "Namespace.Outer+Inner"),
        ] {
            let mut matcher = NameMatcher::new(query, false);
            let (score, field) = matcher.score("Unrelated", text).unwrap();
            assert_eq!(field, MatchField::FullName);
            let mut indices = vec![];
            assert_eq!(matcher.indices(text, &mut indices), Some(score));
            assert_eq!(
                indices
                    .iter()
                    .map(|&i| text.chars().nth(i as usize).unwrap())
                    .collect::<String>(),
                query
            );
            assert!(matcher.score(query, "Unrelated").is_none());
        }
    }
}
