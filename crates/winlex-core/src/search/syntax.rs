use std::ops::Range;
use winnow::{
    ModalResult, Parser,
    ascii::{Caseless, escaped, multispace0},
    combinator::{alt, cut_err, delimited, fail, not, opt, preceded, separated, terminated},
    error::StrContext,
    token::{any, take_while},
};

pub const MAX_QUERY_BYTES: usize = 16384;
pub const MAX_QUERY_DEPTH: usize = 32;
pub const MAX_QUERY_NODES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Query {
    Predicate(Predicate),
    All(Vec<Query>),
    Any(Vec<Query>),
    Not(Box<Query>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Predicate {
    pub field: String,
    pub operator: Operator,
    pub value: Literal,
    pub span: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operator {
    Equal,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Contains,
    StartsWith,
    Fuzzy,
    Within,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Literal {
    Text(String),
    Number(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryError {
    pub kind: QueryErrorKind,
    pub span: Range<usize>,
    pub message: String,
    pub incomplete: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryErrorKind {
    Syntax,
    Field,
    Operator,
    Value,
    Limit,
}
impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} at bytes {}..{}",
            self.message, self.span.start, self.span.end
        )
    }
}
impl std::error::Error for QueryError {}

impl Query {
    pub fn parse(text: &str) -> Result<Self, QueryError> {
        if text.len() > MAX_QUERY_BYTES {
            return Err(QueryError {
                kind: QueryErrorKind::Limit,
                span: 0..text.len(),
                message: "query is too long".into(),
                incomplete: false,
            });
        }
        let mut nodes = 0;
        let mut parser = |input: &mut &str| expression(input, text.len(), 0, &mut nodes);
        parser.parse(text).map_err(|error| {
            let start = error.offset();
            let end = start + text[start..].chars().next().map_or(0, char::len_utf8);
            QueryError {
                kind: QueryErrorKind::Syntax,
                span: start..end,
                message: error.inner().to_string(),
                incomplete: start == text.len(),
            }
        })
    }
}

fn keyword<'a>(
    word: &'static str,
) -> impl Parser<&'a str, (), winnow::error::ErrMode<winnow::error::ContextError>> {
    terminated(
        Caseless(word),
        not(take_while(1.., |c: char| {
            c.is_ascii_alphanumeric() || c == '_'
        })),
    )
    .void()
}

fn expression(
    input: &mut &str,
    length: usize,
    depth: usize,
    nodes: &mut usize,
) -> ModalResult<Query> {
    let mut terms: Vec<_> = separated(
        1..,
        cut_err(|i: &mut &str| conjunction(i, length, depth, nodes)),
        delimited(multispace0, keyword("or"), multispace0),
    )
    .parse_next(input)?;
    Ok(if terms.len() == 1 {
        terms.pop().unwrap()
    } else {
        Query::Any(terms)
    })
}

fn conjunction(
    input: &mut &str,
    length: usize,
    depth: usize,
    nodes: &mut usize,
) -> ModalResult<Query> {
    let mut terms: Vec<_> = separated(
        1..,
        cut_err(|i: &mut &str| unary(i, length, depth, nodes)),
        delimited(multispace0, keyword("and"), multispace0),
    )
    .parse_next(input)?;
    Ok(if terms.len() == 1 {
        terms.pop().unwrap()
    } else {
        Query::All(terms)
    })
}

fn unary(input: &mut &str, length: usize, depth: usize, nodes: &mut usize) -> ModalResult<Query> {
    multispace0.parse_next(input)?;
    *nodes += 1;
    if depth >= MAX_QUERY_DEPTH || *nodes > MAX_QUERY_NODES {
        return cut_err(fail)
            .context(StrContext::Label("query complexity limit"))
            .parse_next(input);
    }
    if opt(keyword("not")).parse_next(input)?.is_some() {
        return cut_err(|i: &mut &str| unary(i, length, depth + 1, nodes))
            .map(|q| Query::Not(Box::new(q)))
            .parse_next(input);
    }
    if opt('(').parse_next(input)?.is_some() {
        return cut_err(terminated(
            |i: &mut &str| expression(i, length, depth + 1, nodes),
            delimited(multispace0, ')', multispace0),
        ))
        .parse_next(input);
    }
    let start = length - input.len();
    let field = take_while(1.., |c: char| c.is_ascii_alphabetic() || c == '_')
        .context(StrContext::Label("field"))
        .parse_next(input)?
        .to_ascii_lowercase();
    multispace0.parse_next(input)?;
    let operator = cut_err(alt((
        "<=".value(Operator::LessEqual),
        ">=".value(Operator::GreaterEqual),
        '='.value(Operator::Equal),
        '<'.value(Operator::Less),
        '>'.value(Operator::Greater),
        keyword("contains").value(Operator::Contains),
        keyword("starts_with").value(Operator::StartsWith),
        keyword("fuzzy").value(Operator::Fuzzy),
        keyword("within").value(Operator::Within),
    )))
    .context(StrContext::Label("operator"))
    .parse_next(input)?;
    multispace0.parse_next(input)?;
    let value = cut_err(alt((
        quoted.map(Literal::Text),
        take_while(1.., |c: char| {
            c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')
        })
        .map(|s: &str| Literal::Number(s.to_owned())),
    )))
    .context(StrContext::Label("literal"))
    .parse_next(input)?;
    let end = length - input.len();
    multispace0.parse_next(input)?;
    Ok(Query::Predicate(Predicate {
        field,
        operator,
        value,
        span: start..end,
    }))
}

fn quoted(input: &mut &str) -> ModalResult<String> {
    preceded(
        '"',
        cut_err(terminated(
            escaped(
                take_while(1.., |c: char| c != '"' && c != '\\' && !c.is_control()),
                '\\',
                cut_err(any.verify_map(|c| match c {
                    '"' | '\\' => Some(c),
                    'n' => Some('\n'),
                    'r' => Some('\r'),
                    't' => Some('\t'),
                    '0' => Some('\0'),
                    _ => None,
                })),
            )
            .map(|text: String| text.into_boxed_str().into_string()),
            '"',
        )),
    )
    .parse_next(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_and_escaping_are_explicit() {
        let query = Query::parse(r#"name = "😀\"\\" or value = 5 and not value = 6"#).unwrap();
        let Query::Any(terms) = query else {
            panic!("OR expected")
        };
        let Query::Predicate(p) = &terms[0] else {
            panic!("predicate expected")
        };
        assert_eq!(p.value, Literal::Text("😀\"\\".into()));
        assert_eq!(p.span, 0..17);
        let Query::All(terms) = &terms[1] else {
            panic!("AND expected")
        };
        assert!(matches!(&terms[1], Query::Not(_)));
        assert!(Query::parse("NAME = \"\" AND VALUE = NaN").is_ok());
    }

    #[test]
    fn quoted_literals_accumulate_unicode_and_each_supported_escape() {
        for (literal, expected) in [
            (r#""""#, ""),
            (r#""中😀\n\r\t\0\"\\end""#, "中😀\n\r\t\0\"\\end"),
        ] {
            let Query::Predicate(p) = Query::parse(&format!("name = {literal}")).unwrap() else {
                panic!("predicate expected");
            };
            assert_eq!(p.value, Literal::Text(expected.into()));
            assert_eq!(p.span, 0..7 + literal.len());
        }
        for literal in ["\"abc\\", "\"abc", r#""\q""#, "\"中\n\"", "\"中\0\""] {
            assert!(Query::parse(&format!("name = {literal}")).is_err());
        }
    }

    #[test]
    fn junction_lists_preserve_flat_groups_and_complexity_boundaries() {
        for (operator, any) in [("and", false), ("or", true)] {
            let query =
                Query::parse(&vec!["value = 1"; MAX_QUERY_NODES].join(&format!(" {operator} ")))
                    .unwrap();
            let terms = match query {
                Query::Any(terms) if any => terms,
                Query::All(terms) if !any => terms,
                _ => panic!("flat junction expected"),
            };
            assert_eq!(terms.len(), MAX_QUERY_NODES);
            assert!(terms.iter().all(|q| matches!(q, Query::Predicate(_))));
            assert!(
                Query::parse(
                    &vec!["value = 1"; MAX_QUERY_NODES + 1].join(&format!(" {operator} "))
                )
                .is_err()
            );
            let error = Query::parse(&format!("name = \"😀\" {operator}")).unwrap_err();
            assert!(error.incomplete);
            assert_eq!(error.span, 14 + operator.len()..14 + operator.len());
        }
        for n in [MAX_QUERY_DEPTH - 1, MAX_QUERY_DEPTH] {
            assert_eq!(
                Query::parse(&format!("{}value = 1{}", "(".repeat(n), ")".repeat(n))).is_ok(),
                n < MAX_QUERY_DEPTH
            );
            assert_eq!(
                Query::parse(&format!("{}value = 1", "not ".repeat(n))).is_ok(),
                n < MAX_QUERY_DEPTH
            );
        }
    }

    #[test]
    fn errors_preserve_offsets_and_reject_unbounded_or_implicit_syntax() {
        let error = Query::parse("value =").unwrap_err();
        assert!(error.incomplete);
        assert_eq!(error.span, 7..7);
        for text in [
            "",
            "name = \"a\" value = 5",
            "value = 5 or",
            "value = 5 order",
            r#"name = "\q""#,
        ] {
            assert!(Query::parse(text).is_err(), "{text}");
        }
        assert!(Query::parse(&format!("{}value = 5{}", "(".repeat(33), ")".repeat(33))).is_err());
        assert!(Query::parse(&vec!["value = 5"; 257].join(" or ")).is_err());
    }
}
