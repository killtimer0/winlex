use std::{borrow::Cow, io::Write, sync::LazyLock};
use winlex_format::declaration::Region;

const LIGHT_HEADER: &[u8] = concat!(
    r"{\rtf1\ansi\deff0\uc1{\fonttbl{\f0 Courier New;}}",
    r"{\colortbl;",
    r"\red0\green0\blue128;",
    r"\red0\green128\blue0;",
    r"\red128\green0\blue0;",
    r"\red128\green0\blue128;",
    r"\red38\green127\blue153;",
    r"\red121\green94\blue38;",
    r"\red0\green16\blue128;}",
    r"\pard\f0 ",
)
.as_bytes();
static DARK_HEADER: LazyLock<Vec<u8>> = LazyLock::new(|| {
    let color = crate::theme::DARK_CONTENT_TEXT;
    format!(
        concat!(
            r"{{\rtf1\ansi\deff0\uc1{{\fonttbl{{\f0 Courier New;}}}}",
            r"{{\colortbl\red{}\green{}\blue{};",
            r"\red86\green156\blue214;",
            r"\red87\green153\blue106;",
            r"\red206\green156\blue120;",
            r"\red181\green206\blue168;",
            r"\red78\green201\blue176;",
            r"\red220\green220\blue170;",
            r"\red156\green220\blue254;}}",
            r"\pard\f0\cf0 ",
        ),
        color & 255,
        color >> 8 & 255,
        color >> 16 & 255
    )
    .into_bytes()
});

pub(super) fn themed(data: &[u8], dark: bool) -> Cow<'_, [u8]> {
    match data.strip_prefix(LIGHT_HEADER).filter(|_| dark) {
        Some(body) => [DARK_HEADER.as_slice(), body].concat().into(),
        None => data.into(),
    }
}

pub(super) fn render(text: &str, regions: &[Region]) -> Vec<u8> {
    let units: Vec<_> = text.encode_utf16().collect();
    let mut rtf = LIGHT_HEADER.to_vec();
    let mut end = 0;
    for (start, next, style) in super::spans(&units, regions) {
        append(&mut rtf, &units[end..start]);
        write!(rtf, "\\cf{style} ").unwrap();
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
    fn text_escapes_rtf_syntax_and_control_characters() {
        let mut output = Vec::new();
        append(
            &mut output,
            &"a\\{b}\t\rc".encode_utf16().collect::<Vec<_>>(),
        );
        assert_eq!(output, br"a\\\{b\}\tab \par c");
    }

    #[test]
    fn unicode_uses_signed_utf16_units_with_fallbacks() {
        let mut output = Vec::new();
        append(&mut output, &"中😀".encode_utf16().collect::<Vec<_>>());
        // RTF \u takes signed 16-bit values, including each surrogate unit.
        assert_eq!(output, br"\u20013?\u-10179?\u-8704?");
    }
}
