use windows_sys::Win32::UI::Controls::{
    WB_CLASSIFY, WB_ISDELIMITER, WB_LEFT, WB_LEFTBREAK, WB_MOVEWORDLEFT, WB_MOVEWORDRIGHT,
    WB_RIGHT, WB_RIGHTBREAK,
};

const IDENTIFIER: i32 = 0;
const SPACE: i32 = 1;
const SYMBOL: i32 = 2;
const NEWLINE: i32 = 3;
const WBF_ISWHITE: i32 = 0x10;
const WBF_BREAKLINE: i32 = 0x20;
const WBF_BREAKAFTER: i32 = 0x40;

fn class(ch: u16) -> i32 {
    match ch {
        0x30..=0x39 | 0x41..=0x5a | 0x5f | 0x61..=0x7a => IDENTIFIER,
        0x09 | 0x20 => SPACE,
        0x0a | 0x0d => NEWLINE,
        _ => SYMBOL,
    }
}

fn next(text: &[u16], index: usize) -> usize {
    let mut end = index + 1;
    if (0xd800..=0xdbff).contains(&text[index])
        && text
            .get(end)
            .is_some_and(|ch| (0xdc00..=0xdfff).contains(ch))
    {
        end += 1;
    } else if text[index] == 0x0d {
        if text.get(end..end + 2) == Some(&[0x0d, 0x0a]) {
            end += 2;
        } else if text.get(end) == Some(&0x0a) {
            end += 1;
        }
    }
    end
}

fn previous(text: &[u16], index: usize) -> usize {
    let mut start = index - 1;
    if (0xdc00..=0xdfff).contains(&text[start])
        && start > 0
        && (0xd800..=0xdbff).contains(&text[start - 1])
    {
        start -= 1;
    } else if text[start] == 0x0a {
        for _ in 0..2 {
            if start > 0 && text[start - 1] == 0x0d {
                start -= 1;
            }
        }
    }
    start
}

fn left(text: &[u16], index: usize) -> usize {
    if index == 0 {
        return 0;
    }
    let mut start = previous(text, index);
    let kind = class(text[start]);
    if matches!(kind, IDENTIFIER | SPACE) {
        while start > 0 && class(text[start - 1]) == kind {
            start -= 1;
        }
    }
    start
}

fn right(text: &[u16], index: usize) -> usize {
    if index == text.len() {
        return index;
    }
    let kind = class(text[index]);
    let mut end = next(text, index);
    if matches!(kind, IDENTIFIER | SPACE) {
        while end < text.len() && class(text[end]) == kind {
            end += 1;
        }
    }
    end
}

fn word_break(text: &[u16], current: usize, action: i32) -> i32 {
    let mut index = current.min(text.len());
    // Keep positions on UTF-16 code point boundaries, even for a supplied low surrogate.
    if index > 0
        && index < text.len()
        && (0xdc00..=0xdfff).contains(&text[index])
        && (0xd800..=0xdbff).contains(&text[index - 1])
    {
        index -= 1;
    }
    if index > 0 && index < text.len() && text[index - 1] == 0x0d {
        if text[index] == 0x0a {
            index = previous(text, index + 1);
        } else if text[index] == 0x0d && text.get(index + 1) == Some(&0x0a) {
            index -= 1;
        }
    }
    match action {
        WB_CLASSIFY => match text.get(index).copied().map(class) {
            Some(IDENTIFIER) => IDENTIFIER,
            Some(SPACE) => SPACE | WBF_ISWHITE | WBF_BREAKLINE,
            Some(NEWLINE) | None => NEWLINE | WBF_ISWHITE | WBF_BREAKLINE,
            _ => SYMBOL | WBF_BREAKAFTER,
        },
        WB_ISDELIMITER => i32::from(text.get(index).is_none_or(|&ch| class(ch) != IDENTIFIER)),
        WB_LEFT | WB_LEFTBREAK | WB_MOVEWORDLEFT => left(text, index) as i32,
        WB_RIGHT | WB_RIGHTBREAK | WB_MOVEWORDRIGHT => right(text, index) as i32,
        _ => index as i32,
    }
}

pub(super) unsafe extern "system" fn callback(
    text: *mut u16,
    current: i32,
    length: i32,
    action: i32,
) -> i32 {
    if text.is_null() || length <= 0 {
        return 0;
    }
    // RichEdit supplies length UTF-16 code units; the buffer need not be NUL-terminated.
    let text = unsafe { std::slice::from_raw_parts(text, length as usize) };
    word_break(text, current.max(0) as usize, action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_movement_does_not_split_surrogate_pairs() {
        let text: Vec<_> = "a😀b".encode_utf16().collect();
        for action in [
            WB_LEFT,
            WB_LEFTBREAK,
            WB_MOVEWORDLEFT,
            WB_RIGHT,
            WB_RIGHTBREAK,
            WB_MOVEWORDRIGHT,
        ] {
            for position in 0..=text.len() {
                let result = word_break(&text, position, action);
                assert!((0..=text.len() as i32).contains(&result));
                assert_ne!(result, 2, "stopped inside surrogate pair");
            }
        }
        assert_eq!(word_break(&text, 1, WB_MOVEWORDRIGHT), 3);
        assert_eq!(word_break(&text, 3, WB_MOVEWORDLEFT), 1);
    }

    #[test]
    fn word_movement_does_not_split_crlf() {
        let text: Vec<_> = "a\r\nb".encode_utf16().collect();
        assert_eq!(word_break(&text, 1, WB_MOVEWORDRIGHT), 3);
        assert_eq!(word_break(&text, 3, WB_MOVEWORDLEFT), 1);
        for action in [WB_MOVEWORDLEFT, WB_MOVEWORDRIGHT] {
            for position in 0..=text.len() {
                assert_ne!(word_break(&text, position, action), 2);
            }
        }
    }
}
