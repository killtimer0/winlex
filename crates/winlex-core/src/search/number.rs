use crate::Architecture;
use anyhow::{Context, Result, ensure};
use std::cmp::Ordering;
use windows_metadata::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    Equal,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}
impl Comparison {
    fn accepts(self, order: Option<Ordering>) -> bool {
        matches!(
            (self, order),
            (Self::Equal, Some(Ordering::Equal))
                | (Self::Less | Self::LessEqual, Some(Ordering::Less))
                | (Self::Greater | Self::GreaterEqual, Some(Ordering::Greater))
                | (Self::LessEqual | Self::GreaterEqual, Some(Ordering::Equal))
        )
    }
}

pub(super) enum NumberQuery {
    Integer(crate::number::IntegerLiteral),
    Float { narrow: Option<f32>, wide: f64 },
}
impl NumberQuery {
    pub fn compile(text: &str) -> Result<Self> {
        ensure!(text.len() <= 4096, "number is too long");
        let special = match text.to_ascii_lowercase().as_str() {
            "nan" => Some(f64::NAN),
            "inf" | "+inf" => Some(f64::INFINITY),
            "-inf" => Some(f64::NEG_INFINITY),
            _ => None,
        };
        if let Some(value) = special {
            return Ok(Self::Float {
                narrow: Some(value as f32),
                wide: value,
            });
        }
        let body = match text.as_bytes().first() {
            Some(b'-' | b'+') => &text[1..],
            _ => text,
        };
        ensure!(!body.is_empty(), "invalid number");
        let prefixed = super::hex_digits(body).is_some()
            || ["0o", "0O", "0b", "0B"].iter().any(|p| body.starts_with(p));
        if !prefixed && body.contains(['.', 'e', 'E']) {
            return Self::decimal(text, body);
        }
        Ok(Self::Integer(crate::number::IntegerLiteral::parse(text)?))
    }
    fn decimal(text: &str, body: &str) -> Result<Self> {
        let wide = text.parse::<f64>().context("invalid decimal number")?;
        let nonzero = body
            .bytes()
            .take_while(|b| !matches!(b, b'e' | b'E'))
            .any(|b| matches!(b, b'1'..=b'9'));
        let narrow = text
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite() && (!nonzero || *v != 0.0));
        ensure!(
            wide.is_finite() && (!nonzero || wide != 0.0),
            "float out of range"
        );
        Ok(Self::Float { narrow, wide })
    }
    pub fn matches(
        &self,
        value: &Value,
        architecture: Option<Architecture>,
        mask: i32,
        op: Comparison,
    ) -> Option<super::MatchQuality> {
        self.compare(value, architecture, mask, op)
            .then_some(match self {
                Self::Integer(_) => super::MatchQuality::Integer,
                Self::Float { .. } => super::MatchQuality::Float,
            })
    }
    pub fn validate_comparison(&self, op: Comparison) -> Result<()> {
        let nan = matches!(self, Self::Float { wide, .. } if wide.is_nan());
        ensure!(
            op == Comparison::Equal || !nan,
            "NaN cannot be an ordered bound"
        );
        Ok(())
    }
    pub fn compare(
        &self,
        value: &Value,
        architecture: Option<Architecture>,
        mask: i32,
        op: Comparison,
    ) -> bool {
        if let Value::EnumValue(_, value) = value {
            return self.compare(value, architecture, mask, op);
        }
        match self {
            Self::Float { narrow, wide } => {
                let pair = match value {
                    Value::F32(value) => narrow.map(|bound| (*value as f64, bound as f64)),
                    Value::F64(value) => Some((*value, *wide)),
                    _ => None,
                };
                pair.is_some_and(|(value, bound)| {
                    if bound.is_nan() {
                        op == Comparison::Equal && value.is_nan()
                    } else {
                        op.accepts(value.partial_cmp(&bound))
                    }
                })
            }
            Self::Integer(crate::number::IntegerLiteral {
                negative,
                magnitude,
            }) => {
                let Some(integer) = crate::integer(value) else {
                    return false;
                };
                let compare = |bits| {
                    if op == Comparison::Equal {
                        let max = u64::MAX >> (64 - bits);
                        let limit = if *negative { 1u64 << (bits - 1) } else { max };
                        let expected = if *negative {
                            0u64.wrapping_sub(*magnitude)
                        } else {
                            *magnitude
                        };
                        *magnitude <= limit && integer as u64 & max == expected & max
                    } else {
                        let bound = if *negative {
                            -(*magnitude as i128)
                        } else {
                            *magnitude as i128
                        };
                        op.accepts(Some(integer.cmp(&bound)))
                    }
                };
                match value {
                    Value::USize(_) | Value::ISize(_) => Architecture::ALL.into_iter().any(|a| {
                        architecture.map_or(a.supports(mask), |selected| a == selected)
                            && match (value, a) {
                                (Value::USize(v), Architecture::X86) => u32::try_from(*v).is_ok(),
                                (Value::ISize(v), Architecture::X86) => i32::try_from(*v).is_ok(),
                                _ => true,
                            }
                            && compare(if a == Architecture::X86 { 32 } else { 64 })
                    }),
                    Value::U8(_) | Value::I8(_) => compare(8),
                    Value::U16(_) | Value::I16(_) => compare(16),
                    Value::U32(_) | Value::I32(_) => compare(32),
                    _ => compare(64),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hit(query: &str, value: Value) -> bool {
        NumberQuery::compile(query)
            .unwrap()
            .matches(&value, None, Architecture::ALL_MASK, Comparison::Equal)
            .is_some()
    }
    #[test]
    fn integers_use_width_local_twos_complement_without_truncation() {
        for query in ["0b110", "+0b110", "06", "0o6", "6", "0x6", "0X6"] {
            assert!(hit(query, Value::U8(6)));
        }
        for query in ["0777", "0o777", "511", "0x1ff"] {
            assert!(hit(query, Value::I16(511)));
        }
        assert!(!hit("257", Value::U8(1)));
        assert!(hit("010", Value::U8(8)));
        for value in [
            Value::I16(257),
            Value::U16(257),
            Value::I32(257),
            Value::U32(257),
            Value::I64(257),
            Value::U64(257),
        ] {
            assert!(hit("257", value));
        }
        for value in [
            Value::U32(0x80000001),
            Value::I32(-0x7fffffff),
            Value::U64(0x80000001),
            Value::I64(0x80000001),
        ] {
            assert!(hit("0x80000001", value));
        }
        assert!(!hit("0x80000001", Value::I64(-0x7fffffff)));
        assert!(!hit("0x80000001", Value::I64(i64::MAX)));
        for value in [
            Value::I8(-1),
            Value::U8(255),
            Value::I64(-1),
            Value::U64(u64::MAX),
        ] {
            assert!(hit("-1", value));
        }
        assert!(!hit("-129", Value::U8(127)));
        assert!(hit("-129", Value::U16(65407)));
        assert!(!hit("1", Value::Bool(true)));
        for query in [
            "08",
            "0x",
            "0b2",
            "--1",
            "0x+1",
            "１２",
            "0xé",
            "18446744073709551616",
            "-9223372036854775809",
        ] {
            assert!(NumberQuery::compile(query).is_err(), "{query}");
        }
    }
    #[test]
    fn decimal_grammar_and_zero_extremes() {
        for text in [
            ".5", "1.", "+.5", "-1.", "1.e2", ".5E-2", "+01.0e+2", "01e2",
        ] {
            assert!(NumberQuery::compile(text).is_ok(), "{text}");
        }
        for text in [
            ".", ".e2", "1.2.3", "1..0e2", "1e", "1e+", "1e-", "1e2e3", "1e+2E-3", "1e１２",
            "1e 2", "1..0e+", "1_0.0", "1.0_0", "1e1_0", "１２.0", " 1.0", "1.0 ", "--1.0",
            "+-1.0", "0x1.0", "0x1p2", "NaN.0", "inf.0", "Infinity",
        ] {
            assert!(NumberQuery::compile(text).is_err(), "{text}");
        }
        for text in ["1e9999", "1e-9999"] {
            assert_eq!(
                NumberQuery::compile(text).err().unwrap().to_string(),
                "float out of range",
                "{text}"
            );
        }
        for text in ["0e9999", "-0E-9999", "+0.0e+9999"] {
            assert!(hit(text, Value::F64(0.0)), "{text}");
        }
    }

    #[test]
    fn floats_compare_in_the_declared_type_without_implicit_intervals() {
        assert!(hit("1.5", Value::F64(1.5)));
        assert!(hit("1.50", Value::F64(1.5)));
        assert!(!hit("1.5", Value::F32(1.46)));
        assert!(!hit("1.50", Value::F32(1.46)));
        assert!(hit("1.5e3", Value::F64(1500.0)));
        assert!(!hit("1.5e3", Value::F64(1450.0)));
        assert!(!hit("1.5e3", Value::F64(1550.0)));
        assert!(!hit("-1.5e3", Value::F64(-1550.0)));
        assert!(!hit("-1.5e3", Value::F64(-1450.0)));
        assert!(!hit("0.3", Value::F64(0.1 + 0.2)));
        assert!(hit("0.1", Value::F32(0.1)));
        assert!(hit("0.1", Value::F64(0.1)));
        let rounding = "1.000000059604644775390625000000000001";
        assert!(hit(
            rounding,
            Value::F32(f32::from_bits(1.0f32.to_bits() + 1))
        ));
        assert!(!hit(rounding, Value::F32(1.0)));
        assert!(!hit("1.5", Value::F64(f64::NAN)));
        assert!(!hit("1.5", Value::F64(f64::INFINITY)));
        assert!(!hit("1.0", Value::I32(1)));
        assert!(!hit("1", Value::F64(1.0)));
        assert!(!hit("0e9223372036854775807", Value::F64(f64::MAX)));
        assert!(hit("0e-9223372036854775808", Value::F64(-0.0)));
        assert!(hit("5e-324", Value::F64(f64::from_bits(1))));
        assert!(!hit("5e-324", Value::F32(0.0)));
        assert!(!hit("1e100", Value::F32(f32::INFINITY)));
    }
    #[test]
    fn special_float_literals_classify_nan_and_preserve_infinity_sign() {
        for query in ["nan", "NaN", "NAN"] {
            for value in [f64::NAN, f64::from_bits(0xfff0000000000001)] {
                assert!(hit(query, Value::F64(value)));
            }
            assert!(hit(query, Value::F32(f32::NAN)));
            assert!(!hit(query, Value::F64(0.0)));
        }
        for query in ["inf", "+inf", "INF"] {
            assert!(hit(query, Value::F64(f64::INFINITY)));
            assert!(!hit(query, Value::F64(f64::NEG_INFINITY)));
        }
        assert!(hit("-inf", Value::F32(f32::NEG_INFINITY)));
        assert!(hit("-0.0", Value::F32(0.0)));
    }
    #[test]
    fn explicit_ranges_use_ordered_values_and_open_or_closed_bounds() {
        let compare = |text, value, op| {
            NumberQuery::compile(text)
                .unwrap()
                .compare(&value, None, 7, op)
        };
        assert!(compare("0", Value::I32(-1), Comparison::Less));
        assert!(!compare("0", Value::I32(-1), Comparison::Greater));
        assert!(compare("-1", Value::U64(u64::MAX), Comparison::Greater));
        assert!(compare(
            "18446744073709551615",
            Value::U64(u64::MAX),
            Comparison::LessEqual
        ));
        assert!(!compare(
            "18446744073709551615",
            Value::U64(u64::MAX),
            Comparison::Less
        ));
        assert!(compare("1.45", Value::F64(1.45), Comparison::GreaterEqual));
        assert!(!compare("1.55", Value::F64(1.55), Comparison::Less));
        assert!(compare("inf", Value::F64(f64::MAX), Comparison::Less));
        assert!(!compare("inf", Value::F64(f64::INFINITY), Comparison::Less));
        assert!(compare(
            "inf",
            Value::F64(f64::INFINITY),
            Comparison::LessEqual
        ));
        assert!(!compare("inf", Value::F64(f64::NAN), Comparison::LessEqual));
        assert!(
            NumberQuery::compile("nan")
                .unwrap()
                .validate_comparison(Comparison::Less)
                .is_err()
        );
    }
    #[test]
    fn equality_uses_storage_bits_but_ordering_uses_signed_values() {
        use windows_metadata::TypeName;
        for (value, text, quality) in [
            (Value::I8(-1), "255", super::super::MatchQuality::Integer),
            (
                Value::ISize(-1),
                "0xffffffff",
                super::super::MatchQuality::Integer,
            ),
            (Value::F32(1.5), "1.5", super::super::MatchQuality::Float),
        ] {
            let query = NumberQuery::compile(text).unwrap();
            for value in [
                value.clone(),
                Value::EnumValue(TypeName::named("N", "E"), Box::new(value)),
            ] {
                assert_eq!(
                    query.matches(&value, Some(Architecture::X86), 7, Comparison::Equal),
                    Some(quality.clone())
                );
                if quality == super::super::MatchQuality::Integer {
                    assert!(query.compare(&value, Some(Architecture::X86), 7, Comparison::Less));
                }
            }
        }
    }

    #[test]
    fn native_values_must_fit_at_least_one_applicable_architecture_for_every_operator() {
        let query = NumberQuery::compile("0").unwrap();
        for value in [
            Value::ISize(i32::MAX as i64 + 1),
            Value::USize(u32::MAX as u64 + 1),
        ] {
            for mask in 0..8 {
                for op in [
                    Comparison::Equal,
                    Comparison::Less,
                    Comparison::LessEqual,
                    Comparison::Greater,
                    Comparison::GreaterEqual,
                ] {
                    let ordered_hit = matches!(op, Comparison::Greater | Comparison::GreaterEqual);
                    assert_eq!(
                        query.compare(&value, None, mask, op),
                        mask & 6 != 0 && ordered_hit
                    );
                    assert!(!query.compare(&value, Some(Architecture::X86), mask, op));
                    for arch in [Architecture::X64, Architecture::Arm64] {
                        assert_eq!(query.compare(&value, Some(arch), mask, op), ordered_hit);
                    }
                }
            }
        }
    }

    #[test]
    fn native_integer_widths_follow_target_architecture() {
        let q = NumberQuery::compile("0xffffffff").unwrap();
        for (architecture, mask) in [
            (Some(Architecture::X64), 7),
            (None, Architecture::X64.bit()),
        ] {
            assert!(
                q.matches(&Value::ISize(-1), architecture, mask, Comparison::Equal)
                    .is_none()
            );
        }
    }
}
