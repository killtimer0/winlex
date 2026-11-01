use super::{
    Comparison, Literal, MatchField, MatchQuality, NameMatcher, Operator, Query, QueryError,
    guid::GuidQuery, number::NumberQuery,
};
use crate::{Architecture, Declaration, EntityCategory};
use windows_metadata::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceField {
    Name,
    FullName,
    Value,
    Guid,
    Namespace,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct MatchEvidence {
    pub field: EvidenceField,
    /// Unicode scalar indices in the metadata text; None highlights the whole value.
    pub indices: Option<Vec<u32>>,
}

pub(super) struct Hit {
    pub quality: MatchQuality,
    pub evidence: Vec<MatchEvidence>,
}
impl Hit {
    pub fn new(quality: MatchQuality, field: EvidenceField, indices: Option<Vec<u32>>) -> Self {
        Self {
            quality,
            evidence: vec![MatchEvidence { field, indices }],
        }
    }
    pub fn filter() -> Self {
        Self {
            quality: MatchQuality::Filter,
            evidence: Vec::new(),
        }
    }
    pub fn merge(&mut self, other: Self) {
        if other.quality.compare(&self.quality).is_lt() {
            self.quality = other.quality;
        }
        for mut evidence in other.evidence {
            if let Some(old) = self
                .evidence
                .iter_mut()
                .find(|old| old.field == evidence.field)
            {
                match (&mut old.indices, evidence.indices.take()) {
                    (Some(a), Some(b)) => {
                        a.extend(b);
                        a.sort_unstable();
                        a.dedup();
                    }
                    _ => old.indices = None,
                }
            } else {
                self.evidence.push(evidence);
            }
        }
    }
}

pub(super) enum TextQuery {
    Fuzzy(NameMatcher),
    Literal(LiteralQuery),
}
pub(super) struct LiteralQuery {
    pub text: String,
    op: Operator,
    case_sensitive: bool,
}
impl TextQuery {
    pub fn new(text: &str, op: Operator, case_sensitive: bool) -> Self {
        if op == Operator::Fuzzy {
            return Self::Fuzzy(NameMatcher::new(text, case_sensitive));
        }
        Self::Literal(LiteralQuery::new(text, op, case_sensitive))
    }
    pub fn matches(&mut self, text: &str, field: EvidenceField) -> Option<Hit> {
        match self {
            Self::Literal(query) => query.matches(text, field),
            Self::Fuzzy(matcher) => {
                let match_field = if field == EvidenceField::FullName {
                    MatchField::FullName
                } else {
                    MatchField::Name
                };
                let mut indices = Vec::new();
                let score = matcher.indices(text, &mut indices)?;
                Some(Hit::new(
                    MatchQuality::Fuzzy {
                        score,
                        field: match_field,
                    },
                    field,
                    Some(indices),
                ))
            }
        }
    }
}
impl LiteralQuery {
    pub fn new(text: &str, op: Operator, case_sensitive: bool) -> Self {
        Self {
            text: if case_sensitive {
                text.into()
            } else {
                text.to_lowercase()
            },
            op,
            case_sensitive,
        }
    }
    pub fn matches(&self, text: &str, field: EvidenceField) -> Option<Hit> {
        let (query, op, case_sensitive) = (self.text.as_str(), self.op, self.case_sensitive);
        let normalized;
        let value = if case_sensitive {
            text
        } else {
            normalized = text.to_lowercase();
            &normalized
        };
        let start = match op {
            Operator::Equal => (value == query).then_some(0),
            Operator::Contains => value.find(query),
            Operator::StartsWith => value.starts_with(query).then_some(0),
            Operator::Within => (value == query
                || value
                    .strip_prefix(query)
                    .is_some_and(|rest| rest.starts_with('.')))
            .then_some(0),
            _ => None,
        }?;
        let end = start + query.len();
        let mut offset = 0;
        let indices = text
            .chars()
            .enumerate()
            .filter_map(|(index, ch)| {
                let width = if case_sensitive {
                    ch.len_utf8()
                } else {
                    ch.to_lowercase().map(char::len_utf8).sum()
                };
                let selected = offset < end && offset + width > start;
                offset += width;
                selected.then_some(index as u32)
            })
            .collect();
        let quality = match field {
            EvidenceField::Name if value == query => MatchQuality::NameExact(MatchField::Name),
            EvidenceField::FullName if value == query => {
                MatchQuality::NameExact(MatchField::FullName)
            }
            EvidenceField::Value if value == query => MatchQuality::StringExact,
            _ => MatchQuality::Text,
        };
        Some(Hit::new(quality, field, Some(indices)))
    }
}

pub(super) enum Condition {
    Name(super::NameQuery),
    Text(EvidenceField, TextQuery),
    Number(NumberQuery, Comparison),
    Guid(GuidQuery),
    Category(EntityCategory),
    Source(usize),
    Architecture(Architecture),
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
}

impl Condition {
    pub fn compile(query: &Query, case_sensitive: bool) -> anyhow::Result<Self> {
        let mut nodes = 0;
        Self::compile_at(query, case_sensitive, 0, &mut nodes)
    }
    fn compile_at(
        query: &Query,
        case_sensitive: bool,
        depth: usize,
        nodes: &mut usize,
    ) -> anyhow::Result<Self> {
        *nodes += 1;
        anyhow::ensure!(
            depth < super::syntax::MAX_QUERY_DEPTH && *nodes <= super::syntax::MAX_QUERY_NODES,
            "query complexity limit"
        );
        match query {
            Query::All(terms) | Query::Any(terms) => {
                anyhow::ensure!(!terms.is_empty(), "empty query group");
                let terms = terms
                    .iter()
                    .map(|q| Self::compile_at(q, case_sensitive, depth + 1, nodes))
                    .collect::<anyhow::Result<Vec<_>>>()?;
                Ok(if matches!(query, Query::All(_)) {
                    Self::All(terms)
                } else {
                    Self::Any(terms)
                })
            }
            Query::Not(q) => Ok(Self::Not(Box::new(Self::compile_at(
                q,
                case_sensitive,
                depth + 1,
                nodes,
            )?))),
            Query::Predicate(p) => {
                use super::QueryErrorKind;
                let mut kind = QueryErrorKind::Value;
                let result = (|| {
                    let op = p.operator;
                    let text = || -> anyhow::Result<&str> {
                        let Literal::Text(text) = &p.value else {
                            anyhow::bail!("field requires a quoted string")
                        };
                        anyhow::ensure!(
                            text.len() <= super::syntax::MAX_QUERY_BYTES,
                            "text is too long"
                        );
                        Ok(text)
                    };
                    match (p.field.as_str(), &p.value) {
                        ("value", Literal::Number(text)) => {
                            kind = QueryErrorKind::Operator;
                            let comparison = match op {
                                Operator::Equal => Comparison::Equal,
                                Operator::Less => Comparison::Less,
                                Operator::LessEqual => Comparison::LessEqual,
                                Operator::Greater => Comparison::Greater,
                                Operator::GreaterEqual => Comparison::GreaterEqual,
                                _ => anyhow::bail!("number requires a comparison operator"),
                            };
                            kind = QueryErrorKind::Value;
                            let number = NumberQuery::compile(text)?;
                            number.validate_comparison(comparison)?;
                            Ok(Self::Number(number, comparison))
                        }
                        ("source", value) => {
                            kind = QueryErrorKind::Operator;
                            anyhow::ensure!(op == Operator::Equal, "source requires =");
                            kind = QueryErrorKind::Value;
                            let Literal::Number(text) = value else {
                                anyhow::bail!("source requires a zero-based integer ID")
                            };
                            anyhow::ensure!(
                                text.bytes().all(|c| c.is_ascii_digit()),
                                "invalid source ID"
                            );
                            Ok(Self::Source(text.parse()?))
                        }
                        ("name" | "fullname" | "value" | "namespace", _) => {
                            let text = text()?;
                            let field = match p.field.as_str() {
                                "name" => EvidenceField::Name,
                                "fullname" => EvidenceField::FullName,
                                "value" => EvidenceField::Value,
                                _ => EvidenceField::Namespace,
                            };
                            let valid = match op {
                                Operator::Equal => true,
                                Operator::Contains | Operator::StartsWith => {
                                    field != EvidenceField::Namespace
                                }
                                Operator::Fuzzy => {
                                    matches!(field, EvidenceField::Name | EvidenceField::FullName)
                                }
                                Operator::Within => field == EvidenceField::Namespace,
                                _ => false,
                            };
                            kind = QueryErrorKind::Operator;
                            anyhow::ensure!(valid, "operator is not supported for this field");
                            Ok(Self::Text(field, TextQuery::new(text, op, case_sensitive)))
                        }
                        ("guid", _) => {
                            let text = text()?;
                            kind = QueryErrorKind::Operator;
                            anyhow::ensure!(
                                matches!(op, Operator::Equal | Operator::Contains),
                                "GUID requires = or contains"
                            );
                            kind = QueryErrorKind::Value;
                            let guid = GuidQuery::compile(text)?;
                            anyhow::ensure!(
                                op != Operator::Equal || guid.digit_count() == 32,
                                "GUID equality requires a complete GUID"
                            );
                            Ok(Self::Guid(guid))
                        }
                        ("category", _) => {
                            let text = text()?;
                            kind = QueryErrorKind::Operator;
                            anyhow::ensure!(op == Operator::Equal, "category requires =");
                            kind = QueryErrorKind::Value;
                            let category = match text.to_ascii_lowercase().as_str() {
                                "type" => EntityCategory::Type,
                                "method" => EntityCategory::Method,
                                "constant" => EntityCategory::Constant,
                                "field" => EntityCategory::Field,
                                "property" => EntityCategory::Property,
                                "event" => EntityCategory::Event,
                                "type_reference" => EntityCategory::TypeReference,
                                _ => anyhow::bail!("unknown category"),
                            };
                            Ok(Self::Category(category))
                        }
                        ("architecture", _) => {
                            let text = text()?;
                            kind = QueryErrorKind::Operator;
                            anyhow::ensure!(op == Operator::Equal, "architecture requires =");
                            kind = QueryErrorKind::Value;
                            Ok(Self::Architecture(text.to_ascii_lowercase().parse()?))
                        }
                        _ => {
                            kind = QueryErrorKind::Field;
                            anyhow::bail!("unknown field: {}", p.field)
                        }
                    }
                })();
                result.map_err(|error: anyhow::Error| {
                    QueryError {
                        kind,
                        span: p.span.clone(),
                        message: error.to_string(),
                        incomplete: false,
                    }
                    .into()
                })
            }
        }
    }
    pub fn needs(&self, field: EvidenceField) -> bool {
        match self {
            Self::Name(query) => {
                field
                    == if query.field == MatchField::FullName {
                        EvidenceField::FullName
                    } else {
                        EvidenceField::Name
                    }
            }
            Self::Text(own, _) => *own == field,
            Self::Number(_, _) => field == EvidenceField::Value,
            Self::Guid(_) => field == EvidenceField::Guid,
            Self::All(terms) | Self::Any(terms) => terms.iter().any(|q| q.needs(field)),
            Self::Not(q) => q.needs(field),
            _ => false,
        }
    }
    pub fn matches(&mut self, candidate: &Candidate<'_, '_>) -> Option<Hit> {
        match self {
            Self::Name(query) => query.matches(candidate.row.name(), candidate.full_name),
            Self::All(terms) => {
                let mut hit = Hit::filter();
                for term in terms {
                    hit.merge(term.matches(candidate)?);
                }
                Some(hit)
            }
            Self::Any(terms) => {
                let mut hit = None;
                for term in terms {
                    if let Some(found) = term.matches(candidate) {
                        hit.get_or_insert_with(Hit::filter).merge(found);
                    }
                }
                hit
            }
            Self::Not(term) => term.matches(candidate).is_none().then(Hit::filter),
            Self::Text(field, query) => {
                let text = match field {
                    EvidenceField::Name => candidate.row.name(),
                    EvidenceField::FullName => candidate.full_name,
                    EvidenceField::Namespace => candidate.namespace,
                    EvidenceField::Value => string_value(candidate.value.as_ref()?)?,
                    EvidenceField::Guid => unreachable!(),
                };
                query.matches(text, *field)
            }
            Self::Number(query, op) => query
                .matches(
                    candidate.value.as_ref()?,
                    candidate.architecture,
                    candidate.row.architecture_mask(),
                    *op,
                )
                .map(|quality| Hit::new(quality, EvidenceField::Value, None)),
            Self::Guid(query) => query
                .matches(candidate.guid.as_deref()?)
                .map(|quality| Hit::new(quality, EvidenceField::Guid, None)),
            Self::Category(category) => (candidate.row.category() == *category).then(Hit::filter),
            Self::Source(source) => (candidate.source == *source).then(Hit::filter),
            Self::Architecture(arch) => arch
                .supports(candidate.row.architecture_mask())
                .then(Hit::filter),
        }
    }
}

pub(super) struct Candidate<'a, 'db> {
    pub row: &'a Declaration<'db>,
    pub full_name: &'a str,
    pub namespace: &'a str,
    pub source: usize,
    pub architecture: Option<Architecture>,
    pub value: Option<Value>,
    pub guid: Option<String>,
}

fn string_value(value: &Value) -> Option<&str> {
    match value {
        Value::Utf8(text) | Value::Utf16(text) => Some(text),
        Value::EnumValue(_, inner) => string_value(inner),
        _ => None,
    }
}
