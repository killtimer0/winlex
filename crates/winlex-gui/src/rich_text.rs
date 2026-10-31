//! RichEdit 2.0-compatible declaration presentation. Offsets are UTF-16 positions.
use crate::native::*;
use anyhow::Context;
use std::{mem::size_of, sync::OnceLock};
use windows::Win32::UI::Controls::RichEdit::{
    CFM_COLOR, CFM_FACE, CHARFORMATW, CHARRANGE, EM_EXGETSEL, EM_EXSETSEL, EM_GETSELTEXT,
    EM_SETBKGNDCOLOR, EM_SETCHARFORMAT, EM_SETPARAFORMAT, PARAFORMAT, PARAFORMAT2, PFM_LINESPACING,
    PFM_RIGHTINDENT, PFM_STARTINDENT, SCF_ALL, SCF_SELECTION,
};
use winlex_format::declaration::NumberRegion;

mod word_break;

/// Capture context before TrackPopupMenu enters its nested message loop.
pub fn context_menu(hwnd: HWND, location: isize, numbers: &[NumberRegion]) -> anyhow::Result<()> {
    use crate::resources::{
        self, IDS_COPY_DECIMAL, IDS_COPY_HEX, IDS_COPY_SELECTION, IDS_COPY_VALUE,
    };
    let mut selection = CHARRANGE::default();
    send(hwnd, EM_EXGETSEL, 0, &mut selection as *mut _ as isize);
    let mut point = POINT {
        x: location as i16 as i32,
        y: (location >> 16) as i16 as i32,
    };
    if location == -1 {
        send(
            hwnd,
            EM_POSFROMCHAR,
            &mut point as *mut _ as usize,
            selection.cpMin as isize,
        );
        let area = client(hwnd);
        point.x = point.x.clamp(0, area.right.saturating_sub(1).max(0));
        point.y = (point.y + 20).clamp(0, area.bottom.saturating_sub(1).max(0));
        unsafe {
            ClientToScreen(hwnd, &mut point);
        }
    } else {
        let mut local = point;
        unsafe {
            SetFocus(hwnd);
            ScreenToClient(hwnd, &mut local);
        }
        let index = send(hwnd, EM_CHARFROMPOS, 0, &local as *const _ as isize) as i32;
        if index >= 0 && !(selection.cpMin..selection.cpMax).contains(&index) {
            selection = CHARRANGE {
                cpMin: index,
                cpMax: index,
            };
            send(hwnd, EM_EXSETSEL, 0, &selection as *const _ as isize);
        }
    }
    let mut buffer = vec![0u16; (selection.cpMax - selection.cpMin).max(0) as usize + 1];
    let length = send(hwnd, EM_GETSELTEXT, 0, buffer.as_mut_ptr() as isize).max(0) as usize;
    let selected = (length != 0).then(|| String::from_utf16_lossy(&buffer[..length]));
    let number = numbers.iter().find(|n| {
        n.range.start <= selection.cpMin as usize && selection.cpMax as usize <= n.range.end
    });
    let mut actions = Vec::new();
    if let Some(number) = number {
        actions.push((IDS_COPY_DECIMAL, Some(number.value.to_string())));
        actions.push((IDS_COPY_HEX, Some(number.hexadecimal())));
        if number.unit.is_some() {
            actions.push((IDS_COPY_VALUE, Some(number.with_unit())));
        }
    }
    actions.push((IDS_COPY_SELECTION, selected));
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return Err(error("Create context menu"));
        }
        for (id, value) in &actions {
            if *id == IDS_COPY_SELECTION && number.is_some() {
                AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            }
            let label = if *id == IDS_COPY_SELECTION {
                resources::text(*id)
            } else {
                format!(
                    "{}\t{}",
                    resources::text(*id),
                    value.as_deref().unwrap_or_default()
                )
            };
            if AppendMenuW(
                menu,
                MF_STRING | if value.is_some() { 0 } else { MF_GRAYED },
                *id as usize,
                wide(&label).as_ptr(),
            ) == 0
            {
                let failure = error("Append context action");
                DestroyMenu(menu);
                return Err(failure);
            }
        }
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            hwnd,
            std::ptr::null(),
        );
        DestroyMenu(menu);
        if IsWindow(hwnd) != 0
            && let Some((_, Some(value))) = actions.iter().find(|(id, _)| *id as i32 == command)
        {
            copy_text(hwnd, value)?;
        }
    }
    Ok(())
}

fn copy_text(hwnd: HWND, value: &str) -> anyhow::Result<()> {
    let text = wide(
        &value
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\n', "\r\n"),
    );
    copy_unicode(hwnd, &text[..text.len() - 1])
}

