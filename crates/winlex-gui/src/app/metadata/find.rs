use crate::native::text;
use crate::{
    layout,
    native::*,
    resources::{self, *},
};
use std::cell::{Cell, RefCell};
use windows::Win32::UI::Controls::RichEdit::{
    CHARRANGE, EM_EXGETSEL, EM_EXSETSEL, EM_FINDTEXTEXW, FINDTEXTEXW,
};
use windows::core::PCWSTR;
use windows_sys::Win32::System::SystemServices::SS_CENTERIMAGE;

#[derive(Default)]
pub(super) struct Find {
    controls: Vec<HWND>,
    visible: Cell<bool>,
    updating: Cell<bool>,
    matches: RefCell<Vec<CHARRANGE>>,
}
impl Find {
    pub fn create(&mut self, owner: HWND) -> anyhow::Result<()> {
        for (id, label, class, style) in [
            (FIND_LABEL, IDS_FIND, w!("STATIC"), SS_CENTERIMAGE),
            (
                FIND_QUERY,
                IDS_FIND,
                w!("EDIT"),
                WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32,
            ),
            (FIND_PREVIOUS, IDS_FIND_PREVIOUS, w!("BUTTON"), WS_TABSTOP),
            (FIND_NEXT, IDS_FIND_NEXT, w!("BUTTON"), WS_TABSTOP),
            (FIND_CLOSE, IDS_FIND_CLOSE, w!("BUTTON"), WS_TABSTOP),
            (
                FIND_CASE,
                IDS_FIND_CASE,
                w!("BUTTON"),
                WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            ),
            (
                FIND_WORD,
                IDS_FIND_WORD,
                w!("BUTTON"),
                WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            ),
            (FIND_COUNT, IDS_FIND, w!("STATIC"), SS_CENTERIMAGE),
        ] {
            let h = unsafe { child(owner, class, resources::wide_text(label), style, id) }?;
            show(h, false);
            self.controls.push(h);
        }
        set_text(self.controls[1], "");
        set_text(self.controls[7], "");
        Ok(())
    }
    pub fn prepare(&self, font: HFONT) {
        for &h in &self.controls {
            super::super::bind(h, font);
        }
    }
    pub fn layout(&self, area: &mut RECT, m: &layout::Metrics, positions: &mut Vec<(HWND, RECT)>) {
        if !self.visible.get() {
            return;
        }
        let mut top = layout::take_top(area, m.height + m.px(8));
        let label = layout::take_left(&mut top, m.px(45));
        positions.push((self.controls[0], label));
        let width = ((top.right - top.left - m.px(180)).max(m.px(30))).min(top.right - top.left);
        positions.push((self.controls[1], layout::take_left(&mut top, width)));
        for i in 2..5 {
            positions.push((self.controls[i], layout::take_left(&mut top, m.px(60))));
        }
        let mut bottom = layout::take_top(area, m.height + m.px(8));
        for i in 5..7 {
            positions.push((self.controls[i], layout::take_left(&mut bottom, m.px(110))));
        }
        positions.push((self.controls[7], bottom));
    }
    fn show(&self, visible: bool) {
        self.visible.set(visible);
        for &h in &self.controls {
            show(h, visible);
        }
    }
    pub fn refresh(&self, editor: HWND, navigate: bool) {
        if self.controls.is_empty() || self.updating.get() {
            return;
        }
        let matches = find_matches(
            editor,
            &text(self.controls[1]),
            send(self.controls[5], BM_GETCHECK, 0, 0) != 0,
            send(self.controls[6], BM_GETCHECK, 0, 0) != 0,
        );
        self.matches.replace(matches);
        self.count(editor, false);
        if navigate {
            self.step(editor, false, true);
        }
    }
    pub fn count(&self, editor: HWND, wrapped: bool) {
        if self.controls.is_empty() {
            return;
        }
        let selection = selection(editor);
        let matches = self.matches.borrow();
        let index = matches
            .iter()
            .position(|r| r.cpMin == selection.cpMin && r.cpMax == selection.cpMax)
            .map_or(0, |i| i + 1);
        let suffix = if wrapped {
            resources::text(IDS_FIND_WRAPPED)
        } else {
            String::new()
        };
        set_text(
            self.controls[7],
            &format!("{index}/{} {suffix}", matches.len()),
        );
    }
    fn step(&self, editor: HWND, previous: bool, incremental: bool) {
        let selection = selection(editor);
        let target = {
            let matches = self.matches.borrow();
            if matches.is_empty() {
                return;
            }
            let index = if previous {
                matches.iter().rposition(|r| r.cpMin < selection.cpMin)
            } else {
                matches.iter().position(|r| {
                    r.cpMin
                        >= if incremental {
                            selection.cpMin
                        } else {
                            selection.cpMax
                        }
                })
            };
            (
                matches[index.unwrap_or(if previous { matches.len() - 1 } else { 0 })],
                index.is_none(),
            )
        };
        send(editor, EM_EXSETSEL, 0, &target.0 as *const _ as isize);
        send(
            editor,
            windows::Win32::UI::Controls::RichEdit::EM_HIDESELECTION,
            0,
            0,
        );
        send(editor, EM_SCROLLCARET, 0, 0);
        self.count(editor, target.1);
    }
    pub fn command(&self, editor: HWND, id: u16, code: u32) -> bool {
        match id {
            FIND_DOCUMENT => {
                self.updating.set(true);
                let range = selection(editor);
                if range.cpMax > range.cpMin && unsafe { GetFocus() } == editor {
                    let mut value = vec![0u16; (range.cpMax - range.cpMin) as usize + 1];
                    send(
                        editor,
                        windows::Win32::UI::Controls::RichEdit::EM_GETSELTEXT,
                        0,
                        value.as_mut_ptr() as isize,
                    );
                    let value = String::from_utf16_lossy(&value[..value.len() - 1]);
                    if !value.contains(['\r', '\n']) {
                        set_text(self.controls[1], &value);
                    }
                }
                self.updating.set(false);
                self.show(true);
                self.refresh(editor, false);
                focus(self.controls[1]);
                send(self.controls[1], EM_SETSEL, 0, -1);
            }
            FIND_CLOSE => {
                let position = crate::rich_text::ViewState::capture(editor);
                self.show(false);
                focus(editor);
                position.restore(editor);
            }
            FIND_NEXT | FIND_PREVIOUS => {
                if text(self.controls[1]).is_empty() {
                    return self.command(editor, FIND_DOCUMENT, 0);
                }
                self.step(editor, id == FIND_PREVIOUS, false);
            }
            FIND_QUERY if code == EN_CHANGE => self.refresh(editor, true),
            FIND_CASE | FIND_WORD if code == BN_CLICKED => self.refresh(editor, true),
            _ => return false,
        }
        true
    }
    pub fn key(&self, editor: HWND, msg: &MSG) -> bool {
        if msg.message != WM_KEYDOWN || !self.visible.get() {
            return false;
        }
        if msg.wParam == VK_ESCAPE as usize
            && (self.controls.contains(&msg.hwnd) || msg.hwnd == editor)
        {
            return self.command(editor, FIND_CLOSE, 0);
        }
        if msg.hwnd == self.controls[1] && msg.wParam == VK_RETURN as usize {
            return self.command(
                editor,
                if unsafe { GetKeyState(VK_SHIFT as i32) } < 0 {
                    FIND_PREVIOUS
                } else {
                    FIND_NEXT
                },
                0,
            );
        }
        false
    }
}
fn selection(editor: HWND) -> CHARRANGE {
    let mut range = CHARRANGE::default();
    send(editor, EM_EXGETSEL, 0, &mut range as *mut _ as isize);
    range
}

