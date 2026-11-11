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
    use winlex_format::declaration::{Region, SymbolRole};

    #[test]
    fn type_coloring_uses_semantics_not_identifier_spelling() {
        let text = "// 😀\rGuid int32;\rusize string;\rstring Guid;";
        let regions: Vec<_> = ["Guid int32", "usize string", "string Guid"]
            .into_iter()
            .map(|declaration| {
                let byte_start = text.find(declaration).unwrap();
                let start = text[..byte_start].encode_utf16().count();
                let ty = declaration.rsplit_once(' ').unwrap().0;
                Region::symbol(start..start + ty.encode_utf16().count(), SymbolRole::Type)
            })
            .collect();
        let output = render(text, &regions);
        let body = String::from_utf8(output).unwrap();
        assert!(body.contains(r"\cf5 Guid\cf0  int32;"));
        assert!(body.contains(r"\cf5 usize\cf0  string;"));
        assert!(body.contains(r"\cf5 string\cf0  Guid;"));
    }

    #[test]
    fn symbol_roles_use_distinct_palette_slots_in_both_themes() {
        let text = "Guid Run value";
        let regions = [
            Region::symbol(0..4, SymbolRole::Type),
            Region::symbol(5..8, SymbolRole::Method),
            Region::symbol(9..14, SymbolRole::Member),
        ];
        let light = render(text, &regions);
        let body = String::from_utf8(light.clone()).unwrap();
        assert!(body.contains(r"\cf5 Guid\cf0  \cf6 Run\cf0  \cf7 value\cf0 "));
        assert_eq!(
            themed(&light, true)
                .strip_prefix(DARK_HEADER.as_slice())
                .unwrap(),
            light.strip_prefix(LIGHT_HEADER).unwrap()
        );
    }

    #[test]
    fn syntax_tokens_use_the_same_palette_slots_in_both_themes() {
        let light = render("const x = 42; \"😀\" // int\r", &[]);
        let expected = concat!(
            r"\cf1 const\cf0  x = \cf4 42\cf0 ; ",
            r#"\cf3 "\u-10179?\u-8704?"\cf0  "#,
            r"\cf2 // int\cf0 \par \par }",
        );
        assert_eq!(
            light.strip_prefix(LIGHT_HEADER).unwrap(),
            expected.as_bytes()
        );
        assert_eq!(
            themed(&light, true)
                .strip_prefix(DARK_HEADER.as_slice())
                .unwrap(),
            expected.as_bytes()
        );
    }

    #[test]
    fn themed_headers_leave_escaped_body_and_invalid_input_untouched() {
        let light = render(
            "const char *s = \"\\pard\\f0 {中文😀}\"; // comment\r42",
            &[],
        );
        let dark = themed(&light, true);
        assert_eq!(
            light.strip_prefix(LIGHT_HEADER).unwrap(),
            dark.strip_prefix(DARK_HEADER.as_slice()).unwrap()
        );
        for invalid in [b"invalid RTF".as_slice(), br"invalid \pard\f0 body", b""] {
            for mode in [false, true] {
                assert_eq!(themed(invalid, mode).as_ref(), invalid);
            }
        }
    }
}