pub fn create(parent: HWND, id: u16) -> anyhow::Result<HWND> {
    // Keep the module loaded for the lifetime of every control.
    static LOADED: OnceLock<Result<(), u32>> = OnceLock::new();
    load_module(&LOADED, w!("riched20.dll")).context("Load RichEdit")?;
    let hwnd = child(
        parent,
        w!("RichEdit20W"),
        &[0],
        WS_TABSTOP | WS_VSCROLL | ES_MULTILINE as u32 | ES_READONLY as u32 | ES_AUTOVSCROLL as u32,
        id,
    )?;
    send(hwnd, EM_SETLIMITTEXT, 0x7ffffffe, 0);
    send(
        hwnd,
        EM_SETWORDBREAKPROC,
        0,
        word_break::callback as *const () as isize,
    );
    Ok(hwnd)
}

fn load_module(cache: &OnceLock<Result<(), u32>>, name: *const u16) -> std::io::Result<()> {
    cache
        .get_or_init(|| {
            if unsafe { LoadLibraryW(name) }.is_null() {
                Err(unsafe { GetLastError() })
            } else {
                Ok(())
            }
        })
        .map_err(|code| std::io::Error::from_raw_os_error(code as i32))
}

pub fn font(hwnd: HWND) {
    let mut format = CHARFORMATW {
        cbSize: size_of::<CHARFORMATW>() as u32,
        dwMask: CFM_FACE,
        ..Default::default()
    };
    for (out, ch) in format
        .szFaceName
        .iter_mut()
        .zip("Courier New".encode_utf16())
    {
        *out = ch;
    }
    send(
        hwnd,
        EM_SETCHARFORMAT,
        SCF_ALL as usize,
        &format as *const _ as isize,
    );
    let mut selection = CHARRANGE::default();
    send(hwnd, EM_EXGETSEL, 0, &mut selection as *mut _ as isize);
    let all = CHARRANGE {
        cpMin: 0,
        cpMax: -1,
    };
    send(hwnd, EM_EXSETSEL, 0, &all as *const _ as isize);
    let paragraph = PARAFORMAT2 {
        Base: PARAFORMAT {
            cbSize: size_of::<PARAFORMAT2>() as u32,
            dwMask: PFM_LINESPACING | PFM_STARTINDENT | PFM_RIGHTINDENT,
            dxStartIndent: 120,
            dxRightIndent: 120,
            ..Default::default()
        },
        bLineSpacingRule: 5,
        dyLineSpacing: 23,
        ..Default::default()
    };
    send(hwnd, EM_SETPARAFORMAT, 0, &paragraph as *const _ as isize);
    send(hwnd, EM_EXSETSEL, 0, &selection as *const _ as isize);
}

fn color(hwnd: HWND, start: i32, end: i32, value: u32) {
    let range = CHARRANGE {
        cpMin: start,
        cpMax: end,
    };
    send(hwnd, EM_EXSETSEL, 0, &range as *const _ as isize);
    let format = CHARFORMATW {
        cbSize: size_of::<CHARFORMATW>() as u32,
        dwMask: CFM_COLOR,
        crTextColor: windows::Win32::Foundation::COLORREF(value),
        ..Default::default()
    };
    send(
        hwnd,
        EM_SETCHARFORMAT,
        SCF_SELECTION as usize,
        &format as *const _ as isize,
    );
}

pub fn set_content(hwnd: HWND, body: &str, syntax: bool) {
    // RichEdit stores paragraph separators as a single CR.
    let body = body.replace("\r\n", "\n").replace('\n', "\r");
    if text(hwnd).replace("\r\n", "\n").replace('\n', "\r") == body {
        return;
    }
    send(hwnd, WM_SETREDRAW, 0, 0);
    set_text(hwnd, &body);
    font(hwnd);
    apply_colors(hwnd, &body, syntax);
    redraw(hwnd);
}

pub fn recolor(hwnd: HWND, syntax: bool) {
    let body = text(hwnd).replace("\r\n", "\n").replace('\n', "\r");
    send(hwnd, WM_SETREDRAW, 0, 0);
    apply_colors(hwnd, &body, syntax);
    redraw(hwnd);
}

fn redraw(hwnd: HWND) {
    send(hwnd, WM_SETREDRAW, 1, 0);
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 1);
    }
}

