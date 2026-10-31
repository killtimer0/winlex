mod condition;
mod guid;
use condition::{Candidate, Condition, Hit, TextQuery};
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
    anyhow::ensure!(
        !digits.is_empty() && digits.bytes().all(|b| (b as char).is_digit(radix)),
        "invalid unsigned integer"
    );
    Ok(u64::from_str_radix(digits, radix)?)
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

struct NameQuery {
    text: String,
    case_sensitive: bool,
    field: MatchField,
    scope: bool,
    fuzzy: Option<NameMatcher>,
}
impl NameQuery {
    fn compile(text: &str, fuzzy: bool, case_sensitive: bool) -> Self {
        let scope = text.ends_with(['.', '+']);
        Self {
            text: if case_sensitive {
                text.to_owned()
            } else {
                text.to_lowercase()
            },
            case_sensitive,
            field: MatchField::for_query(text),
            scope,
            fuzzy: (fuzzy && !scope).then(|| NameMatcher::new(text, case_sensitive)),
        }
    }
    fn matches(&mut self, name: &str, full_name: &str) -> Option<MatchQuality> {
        let text = self.field.text(name, full_name);
        let normalized;
        let comparable = if self.case_sensitive {
            text
        } else {
            normalized = text.to_lowercase();
            &normalized
        };
        if self.scope {
            return (self.text.len() > 1 && comparable.starts_with(&self.text))
                .then_some(MatchQuality::NameScope);
        }
        if comparable == self.text {
            return Some(MatchQuality::NameExact(self.field));
        }
        self.fuzzy
            .as_mut()?
            .score(name, full_name)
            .map(|(score, field)| MatchQuality::Fuzzy { score, field })
    }
}

pub struct SearchQuery {
    matching: Matching,
    context_needed: bool,
    value_needed: bool,
    guid_needed: bool,
}
enum Matching {
    Simple {
        name: Option<NameQuery>,
        number: Option<number::NumberQuery>,
        guid: Option<guid::GuidQuery>,
        string: Option<TextQuery>,
    },
    Condition(Condition),
}
impl SearchQuery {
    pub fn compile(text: &str, mode: SearchMode, case_sensitive: bool) -> anyhow::Result<Self> {
        anyhow::ensure!(!text.is_empty(), "empty search query");
        anyhow::ensure!(text.len() <= syntax::MAX_QUERY_BYTES, "query is too long");
        let (name, number, guid, string) = match mode {
            SearchMode::Combined => (
                Some(NameQuery::compile(text, true, case_sensitive)),
                number::NumberQuery::compile(text).ok(),
                guid::GuidQuery::compile(text)
                    .ok()
                    .filter(|guid| guid.digit_count() >= 4),
                Some(TextQuery::new(text, Operator::Contains, case_sensitive)),
            ),
            SearchMode::Name => {
                anyhow::ensure!(
                    text != "." && text != "+",
                    "name scope requires a parent name"
                );
                (
                    Some(NameQuery::compile(text, false, case_sensitive)),
                    None,
                    None,
                    None,
                )
            }
            SearchMode::Value => (None, Some(number::NumberQuery::compile(text)?), None, None),
            SearchMode::Guid => (None, None, Some(guid::GuidQuery::compile(text)?), None),
            SearchMode::Condition => return Self::from_query(&Query::parse(text)?, case_sensitive),
        };
        Ok(Self {
            context_needed: name
                .as_ref()
                .is_some_and(|q| q.field == MatchField::FullName),
            value_needed: number.is_some() || string.is_some(),
            guid_needed: guid.is_some(),
            matching: Matching::Simple {
                name,
                number,
                guid,
                string,
            },
        })
    }
    pub fn from_query(query: &Query, case_sensitive: bool) -> anyhow::Result<Self> {
        let condition = Condition::compile(query, case_sensitive)?;
        Ok(Self {
            context_needed: condition.needs(EvidenceField::FullName)
                || condition.needs(EvidenceField::Namespace),
            value_needed: condition.needs(EvidenceField::Value),
            guid_needed: condition.needs(EvidenceField::Guid),
            matching: Matching::Condition(condition),
        })
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
        let value = if self.value_needed {
            match row {
                Declaration::Field(field) => field.constant().map(|v| v.value()),
                _ => None,
            }
        } else {
            None
        };
        let guid = if self.guid_needed {
            match row {
                Declaration::Type(row) => crate::guid(*row),
                Declaration::Field(row) => crate::guid(*row),
                _ => None,
            }
        } else {
            None
        };
        let (name, number, guid_query, string) = match &mut self.matching {
            Matching::Simple {
                name,
                number,
                guid,
                string,
            } => (name, number, guid, string),
            Matching::Condition(condition) => {
                let string = value.as_ref().and_then(condition::string_value);
                let candidate = Candidate {
                    row,
                    full_name,
                    namespace,
                    source,
                    architecture,
                    value,
                    string,
                    guid,
                };
                return condition.matches(&candidate).map(|mut hit| {
                    if !matches!(condition, Condition::Text(_, _)) {
                        hit.quality = MatchQuality::Filter;
                    }
                    (hit.quality, hit.evidence)
                });
            }
        };
        let quality = name
            .as_mut()
            .and_then(|query| query.matches(row.name(), full_name));
        let mut hit = quality.map(|quality| {
            let query = name.as_mut().unwrap();
            let text = query.field.text(row.name(), full_name);
            let mut indices = Vec::new();
            match quality {
                MatchQuality::Fuzzy { .. } => {
                    query.fuzzy.as_mut().unwrap().indices(text, &mut indices);
                }
                MatchQuality::NameScope => {
                    let mut bytes = 0;
                    for (i, ch) in text.chars().enumerate() {
                        if bytes >= query.text.len() {
                            break;
                        }
                        indices.push(i as u32);
                        bytes += if query.case_sensitive {
                            ch.len_utf8()
                        } else {
                            ch.to_lowercase().map(char::len_utf8).sum()
                        };
                    }
                }
                _ => indices.extend(0..text.chars().count() as u32),
            }
            Hit {
                quality,
                evidence: vec![MatchEvidence {
                    field: if query.field == MatchField::Name {
                        EvidenceField::Name
                    } else {
                        EvidenceField::FullName
                    },
                    indices: Some(indices),
                }],
            }
        });
        let mut accept = |found: Option<Hit>| {
            if let Some(found) = found {
                hit.get_or_insert_with(Hit::filter).merge(found);
            }
        };
        if let Some(query) = number
            && let Some(value) = &value
        {
            accept(
                query
                    .matches(value, architecture, row.architecture_mask())
                    .map(|quality| Hit {
                        quality,
                        evidence: vec![MatchEvidence {
                            field: EvidenceField::Value,
                            indices: None,
                        }],
                    }),
            );
        }
        if let Some(query) = string
            && let Some(value) = value.as_ref().and_then(condition::string_value)
        {
            accept(query.matches(&value, EvidenceField::Value));
        }
        if let Some(query) = guid_query
            && let Some(value) = &guid
        {
            accept(query.matches(value).map(|quality| Hit {
                quality,
                evidence: vec![MatchEvidence {
                    field: EvidenceField::Guid,
                    indices: None,
                }],
            }));
        }
        hit.map(|hit| (hit.quality, hit.evidence))
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
