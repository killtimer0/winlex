use crate::Architecture;
use anyhow::{Context, Result, bail, ensure};
use num_bigint::BigInt;
use num_traits::{Signed, Zero};
use std::cmp::Ordering;
use windows_metadata::Value;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct FloatDistance(Decimal);

#[derive(Clone, Debug)]
pub(super) struct Decimal {
    coefficient: BigInt,
    exponent: i128,
    digits: usize,
}
impl Decimal {
    fn new(coefficient: BigInt, exponent: i128) -> Self {
        let digits = coefficient.to_str_radix(10).trim_start_matches('-').len();
        Self {
            coefficient,
            exponent,
            digits,
        }
    }
    fn float(value: f64) -> Option<Self> {
        if !value.is_finite() {
            return None;
        }
        let bits = value.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as i32;
        let fraction = bits & ((1u64 << 52) - 1);
        let (mantissa, power) = if exponent == 0 {
            (fraction, -1074)
        } else {
            (fraction | (1u64 << 52), exponent - 1023 - 52)
        };
        if mantissa == 0 {
            return Some(Self::new(BigInt::ZERO, 0));
        }
        let mut coefficient = BigInt::from(mantissa);
        let decimal_power = if power >= 0 {
            coefficient <<= power as usize;
            0
        } else {
            coefficient *= BigInt::from(5u8).pow((-power) as u32);
            power as i128
        };
        if bits >> 63 != 0 {
            coefficient = -coefficient;
        }
        Some(Self::new(coefficient, decimal_power))
    }
    fn magnitude_cmp(&self, other: &Self) -> Ordering {
        let order =
            (self.exponent + self.digits as i128).cmp(&(other.exponent + other.digits as i128));
        if order != Ordering::Equal {
            return order;
        }
        let (left, right) = self.aligned(other);
        left.abs().cmp(&right.abs())
    }
    fn aligned(&self, other: &Self) -> (BigInt, BigInt) {
        let exponent = self.exponent.min(other.exponent);
        let scale = |value: &Self| {
            let power =
                u32::try_from(value.exponent - exponent).expect("bounded decimal alignment");
            &value.coefficient * BigInt::from(10u8).pow(power)
        };
        (scale(self), scale(other))
    }
    fn distance(&self, other: &Self) -> FloatDistance {
        if self.coefficient.is_zero() {
            return FloatDistance(Self::new(other.coefficient.abs(), other.exponent));
        }
        if other.coefficient.is_zero() {
            return FloatDistance(Self::new(self.coefficient.abs(), self.exponent));
        }
        let (left, right) = self.aligned(other);
        FloatDistance(Self::new(
            (left - right).abs(),
            self.exponent.min(other.exponent),
        ))
    }
}
impl PartialEq for Decimal {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Decimal {}
impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        let sign = self.coefficient.sign().cmp(&other.coefficient.sign());
        if sign != Ordering::Equal {
            return sign;
        }
        if self.coefficient.is_zero() {
            return Ordering::Equal;
        }
        let order = self.magnitude_cmp(other);
        if self.coefficient.is_negative() {
            order.reverse()
        } else {
            order
        }
    }
}
impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub(super) enum NumberQuery {
    Integer {
        negative: bool,
        magnitude: u64,
    },
    Float {
        center: Decimal,
        lower: Decimal,
        upper: Decimal,
    },
}
impl NumberQuery {
    pub fn compile(text: &str) -> Result<Self> {
        let (negative, body) = match text.as_bytes().first() {
            Some(b'-') => (true, &text[1..]),
            Some(b'+') => (false, &text[1..]),
            _ => (false, text),
        };
        ensure!(!body.is_empty(), "invalid number");
        let prefixed = super::hex_digits(body).is_some()
            || ["0o", "0O", "0b", "0B"].iter().any(|p| body.starts_with(p));
        if !prefixed && body.contains(['.', 'e', 'E']) {
            return Self::decimal(body, negative);
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
    fn decimal(body: &str, negative: bool) -> Result<Self> {
        let mut parts = body.split(['e', 'E']);
        let mantissa = parts.next().unwrap();
        let exponent = parts
            .next()
            .map(|s| {
                let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
                ensure!(
                    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
                    "invalid decimal exponent"
                );
                Ok(s.parse::<i64>()? as i128)
            })
            .transpose()?
            .unwrap_or(0);
        ensure!(parts.next().is_none(), "invalid decimal exponent");
        let mut digits = String::new();
        let mut point = None;
        for byte in mantissa.bytes() {
            match byte {
                b'.' if point.is_none() => point = Some(digits.len()),
                b'0'..=b'9' => digits.push(byte as char),
                _ => bail!("invalid decimal number"),
            }
        }
        ensure!(!digits.is_empty(), "invalid decimal number");
        let exponent = exponent - point.map_or(0, |p| digits.len() - p) as i128;
        let mut coefficient = BigInt::parse_bytes(digits.as_bytes(), 10).unwrap();
        if negative {
            coefficient = -coefficient;
        }
        let lower = Decimal::new((&coefficient * 2 - 1) * 5, exponent - 1);
        let upper = Decimal::new((&coefficient * 2 + 1) * 5, exponent - 1);
        Ok(Self::Float {
            center: Decimal::new(coefficient, exponent),
            lower,
            upper,
        })
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
            F32(value) => return self.float(*value as f64),
            F64(value) => return self.float(*value),
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
    fn float(&self, value: f64) -> Option<super::MatchQuality> {
        let Self::Float {
            center,
            lower,
            upper,
        } = self
        else {
            return None;
        };
        let value = Decimal::float(value)?;
        (value >= *lower && value < *upper)
            .then(|| super::MatchQuality::Float(center.distance(&value)))
    }
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
    fn decimal_intervals_preserve_precision_and_exact_binary_boundaries() {
        assert!(hit("1.5", Value::F64(1.5)));
        assert!(hit("1.5", Value::F32(1.46)));
        assert!(!hit("1.50", Value::F32(1.46)));
        assert!(hit("1.5e3", Value::F64(1450.0)));
        assert!(!hit("1.5e3", Value::F64(1550.0)));
        assert!(hit("-1.5e3", Value::F64(-1550.0)));
        assert!(!hit("-1.5e3", Value::F64(-1450.0)));
        assert!(hit("0.3", Value::F64(0.1 + 0.2)));
        assert!(!hit("1.5", Value::F64(f64::NAN)));
        assert!(!hit("1.5", Value::F64(f64::INFINITY)));
        assert!(!hit("1.0", Value::I32(1)));
        assert!(!hit("1", Value::F64(1.0)));
        assert!(hit("0e9223372036854775807", Value::F64(f64::MAX)));
        assert!(hit("0e-9223372036854775808", Value::F64(-0.0)));
        assert!(!hit("1e-9223372036854775808", Value::F64(0.0)));
        assert!(hit("5e-324", Value::F64(f64::from_bits(1))));
        let q = NumberQuery::compile("1.5").unwrap();
        assert!(
            q.matches(&Value::F64(1.5), None, 7)
                .unwrap()
                .compare(&q.matches(&Value::F64(1.51), None, 7).unwrap())
                .is_lt()
        );
        for query in [".", "1.2.3", "1e", "1e2e3", "NaN", "Infinity"] {
            assert!(NumberQuery::compile(query).is_err(), "{query}");
        }
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