fn find_matches(editor: HWND, query: &str, match_case: bool, whole_word: bool) -> Vec<CHARRANGE> {
    use windows_sys::Win32::UI::Controls::Dialogs::{FR_DOWN, FR_MATCHCASE, FR_WHOLEWORD};
    let mut matches = Vec::new();
    if query.is_empty() {
        return matches;
    }
    let query = wide(query);
    let flags = FR_DOWN
        | if match_case { FR_MATCHCASE } else { 0 }
        | if whole_word { FR_WHOLEWORD } else { 0 };
    let mut find = FINDTEXTEXW {
        chrg: CHARRANGE {
            cpMin: 0,
            cpMax: -1,
        },
        lpstrText: PCWSTR(query.as_ptr()),
        chrgText: CHARRANGE::default(),
    };
    while send(
        editor,
        EM_FINDTEXTEXW,
        flags as usize,
        &mut find as *mut _ as isize,
    ) >= 0
    {
        if find.chrgText.cpMax <= find.chrg.cpMin {
            break;
        }
        matches.push(find.chrgText);
        find.chrg.cpMin = find.chrgText.cpMax;
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_find_preserves_utf16_ranges_and_passive_reading_position() {
        unsafe {
            let owner = CreateWindowExW(
                0,
                w!("STATIC"),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                640,
                480,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance(),
                std::ptr::null(),
            );
            assert!(!owner.is_null());
            let editor = crate::rich_text::create(owner, 1).unwrap();
            crate::rich_text::set_content(
                editor,
                &crate::rich_text::Content::new("😀 Foo foo food\nFoo".into()),
            )
            .unwrap();
            let selected = CHARRANGE {
                cpMin: 7,
                cpMax: 10,
            };
            send(editor, EM_EXSETSEL, 0, &selected as *const _ as isize);
            for (case, word, expected) in [
                (false, false, vec![(3, 6), (7, 10), (11, 14), (16, 19)]),
                (false, true, vec![(3, 6), (7, 10), (16, 19)]),
                (true, true, vec![(7, 10)]),
            ] {
                let found = find_matches(editor, "foo", case, word);
                assert_eq!(
                    found.iter().map(|r| (r.cpMin, r.cpMax)).collect::<Vec<_>>(),
                    expected
                );
                assert_eq!((selection(editor).cpMin, selection(editor).cpMax), (7, 10));
            }
            assert!(find_matches(editor, "", false, false).is_empty());
            assert!(find_matches(editor, "missing", false, false).is_empty());
            DestroyWindow(owner);
        }
    }
}
