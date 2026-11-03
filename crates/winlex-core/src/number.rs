//! Shared integer grammar. Parsing deliberately does not trim whitespace.
use anyhow::{Context, Result, ensure};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntegerLiteral {
    pub negative: bool,
    pub magnitude: u64,
}

impl IntegerLiteral {
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(text.len() <= 4096, "number is too long");
        let (negative, body) = match text.as_bytes().first() {
            Some(b'-') => (true, &text[1..]),
            Some(b'+') => (false, &text[1..]),
            _ => (false, text),
        };
        ensure!(!body.is_empty(), "invalid number");
        let (radix, digits) = if body.starts_with('0') && body.len() > 1 {
            match body.as_bytes()[1] {
                b'x' | b'X' => (16, &body[2..]),
                b'o' | b'O' => (8, &body[2..]),
                b'b' | b'B' => (2, &body[2..]),
                _ => (8, &body[1..]),
            }
        } else {
            (10, body)
        };
        let magnitude = parse_unsigned(digits, radix).context("invalid integer")?;
        ensure!(
            !negative || magnitude <= 1u64 << 63,
            "negative integer out of range"
        );
        Ok(Self {
            negative,
            magnitude,
        })
    }

    /// Checked 32-bit code conversion, including signed two's-complement input.
    pub fn to_code_u32(self) -> Result<u32> {
        ensure!(
            self.magnitude
                <= if self.negative {
                    1 << 31
                } else {
                    u32::MAX as u64
                },
            "code out of 32-bit range"
        );
        Ok(if self.negative {
            0u32.wrapping_sub(self.magnitude as u32)
        } else {
            self.magnitude as u32
        })
    }

    pub fn to_u16(self) -> Result<u16> {
        ensure!(!self.negative, "unsigned integer cannot be negative");
        u16::try_from(self.magnitude).context("integer out of 16-bit range")
    }
}

pub(crate) fn parse_unsigned(digits: &str, radix: u32) -> Result<u64> {
    ensure!(
        !digits.is_empty() && digits.bytes().all(|b| (b as char).is_digit(radix)),
        "invalid unsigned integer"
    );
    Ok(u64::from_str_radix(digits, radix)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grammar_and_checked_conversions() {
        assert_eq!(IntegerLiteral::parse("010").unwrap().magnitude, 8);
        for s in ["0x80070005", "-2147024891"] {
            assert_eq!(
                IntegerLiteral::parse(s).unwrap().to_code_u32().unwrap(),
                0x80070005
            );
        }
        for s in ["4294967296", "-2147483649"] {
            assert!(IntegerLiteral::parse(s).unwrap().to_code_u32().is_err());
        }
        for s in ["-1", "-0", "65536"] {
            assert!(IntegerLiteral::parse(s).unwrap().to_u16().is_err());
        }
        for s in [
            "08",
            " 1",
            "1 ",
            "1.0",
            "NaN",
            "Infinity",
            "ERROR_SUCCESS",
            "0x+1",
        ] {
            assert!(IntegerLiteral::parse(s).is_err(), "{s}");
        }
    }
}
