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
        parts
            .next()
            .map(|s| {
                let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
                ensure!(
                    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
                    "invalid decimal exponent"
                );
                Ok::<_, anyhow::Error>(())
            })
            .transpose()?;
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
    ) -> Option<super::MatchQuality> {
        use Value::*;
        let (bits, unsigned) = match value {
            EnumValue(_, value) => return self.matches(value, architecture, mask),
            F32(_) | F64(_) => {
                return self
                    .compare(value, architecture, mask, Comparison::Equal)
                    .then_some(super::MatchQuality::Float);
            }
            U8(v) => (8, *v as u64),
            I8(v) => (8, *v as u8 as u64),
            U16(v) => (16, *v as u64),
            I16(v) => (16, *v as u16 as u64),
            U32(v) => (32, *v as u64),
            I32(v) => (32, *v as u32 as u64),
            U64(v) => (64, *v),
            I64(v) => (64, *v as u64),
            USize(_) | ISize(_) => {
                return [Architecture::X86, Architecture::X64, Architecture::Arm64]
                    .into_iter()
                    .filter(|&a| architecture.map_or(a.supports(mask), |selected| a == selected))
                    .find_map(|a| {
                        let value = match (value, a) {
                            (USize(v), Architecture::X86) => U32(u32::try_from(*v).ok()?),
                            (ISize(v), Architecture::X86) => I32(i32::try_from(*v).ok()?),
                            (USize(v), _) => U64(*v),
                            (ISize(v), _) => I64(*v),
                            _ => unreachable!(),
                        };
                        self.matches(&value, Some(a), mask)
                    });
            }
            _ => return None,
        };
        let Self::Integer {
            negative,
            magnitude,
        } = self
        else {
            return None;
        };
        let max = u64::MAX >> (64 - bits);
        let expected = if *negative {
            if *magnitude > 1u64 << (bits - 1) {
                return None;
            }
            0u64.wrapping_sub(*magnitude) & max
        } else {
            if *magnitude > max {
                return None;
            }
            *magnitude
        };
        (unsigned == expected).then_some(super::MatchQuality::Integer)
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
        if matches!(value, Value::F32(_) | Value::F64(_)) {
            return false;
        }
        if op == Comparison::Equal {
            return self.matches(value, architecture, mask).is_some();
        }
        let Self::Integer {
            negative,
            magnitude,
        } = self
        else {
            unreachable!()
        };
        let bound = if *negative {
            -(*magnitude as i128)
        } else {
            *magnitude as i128
        };
        let integer = match value {
            Value::I8(v) => *v as i128,
            Value::U8(v) => *v as i128,
            Value::I16(v) => *v as i128,
            Value::U16(v) => *v as i128,
            Value::I32(v) => *v as i128,
            Value::U32(v) => *v as i128,
            Value::I64(v) => *v as i128,
            Value::U64(v) => *v as i128,
            Value::ISize(v) => {
                if !native_fits(*v as i128, true, architecture, mask) {
                    return false;
                }
                *v as i128
            }
            Value::USize(v) => {
                if !native_fits(*v as i128, false, architecture, mask) {
                    return false;
                }
                *v as i128
            }
            _ => return false,
        };
        op.accepts(Some(integer.cmp(&bound)))
    }
}

fn native_fits(value: i128, signed: bool, architecture: Option<Architecture>, mask: i32) -> bool {
    [Architecture::X86, Architecture::X64, Architecture::Arm64]
        .into_iter()
        .filter(|&a| architecture.map_or(a.supports(mask), |selected| a == selected))
        .any(|a| {
            a != Architecture::X86
                || if signed {
                    i32::try_from(value).is_ok()
                } else {
                    u32::try_from(value).is_ok()
                }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hit(query: &str, value: Value) -> bool {
        NumberQuery::compile(query)
            .unwrap()
            .matches(&value, None, Architecture::ALL_MASK)
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
    fn native_integer_widths_follow_target_architecture() {
        let q = NumberQuery::compile("0xffffffff").unwrap();
        assert!(
            q.matches(&Value::ISize(-1), Some(Architecture::X86), 7)
                .is_some()
        );
        assert!(
            q.matches(&Value::ISize(-1), Some(Architecture::X64), 7)
                .is_none()
        );
        assert!(
            q.matches(&Value::ISize(-1), None, Architecture::X64.bit())
                .is_none()
        );
    }
}
