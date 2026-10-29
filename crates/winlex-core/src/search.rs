use crate::EntitySummary;
use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{Atom, AtomKind, CaseMatching, Normalization},
};
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchField {
    Name,
    FullName,
}

#[derive(Debug)]
pub struct SearchMatch {
    pub entity: EntitySummary,
    /// Fuzzy relevance score; other query modes use zero.
    pub score: u16,
    /// Matched name field; not meaningful for value, GUID or reference queries.
    pub field: MatchField,
}
impl std::ops::Deref for SearchMatch {
    type Target = EntitySummary;
    fn deref(&self) -> &Self::Target {
        &self.entity
    }
}
impl SearchMatch {
    pub fn compare(&self, other: &Self) -> Ordering {
        other.score.cmp(&self.score).then(self.id.cmp(&other.id))
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
            field: if query.contains(['.', '+']) {
                MatchField::FullName
            } else {
                MatchField::Name
            },
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
        let text = match self.field {
            MatchField::Name => name,
            MatchField::FullName => full_name,
        };
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
