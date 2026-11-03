use std::{borrow::Cow, io::Write};

const LIGHT_HEADER: &[u8] = br"{\rtf1\ansi\deff0\uc1{\fonttbl{\f0 Courier New;}}{\colortbl;\red0\green0\blue128;\red0\green128\blue0;\red128\green0\blue0;\red128\green0\blue128;}\pard\f0 ";
const DARK_HEADER: &[u8] = br"{\rtf1\ansi\deff0\uc1{\fonttbl{\f0 Courier New;}}{\colortbl\red238\green238\blue238;\red86\green156\blue214;\red87\green153\blue106;\red206\green156\blue120;\red181\green206\blue168;}\pard\f0\cf0 ";

pub(super) fn themed(data: &[u8], dark: bool) -> Cow<'_, [u8]> {
    match data.strip_prefix(LIGHT_HEADER).filter(|_| dark) {
        Some(body) => [DARK_HEADER, body].concat().into(),
        None => data.into(),
    }
}

pub(super) fn render(text: &str) -> Vec<u8> {
    let units: Vec<_> = text.encode_utf16().collect();
    let mut rtf = LIGHT_HEADER.to_vec();
    let mut end = 0;
    for (start, next, color) in super::spans(&units) {
        append(&mut rtf, &units[end..start]);
        let index = match color {
            0x800000 => 1,
            0x008000 => 2,
            0x000080 => 3,
            0x800080 => 4,
            _ => unreachable!(),
        };
        write!(rtf, "\\cf{index} ").unwrap();
        append(&mut rtf, &units[start..next]);
        rtf.extend_from_slice(br"\cf0 ");
        end = next;
    }
    append(&mut rtf, &units[end..]);
    // RichEdit consumes the final paragraph marker when importing RTF.
    rtf.extend_from_slice(br"\par }");
    rtf
}

fn append(out: &mut Vec<u8>, text: &[u16]) {
    for &unit in text {
        match unit {
            13 => out.extend_from_slice(br"\par "),
            9 => out.extend_from_slice(br"\tab "),
            92 | 123 | 125 => out.extend_from_slice(&[b'\\', unit as u8]),
            32..=126 => out.push(unit as u8),
            _ => write!(out, "\\u{}?", unit as i16).unwrap(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themed_headers_leave_escaped_body_and_invalid_input_untouched() {
        let light = render("const char *s = \"\\pard\\f0 {中文😀}\"; // comment\r42");
        let dark = themed(&light, true);
        assert_eq!(
            light.strip_prefix(LIGHT_HEADER).unwrap(),
            dark.strip_prefix(DARK_HEADER).unwrap()
        );
        assert!(matches!(themed(&light, false), Cow::Borrowed(_)));
        for invalid in [b"invalid RTF".as_slice(), br"invalid \pard\f0 body", b""] {
            for mode in [false, true] {
                assert_eq!(themed(invalid, mode).as_ref(), invalid);
                assert!(matches!(themed(invalid, mode), Cow::Borrowed(_)));
            }
        }
    }
}
