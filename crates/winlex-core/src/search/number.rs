use crate::Architecture;
use anyhow::{Context, Result, bail, ensure};
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
    Integer {
        negative: bool,
        magnitude: u64,
    },
    Float {
        narrow: Option<f32>,
        wide: Option<f64>,
    },
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
                wide: Some(value),
            });
        }
        let (negative, body) = match text.as_bytes().first() {
            Some(b'-') => (true, &text[1..]),
            Some(b'+') => (false, &text[1..]),
            _ => (false, text),
        };
        ensure!(!body.is_empty(), "invalid number");
        let prefixed = super::hex_digits(body).is_some()
            || ["0o", "0O", "0b", "0B"].iter().any(|p| body.starts_with(p));
        if !prefixed && body.contains(['.', 'e', 'E']) {
            return Self::decimal(text, body);
        }
        let (radix, digits) = if body.starts_with(['0']) && body.len() > 1 {
            match body.as_bytes()[1] {
                b'x' | b'X' => (16, &body[2..]),
                b'o' | b'O' => (8, &body[2..]),
                b'b' | b'B' => (2, &body[2..]),
                _ => (8, &body[1..]),
            }
        } else {
            (10, body)
        };
        let magnitude = super::parse_unsigned(digits, radix).context("invalid integer")?;
        ensure!(
            !negative || magnitude <= 1u64 << 63,
            "negative integer out of range"
        );
        Ok(Self::Integer {
            negative,
            magnitude,
        })
    }
    fn decimal(text: &str, body: &str) -> Result<Self> {
        let mut parts = body.split(['e', 'E']);
        let mantissa = parts.next().unwrap();
        if let Some(exponent) = parts.next() {
            let digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
            ensure!(
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
                "invalid decimal exponent"
            );
        }
        ensure!(parts.next().is_none(), "invalid decimal exponent");
        let mut digits = 0;
        let mut point = false;
        let mut nonzero = false;
        for byte in mantissa.bytes() {
            match byte {
                b'.' if !point => point = true,
                b'0'..=b'9' => {
                    digits += 1;
                    nonzero |= byte != b'0';
                }
                _ => bail!("invalid decimal number"),
            }
        }
        ensure!(digits > 0, "invalid decimal number");
        let narrow = text
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite() && (!nonzero || *v != 0.0));
        let wide = text
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && (!nonzero || *v != 0.0));
        ensure!(narrow.is_some() || wide.is_some(), "float out of range");
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
                Self::Integer { .. } => super::MatchQuality::Integer,
                Self::Float { .. } => super::MatchQuality::Float,
            })
    }
    pub fn is_nan(&self) -> bool {
        matches!(self, Self::Float { wide: Some(value), .. } if value.is_nan())
    }
    pub fn validate_comparison(&self, op: Comparison) -> Result<()> {
        ensure!(
            op == Comparison::Equal || !self.is_nan(),
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
        if let Self::Float { narrow, wide } = self {
            let pair = match value {
                Value::F32(value) => narrow.map(|bound| (*value as f64, bound as f64)),
                Value::F64(value) => wide.map(|bound| (*value, bound)),
                _ => None,
            };
            return pair.is_some_and(|(value, bound)| {
                if bound.is_nan() {
                    op == Comparison::Equal && value.is_nan()
                } else {
                    op.accepts(value.partial_cmp(&bound))
                }
            });
        }
        if matches!(value, Value::USize(_) | Value::ISize(_)) {
            return Architecture::ALL
                .into_iter()
                .filter(|&a| architecture.map_or(a.supports(mask), |selected| a == selected))
                .any(|a| {
                    let value = match (value, a) {
                        (Value::USize(v), Architecture::X86) => {
                            u32::try_from(*v).ok().map(Value::U32)
                        }
                        (Value::ISize(v), Architecture::X86) => {
                            i32::try_from(*v).ok().map(Value::I32)
                        }
                        (Value::USize(v), _) => Some(Value::U64(*v)),
                        (Value::ISize(v), _) => Some(Value::I64(*v)),
                        _ => unreachable!(),
                    };
                    value.is_some_and(|value| self.compare(&value, Some(a), mask, op))
                });
        }
        let Some(integer) = crate::integer(value) else {
            return false;
        };
        let Self::Integer {
            negative,
            magnitude,
        } = self
        else {
            unreachable!()
        };
        if op == Comparison::Equal {
            let bits = match value {
                Value::U8(_) | Value::I8(_) => 8,
                Value::U16(_) | Value::I16(_) => 16,
                Value::U32(_) | Value::I32(_) => 32,
                _ => 64,
            };
            let max = u64::MAX >> (64 - bits);
            let expected = if *negative {
                if *magnitude > 1u64 << (bits - 1) {
                    return false;
                }
                0u64.wrapping_sub(*magnitude) & max
            } else {
                if *magnitude > max {
                    return false;
                }
                *magnitude
            };
            return integer as u64 & max == expected;
        }
        let bound = if *negative {
            -(*magnitude as i128)
        } else {
            *magnitude as i128
        };
        op.accepts(Some(integer.cmp(&bound)))
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
    fn decimal_exponent_validation_preserves_diagnostics_and_zero_extremes() {
        for text in [
            "1e", "1e+", "1e-", "1e2e3", "1e+2E-3", "1e１２", "1e 2", "1..0e+",
        ] {
            assert_eq!(
                NumberQuery::compile(text).err().unwrap().to_string(),
                "invalid decimal exponent",
                "{text}"
            );
        }
        for text in ["1..0e2", ".e2"] {
            assert_eq!(
                NumberQuery::compile(text).err().unwrap().to_string(),
                "invalid decimal number",
                "{text}"
            );
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
        for query in [".", "1.2.3", "1e", "1e2e3", "Infinity", "1e9999", "1e-9999"] {
            assert!(NumberQuery::compile(query).is_err(), "{query}");
        }
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
                assert!(query.compare(&value, Some(Architecture::X86), 7, Comparison::Equal));
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
        assert!(
            q.matches(
                &Value::ISize(-1),
                Some(Architecture::X86),
                7,
                Comparison::Equal
            )
            .is_some()
        );
        assert!(
            q.matches(
                &Value::ISize(-1),
                Some(Architecture::X64),
                7,
                Comparison::Equal
            )
            .is_none()
        );
        assert!(
            q.matches(
                &Value::ISize(-1),
                None,
                Architecture::X64.bit(),
                Comparison::Equal
            )
            .is_none()
        );
    }
}
