use anyhow::{Context, Result, ensure};

pub(super) struct GuidQuery {
    fragment: String,
}
impl GuidQuery {
    pub fn digit_count(&self) -> usize {
        self.fragment.len()
    }
    pub fn compile(text: &str) -> Result<Self> {
        let fragment = if text.contains(|c: char| c == ',' || c.is_ascii_whitespace())
            || super::hex_digits(text).is_some()
        {
            Self::bytes(text)?
        } else {
            let text = if text.starts_with('{') || text.ends_with('}') {
                let inner = text.strip_prefix('{').and_then(|s| s.strip_suffix('}'));
                ensure!(inner.is_some(), "unpaired GUID braces");
                inner.unwrap()
            } else {
                text
            };
            let mut fragment = String::new();
            let mut separators = Vec::new();
            for byte in text.bytes() {
                if byte == b'-' {
                    ensure!(
                        !fragment.is_empty() && separators.last() != Some(&fragment.len()),
                        "invalid GUID separators"
                    );
                    separators.push(fragment.len());
                } else {
                    ensure!(byte.is_ascii_hexdigit(), "invalid GUID fragment");
                    fragment.push((byte as char).to_ascii_lowercase());
                }
            }
            ensure!(!text.ends_with('-'), "invalid GUID separators");
            if !separators.is_empty() {
                ensure!(
                    fragment.len() <= 32
                        && (0..=32 - fragment.len()).any(|start| {
                            [8, 12, 16, 20]
                                .into_iter()
                                .filter(|&p| p > start && p < start + fragment.len())
                                .map(|p| p - start)
                                .eq(separators.iter().copied())
                        }),
                    "invalid GUID grouping"
                );
            }
            fragment
        };
        ensure!(
            !fragment.is_empty() && fragment.len() <= 32,
            "GUID fragment must contain 1 to 32 hexadecimal digits"
        );
        Ok(Self { fragment })
    }
    fn bytes(text: &str) -> Result<String> {
        let mut tokens = Vec::new();
        for group in text.split(',') {
            ensure!(!group.trim().is_empty(), "empty GUID byte");
            tokens.extend(group.split_ascii_whitespace());
        }
        let hexadecimal = tokens.iter().any(|token| {
            super::hex_digits(token).is_none()
                && token
                    .bytes()
                    .any(|b| matches!(b, b'a'..=b'f' | b'A'..=b'F'))
        });
        let mut fragment = String::new();
        for token in tokens {
            let (digits, radix) = if let Some(digits) = super::hex_digits(token) {
                (digits, 16)
            } else if hexadecimal {
                ensure!(
                    token.len() == 2,
                    "hexadecimal GUID bytes require two digits"
                );
                (token, 16)
            } else {
                (token, 10)
            };
            let byte = u8::try_from(
                crate::number::parse_unsigned(digits, radix).context("invalid GUID byte")?,
            )
            .context("GUID byte out of range")?;
            use std::fmt::Write;
            write!(fragment, "{byte:02x}").unwrap();
            ensure!(fragment.len() <= 32, "GUID byte sequence is too long");
        }
        Ok(fragment)
    }
    pub fn matches(&self, guid: &str) -> Option<super::MatchQuality> {
        let text: String = guid
            .chars()
            .filter(|&c| c != '-')
            .map(|c| c.to_ascii_lowercase())
            .collect();
        text.contains(&self.fragment)
            .then_some(if self.fragment.len() == 32 {
                super::MatchQuality::GuidExact
            } else {
                super::MatchQuality::GuidFragment
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::MatchQuality;
    #[test]
    fn canonical_order_and_contiguous_fragments() {
        let guid = "12345678-9abc-def0-1234-56789abcdef0";
        for text in [
            guid,
            "{12345678-9ABC-def0-1234-56789abcdef0}",
            "123456789abcdef0123456789abcdef0",
            "1234",
            "F0-1234",
            "0-1",
        ] {
            assert!(
                GuidQuery::compile(text).unwrap().matches(guid).is_some(),
                "{text}"
            );
        }
        assert!(
            GuidQuery::compile("78563412")
                .unwrap()
                .matches(guid)
                .is_none()
        );
        for text in ["", "{}", "{1234", "12--34", "12-", "123456789-abc", "gg"] {
            assert!(GuidQuery::compile(text).is_err(), "{text}");
        }
    }
    #[test]
    fn byte_lists_have_explicit_radix_rules() {
        for (text, expected, other) in [
            ("239,254", "effe", "feef"),
            ("FF 0A", "ff0a", "0aff"),
            ("0xBC,0xAB", "bcab", "abbc"),
            ("10 20", "0a14", "1020"),
            ("0x10,0x20", "1020", "0a14"),
            ("239,0xFE", "effe", "feef"),
            ("FF 10", "ff10", "ff0a"),
            ("010,020", "0a14", "0810"),
            ("0Xbc,010", "bc0a", "bc08"),
        ] {
            let query = GuidQuery::compile(text).unwrap();
            assert_eq!(query.digit_count(), 4, "{text}");
            assert_eq!(
                query.matches(&format!("00000000-0000-0000-0000-00000000{expected}")),
                Some(MatchQuality::GuidFragment),
                "{text}"
            );
            assert!(
                query
                    .matches(&format!("00000000-0000-0000-0000-00000000{other}"))
                    .is_none(),
                "{text}"
            );
        }
        let query = GuidQuery::compile("239254").unwrap();
        assert_eq!(query.digit_count(), 6);
        assert!(
            query
                .matches("00000000-0000-0000-0000-000000239254")
                .is_some()
        );
        assert!(
            query
                .matches("00000000-0000-0000-0000-00000000effe")
                .is_none()
        );
        for text in [
            "256,1", "-1,0", "+1,0", "0x+1", "FF 239", "0x100", "1,,2", ",1", "1,", "0x", "１２,1",
            "0xé",
        ] {
            assert!(GuidQuery::compile(text).is_err(), "{text}");
        }
    }
}
