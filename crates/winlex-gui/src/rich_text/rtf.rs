use std::io::Write;

pub(super) fn render(text: &str) -> Vec<u8> {
    let units: Vec<_> = text.encode_utf16().collect();
    let mut rtf = br"{\rtf1\ansi\deff0\uc1{\fonttbl{\f0 Courier New;}}{\colortbl;\red0\green0\blue128;\red0\green128\blue0;\red128\green0\blue0;\red128\green0\blue128;}\pard\f0 ".to_vec();
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
