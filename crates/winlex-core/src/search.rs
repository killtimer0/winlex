mod guid;
mod number;
#[cfg(test)]
mod tests_metadata;
use crate::{Architecture, Declaration, EntitySummary, SearchMode};
use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{Atom, AtomKind, CaseMatching, Normalization},
};
pub use number::FloatDistance;
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

#[derive(Debug)]
pub struct SearchMatch {
    pub entity: EntitySummary,
    pub quality: MatchQuality,
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
    Float(FloatDistance),
    GuidFragment,
    NameScope,
    Fuzzy { score: u16, field: MatchField },
    Reference,
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
            Self::NameExact(_) | Self::Integer | Self::GuidExact | Self::Reference => 0,
            Self::Float(_) => 1,
            Self::GuidFragment => 2,
            Self::NameScope => 3,
            Self::Fuzzy { .. } => 4,
        };
        rank(self)
            .cmp(&rank(other))
            .then_with(|| match (self, other) {
                (Self::Float(a), Self::Float(b)) => a.cmp(b),
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

pub(crate) struct SearchQuery {
    name: Option<NameQuery>,
    number: Option<number::NumberQuery>,
    guid: Option<guid::GuidQuery>,
}
impl SearchQuery {
    pub fn compile(text: &str, mode: SearchMode, case_sensitive: bool) -> anyhow::Result<Self> {
        let mut query = Self {
            name: None,
            number: None,
            guid: None,
        };
        match mode {
            SearchMode::Combined => {
                query.name = Some(NameQuery::compile(text, true, case_sensitive));
                query.number = number::NumberQuery::compile(text).ok();
                query.guid = guid::GuidQuery::compile(text)
                    .ok()
                    .filter(|guid| guid.digit_count() >= 4);
            }
            SearchMode::Name => {
                anyhow::ensure!(
                    text != "." && text != "+",
                    "name scope requires a parent name"
                );
                query.name = Some(NameQuery::compile(text, false, case_sensitive));
            }
            SearchMode::Value => query.number = Some(number::NumberQuery::compile(text)?),
            SearchMode::Guid => query.guid = Some(guid::GuidQuery::compile(text)?),
        }
        Ok(query)
    }
    pub fn needs_full_name(&self) -> bool {
        self.name
            .as_ref()
            .is_some_and(|name| name.field == MatchField::FullName)
    }
    pub fn matches(
        &mut self,
        row: &Declaration<'_>,
        full_name: &str,
        architecture: Option<Architecture>,
    ) -> Option<MatchQuality> {
        let mut quality = self
            .name
            .as_mut()
            .and_then(|query| query.matches(row.name(), full_name));
        let mut accept = |hit: Option<MatchQuality>| {
            if let Some(hit) = hit
                && quality.as_ref().is_none_or(|old| hit.compare(old).is_lt())
            {
                quality = Some(hit);
            }
        };
        if let Some(query) = &self.number
            && let Declaration::Field(field) = row
            && let Some(constant) = field.constant()
        {
            accept(query.matches(&constant.value(), architecture, row.architecture_mask()));
        }
        if let Some(query) = &self.guid {
            let value = match row {
                Declaration::Type(row) => crate::guid(*row),
                Declaration::Field(row) => crate::guid(*row),
                _ => None,
            };
            accept(value.and_then(|value| query.matches(&value)));
        }
        quality
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