fn apply_colors(hwnd: HWND, body: &str, syntax: bool) {
    let mut range = CHARRANGE::default();
    send(hwnd, EM_EXGETSEL, 0, &mut range as *mut _ as isize);
    let first_line = send(hwnd, EM_GETFIRSTVISIBLELINE, 0, 0);
    send(hwnd, EM_SETBKGNDCOLOR, 1, 0);
    color(hwnd, 0, -1, unsafe { GetSysColor(COLOR_WINDOWTEXT) });
    if syntax && !high_contrast() && unsafe { GetSysColor(COLOR_WINDOW) } == 0x00ffffff {
        let chars: Vec<u16> = body.encode_utf16().collect();
        for (start, end, value) in spans(&chars) {
            color(hwnd, start as i32, end as i32, value);
        }
    }
    send(hwnd, EM_EXSETSEL, 0, &range as *const _ as isize);
    send(
        hwnd,
        EM_LINESCROLL,
        0,
        first_line - send(hwnd, EM_GETFIRSTVISIBLELINE, 0, 0),
    );
}

fn spans(text: &[u16]) -> Vec<(usize, usize, u32)> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let start = i;
        let ch = text[i];
        i += 1;
        let value = if ch == b'/' as u16 && text.get(i) == Some(&(b'/' as u16)) {
            while i < text.len() && text[i] != 13 {
                i += 1;
            }
            Some(0x008000)
        } else if ch == b'/' as u16 && text.get(i) == Some(&(b'*' as u16)) {
            i += 1;
            while i < text.len() {
                if text[i] == b'*' as u16 && text.get(i + 1) == Some(&(b'/' as u16)) {
                    i += 2;
                    break;
                }
                i += 1;
            }
            Some(0x008000)
        } else if ch == b'"' as u16 || ch == b'\'' as u16 {
            while i < text.len() {
                let next = text[i];
                i += 1;
                if next == b'\\' as u16 {
                    i = (i + 1).min(text.len());
                } else if next == ch || next == 13 {
                    break;
                }
            }
            Some(0x000080)
        } else if ch <= 127 && (ch as u8).is_ascii_digit() {
            while i < text.len()
                && text[i] <= 127
                && ((text[i] as u8).is_ascii_alphanumeric() || text[i] == b'.' as u16)
            {
                i += 1;
            }
            Some(0x800080)
        } else if ch <= 127 && ((ch as u8).is_ascii_alphabetic() || ch == b'_' as u16) {
            while i < text.len()
                && text[i] <= 127
                && ((text[i] as u8).is_ascii_alphanumeric() || text[i] == b'_' as u16)
            {
                i += 1;
            }
            let word = String::from_utf16_lossy(&text[start..i]);
            matches!(
                word.as_str(),
                "struct"
                    | "class"
                    | "delegate"
                    | "union"
                    | "enum"
                    | "typedef"
                    | "const"
                    | "void"
                    | "interface"
                    | "unsigned"
                    | "signed"
                    | "int"
                    | "long"
                    | "short"
                    | "char"
                    | "float"
                    | "double"
                    | "bool"
                    | "return"
                    | "INT8"
                    | "UINT8"
                    | "INT16"
                    | "UINT16"
                    | "INT32"
                    | "UINT32"
                    | "INT64"
                    | "UINT64"
                    | "INT_PTR"
                    | "UINT_PTR"
                    | "char16_t"
                    | "string"
                    | "object"
            )
            .then_some(0x800000)
        } else {
            None
        };
        if let Some(value) = value {
            result.push((start, i, value));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_word_selection_and_navigation_use_identifier_boundaries() {
        unsafe {
            let parent = CreateWindowExW(
                0,
                w!("STATIC"),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                900,
                600,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance(),
                std::ptr::null(),
            );
            assert!(!parent.is_null());
            let editor = create(parent, 1).unwrap();
            place(editor, crate::layout::rect(0, 0, 800, 400));
            let body = "ERROR_ACCESS_DENIED  next_value(x);";
            set_content(editor, body, true);
            let selection = || {
                let mut range = CHARRANGE::default();
                send(editor, EM_EXGETSEL, 0, &mut range as *mut _ as isize);
                (range.cpMin, range.cpMax)
            };
            for (index, expected) in [(8, (0, 19)), (25, (21, 31)), (31, (31, 32))] {
                let mut point = POINT { x: 0, y: 0 };
                send(editor, EM_POSFROMCHAR, &mut point as *mut _ as usize, index);
                let location = ((point.y + 2) << 16 | (point.x + 2)) as isize;
                send(
                    editor,
                    WM_LBUTTONDOWN,
                    windows_sys::Win32::System::SystemServices::MK_LBUTTON as usize,
                    location,
                );
                send(editor, WM_LBUTTONUP, 0, location);
                send(
                    editor,
                    WM_LBUTTONDBLCLK,
                    windows_sys::Win32::System::SystemServices::MK_LBUTTON as usize,
                    location,
                );
                send(editor, WM_LBUTTONUP, 0, location);
                assert_eq!(selection(), expected);
            }
            let mut original = [0u8; 256];
            assert_ne!(GetKeyboardState(original.as_mut_ptr()), 0);
            for (start, key, shift, expected) in [
                (0, VK_RIGHT, false, (19, 19)),
                (19, VK_RIGHT, false, (21, 21)),
                (21, VK_LEFT, false, (19, 19)),
                (19, VK_LEFT, false, (0, 0)),
                (0, VK_RIGHT, true, (0, 19)),
                (31, VK_LEFT, true, (21, 31)),
            ] {
                let range = CHARRANGE {
                    cpMin: start,
                    cpMax: start,
                };
                send(editor, EM_EXSETSEL, 0, &range as *const _ as isize);
                let mut keys = original;
                keys[VK_CONTROL as usize] = 0x80;
                keys[VK_SHIFT as usize] = if shift { 0x80 } else { 0 };
                assert_ne!(SetKeyboardState(keys.as_ptr()), 0);
                send(editor, WM_KEYDOWN, key as usize, 0);
                send(editor, WM_KEYUP, key as usize, 0);
                let actual = selection();
                assert_ne!(SetKeyboardState(original.as_ptr()), 0);
                assert_eq!(actual, expected);
            }
            DestroyWindow(parent);
        }
    }

    #[test]
    fn visual_wrapping_preserves_declaration_and_paragraph_format() {
        unsafe {
            let parent = CreateWindowExW(
                0,
                w!("STATIC"),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                900,
                600,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance(),
                std::ptr::null(),
            );
            assert!(!parent.is_null());
            let editor = create(parent, 1).unwrap();
            let body = "const UINT32 example = 123; // a long declaration with several words that wrap without changing the copied text 😀";
            place(editor, crate::layout::rect(0, 0, 800, 400));
            set_content(editor, body, true);
            let wide_lines = send(editor, EM_GETLINECOUNT, 0, 0);
            place(editor, crate::layout::rect(0, 0, 180, 400));
            assert!(send(editor, EM_GETLINECOUNT, 0, 0) > wide_lines);
            assert_eq!(text(editor), body);
            let mut paragraph = PARAFORMAT2 {
                Base: PARAFORMAT {
                    cbSize: size_of::<PARAFORMAT2>() as u32,
                    ..Default::default()
                },
                ..Default::default()
            };
            send(
                editor,
                windows::Win32::UI::Controls::RichEdit::EM_GETPARAFORMAT,
                0,
                &mut paragraph as *mut _ as isize,
            );
            let left = paragraph.Base.dxStartIndent;
            let right = paragraph.Base.dxRightIndent;
            assert_eq!(left, 120);
            assert_eq!(right, 120);
            assert_eq!(paragraph.bLineSpacingRule, 5);
            assert_eq!(paragraph.dyLineSpacing, 23);
            set_content(editor, "struct Example {\n    UINT32 value;\n};", true);
            assert_eq!(
                text(editor).replace("\r\n", "\n").replace('\r', "\n"),
                "struct Example {\n    UINT32 value;\n};"
            );
            DestroyWindow(parent);
        }
    }

    #[test]
    fn cached_module_failure_preserves_original_error_code() {
        use super::*;
        let cache = OnceLock::new();
        let failure = load_module(&cache, w!("Winlex.Test.Missing.RichEdit.dll"))
            .context("Load RichEdit")
            .unwrap_err();
        unsafe {
            SetLastError(ERROR_ACCESS_DENIED);
        }
        let cached = load_module(&cache, w!("Winlex.Test.Missing.RichEdit.dll"))
            .context("Load RichEdit")
            .unwrap_err();
        for error in [failure, cached] {
            assert_eq!(
                error
                    .downcast_ref::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(ERROR_MOD_NOT_FOUND as i32)
            );
            assert!(format!("{error:#}").starts_with("Load RichEdit: "));
        }
    }

    #[test]
    fn highlighting_uses_utf16_and_keeps_comments_and_strings_whole() {
        let text: Vec<_> =
            "// 😀 struct 42\rconst char *s = \"a\\\"b\"; /* int 7 */ 123 UINT32 UINT_PTR char16_t"
                .encode_utf16()
                .collect();
        let tokens: Vec<_> = super::spans(&text)
            .into_iter()
            .map(|(start, end, _)| String::from_utf16(&text[start..end]).unwrap())
            .collect();
        assert_eq!(
            tokens,
            [
                "// 😀 struct 42",
                "const",
                "char",
                "\"a\\\"b\"",
                "/* int 7 */",
                "123",
                "UINT32",
                "UINT_PTR",
                "char16_t"
            ]
        );
    }
}
