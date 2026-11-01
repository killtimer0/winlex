//! RichEdit 2.0-compatible declaration presentation. Offsets are UTF-16 positions.
use crate::native::*;
use anyhow::{Context, ensure};
use std::{mem::size_of, sync::OnceLock};
use windows::Win32::UI::Controls::RichEdit::{
    CFE_LINK, CFE_UNDERLINE, CFM_COLOR, CFM_FACE, CFM_LINK, CFM_SIZE, CFM_UNDERLINE, CHARFORMAT2W,
    CHARFORMATW, CHARRANGE, EDITSTREAM, EM_EXGETSEL, EM_EXSETSEL, EM_GETCHARFORMAT,
    EM_GETEVENTMASK, EM_GETOLEINTERFACE, EM_GETSELTEXT, EM_SETBKGNDCOLOR, EM_SETCHARFORMAT,
    EM_SETEVENTMASK, EM_SETPARAFORMAT, EM_SETSCROLLPOS, EM_STREAMIN, ENLINK, ENM_LINK,
    IRichEditOle, ITextDocument, PARAFORMAT, PARAFORMAT2, PFM_LINESPACING, PFM_RIGHTINDENT,
    PFM_STARTINDENT, SCF_ALL, SCF_SELECTION, SF_RTF,
};
use windows_core::Interface;
use winlex_core::EntityId;
use winlex_format::declaration::{Region, RegionKind, Rendered};

mod rtf;
mod scroll;
mod word_break;

pub struct Content {
    text: String,
    rtf: Vec<u8>,
    pub regions: Vec<Region>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewState {
    selection: (i32, i32),
    anchor: i32,
    offset: i32,
    horizontal: i32,
    line_height: i32,
}

impl ViewState {
    pub fn capture(hwnd: HWND) -> Self {
        let mut selection = CHARRANGE::default();
        send_mut(hwnd, EM_EXGETSEL, 0, &mut selection);
        let line = send(hwnd, EM_GETFIRSTVISIBLELINE, 0, 0);
        let anchor = send(hwnd, EM_LINEINDEX, line as usize, 0).max(0) as i32;
        Self {
            selection: (selection.cpMin, selection.cpMax),
            anchor,
            offset: scroll::offset(hwnd, anchor).y,
            horizontal: scroll::position(hwnd).x,
            line_height: scroll::line_height(hwnd).unwrap_or(1),
        }
    }

    pub fn restore(self, hwnd: HWND) {
        let selection = CHARRANGE {
            cpMin: self.selection.0,
            cpMax: self.selection.1,
        };
        send_ref(hwnd, EM_EXSETSEL, 0, &selection);
        let height = scroll::line_height(hwnd).unwrap_or(1);
        let scale = |v| unsafe { MulDiv(v, height, self.line_height.max(1)) };
        let target = POINT {
            x: scale(self.horizontal).max(0),
            y: (scroll::position(hwnd).y - scroll::offset(hwnd, self.anchor).y
                + scale(self.offset))
            .max(0),
        };
        send_ref(hwnd, EM_SETSCROLLPOS, 0, &target);
    }
}

impl Content {
    pub fn new(text: String) -> Self {
        let mut body = Rendered::default();
        body.text = text;
        body.into()
    }
}

impl From<Rendered> for Content {
    fn from(body: Rendered) -> Self {
        let text = body.text.replace("\r\n", "\n").replace('\n', "\r");
        Self {
            rtf: rtf::render(&text),
            text,
            regions: body.regions,
        }
    }
}

pub fn region_at(regions: &[Region], range: std::ops::Range<usize>) -> Option<&Region> {
    regions.iter().find(|region| {
        range.start <= range.end
            && region.range.start <= range.start
            && range.end <= region.range.end
            && (range.start < region.range.end || region.number().is_some())
    })
}

pub fn link_region<'a>(hwnd: HWND, content: &'a Content, link: &ENLINK) -> Option<&'a Region> {
    let range = link.chrg;
    let mut candidates = content.regions.iter().filter(|region| {
        matches!(region.kind, RegionKind::Reference(_))
            && range.cpMin >= 0
            && range.cpMin as usize <= region.range.start
            && region.range.end <= range.cpMax as usize
    });
    let first = candidates.next()?;
    if candidates.next().is_none() {
        return Some(first);
    }
    let point = POINT {
        x: link.lParam.0 as i16 as i32,
        y: (link.lParam.0 >> 16) as i16 as i32,
    };
    let index = send_ref(hwnd, EM_CHARFROMPOS, 0, &point);
    let region = region_at(&content.regions, index as usize..index as usize)?;
    matches!(region.kind, RegionKind::Reference(_)).then_some(region)
}

enum ContextAction {
    Copy(String),
    Navigate(EntityId),
}

/// Capture context before TrackPopupMenu enters its nested message loop.
pub fn context_menu(
    hwnd: HWND,
    location: isize,
    regions: &[Region],
    mouse_region: Option<&Region>,
) -> anyhow::Result<Option<EntityId>> {
    use crate::resources::{
        self, IDS_COPY_DECIMAL, IDS_COPY_HEX, IDS_COPY_SELECTION, IDS_COPY_VALUE,
        IDS_GO_TO_DECLARATION,
    };
    let mut selection = CHARRANGE::default();
    send_mut(hwnd, EM_EXGETSEL, 0, &mut selection);
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
        let scroll = scroll::position(hwnd);
        let mut local = point;
        unsafe {
            ScreenToClient(hwnd, &mut local);
        }
        let index = send_ref(hwnd, EM_CHARFROMPOS, 0, &local) as i32;
        let selected_link = mouse_region.is_some_and(|region| {
            selection.cpMin as usize <= region.range.start
                && region.range.end <= selection.cpMax as usize
        });
        if index >= 0 && !(selection.cpMin..selection.cpMax).contains(&index) && !selected_link {
            selection = CHARRANGE {
                cpMin: index,
                cpMax: index,
            };
            send_ref(hwnd, EM_EXSETSEL, 0, &selection);
        }
        focus(hwnd);
        send_ref(hwnd, EM_SETSCROLLPOS, 0, &scroll);
    }
    let mut buffer = vec![0u16; (selection.cpMax - selection.cpMin).max(0) as usize + 1];
    let length = send(hwnd, EM_GETSELTEXT, 0, buffer.as_mut_ptr() as isize).max(0) as usize;
    let selected = (length != 0).then(|| String::from_utf16_lossy(&buffer[..length]));
    let selected_region = region_at(regions, selection.cpMin as usize..selection.cpMax as usize);
    let region = if location == -1 {
        selected_region
    } else {
        mouse_region.or_else(|| selected_region.filter(|region| region.number().is_some()))
    };
    let mut actions = Vec::new();
    if let Some(region) = region {
        match &region.kind {
            RegionKind::Number(number) => {
                actions.push((
                    IDS_COPY_DECIMAL,
                    Some(ContextAction::Copy(number.value.to_string())),
                ));
                actions.push((
                    IDS_COPY_HEX,
                    Some(ContextAction::Copy(number.hexadecimal())),
                ));
                if number.unit.is_some() {
                    actions.push((
                        IDS_COPY_VALUE,
                        Some(ContextAction::Copy(number.with_unit())),
                    ));
                }
            }
            RegionKind::Reference(target) => {
                actions.push((
                    IDS_GO_TO_DECLARATION,
                    Some(ContextAction::Navigate(*target)),
                ));
            }
        }
    }
    actions.push((IDS_COPY_SELECTION, selected.map(ContextAction::Copy)));
    unsafe {
        let menu = Popup::new()?;
        for (id, value) in &actions {
            if *id == IDS_COPY_SELECTION && region.is_some() {
                menu.separator()?;
            }
            let label = if let Some(ContextAction::Copy(value)) = value
                && *id != IDS_COPY_SELECTION
            {
                format!("{}\t{}", resources::text(*id), value)
            } else {
                resources::text(*id)
            };
            menu.item(*id, &wide(&label), value.is_some())?;
        }
        let command = menu.track(hwnd, point);
        if IsWindow(hwnd) != 0
            && let Some((_, Some(value))) = actions.iter().find(|(id, _)| *id == command)
        {
            match value {
                ContextAction::Copy(value) => copy_text(hwnd, value)?,
                ContextAction::Navigate(target) => return Ok(Some(*target)),
            }
        }
    }
    Ok(None)
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
    load_module(&LOADED, "riched20.dll").context("Load RichEdit")?;
    let hwnd = unsafe {
        child(
            parent,
            w!("RichEdit20W"),
            &[0],
            WS_TABSTOP
                | WS_VSCROLL
                | ES_MULTILINE as u32
                | ES_READONLY as u32
                | ES_AUTOVSCROLL as u32,
            id,
        )
    }?;
    if let Err(error) = scroll::install(hwnd) {
        unsafe { DestroyWindow(hwnd) };
        return Err(error);
    }
    send(hwnd, EM_SETLIMITTEXT, 0x7ffffffe, 0);
    let mask = send(hwnd, EM_GETEVENTMASK, 0, 0);
    send(hwnd, EM_SETEVENTMASK, 0, mask | ENM_LINK as isize);
    send(
        hwnd,
        EM_SETWORDBREAKPROC,
        0,
        word_break::callback as *const () as isize,
    );
    Ok(hwnd)
}

fn load_module(cache: &OnceLock<Result<(), u32>>, name: &str) -> std::io::Result<()> {
    cache
        .get_or_init(|| {
            use std::{
                ffi::OsString,
                os::windows::ffi::{OsStrExt, OsStringExt},
                path::PathBuf,
            };
            use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
            let mut directory = vec![0u16; 260];
            loop {
                let len =
                    unsafe { GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32) };
                if len == 0 {
                    return Err(unsafe { GetLastError() });
                }
                if (len as usize) < directory.len() {
                    directory.truncate(len as usize);
                    break;
                }
                directory.resize(len as usize + 1, 0);
            }
            let path = PathBuf::from(OsString::from_wide(&directory)).join(name);
            let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            if unsafe { LoadLibraryW(path.as_ptr()) }.is_null() {
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
    send_ref(hwnd, EM_SETCHARFORMAT, SCF_ALL as usize, &format);
    let mut selection = CHARRANGE::default();
    send_mut(hwnd, EM_EXGETSEL, 0, &mut selection);
    let all = CHARRANGE {
        cpMin: 0,
        cpMax: -1,
    };
    send_ref(hwnd, EM_EXSETSEL, 0, &all);
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
    send_ref(hwnd, EM_SETPARAFORMAT, 0, &paragraph);
    send_ref(hwnd, EM_EXSETSEL, 0, &selection);
}

pub fn set_content(hwnd: HWND, content: &Content) -> anyhow::Result<()> {
    send(hwnd, WM_SETREDRAW, 0, 0);
    let frozen = freeze(hwnd);
    let result = replace(hwnd, content);
    redraw(hwnd, frozen);
    result
}

pub fn recolor(hwnd: HWND, content: &Content) -> anyhow::Result<()> {
    let view = ViewState::capture(hwnd);
    let result = set_content(hwnd, content);
    view.restore(hwnd);
    result
}

fn freeze(hwnd: HWND) -> Option<ITextDocument> {
    let mut ole = None::<IRichEditOle>;
    send(hwnd, EM_GETOLEINTERFACE, 0, &mut ole as *mut _ as isize);
    let document = ole?.cast::<ITextDocument>().ok()?;
    unsafe { document.Freeze().ok()? };
    Some(document)
}

fn redraw(hwnd: HWND, frozen: Option<ITextDocument>) {
    if let Some(document) = frozen {
        let _ = unsafe { document.Unfreeze() };
    }
    send(hwnd, WM_SETREDRAW, 1, 0);
    // RichEdit20 otherwise clamps deep scrolling to the lazily measured prefix.
    send(hwnd, EM_GETLINECOUNT, 0, 0);
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 1);
    }
}

fn replace(hwnd: HWND, content: &Content) -> anyhow::Result<()> {
    let mut format = CHARFORMATW {
        cbSize: size_of::<CHARFORMATW>() as u32,
        ..Default::default()
    };
    send_mut(hwnd, EM_GETCHARFORMAT, 0, &mut format);
    send(hwnd, EM_SETBKGNDCOLOR, 1, 0);
    let colored = !high_contrast() && unsafe { GetSysColor(COLOR_WINDOW) } == 0x00ffffff;
    let result = if colored {
        stream_in(hwnd, &content.rtf)
    } else {
        Ok(())
    };
    if !colored || result.is_err() {
        set_text(hwnd, &content.text);
        let plain = CHARFORMAT2W {
            Base: CHARFORMATW {
                cbSize: size_of::<CHARFORMAT2W>() as u32,
                dwMask: CFM_COLOR | CFM_LINK | CFM_UNDERLINE,
                crTextColor: windows::Win32::Foundation::COLORREF(unsafe {
                    GetSysColor(COLOR_WINDOWTEXT)
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        send_ref(hwnd, EM_SETCHARFORMAT, SCF_ALL as usize, &plain);
    }
    format.dwMask = CFM_SIZE;
    send_ref(hwnd, EM_SETCHARFORMAT, SCF_ALL as usize, &format);
    font(hwnd);
    apply_regions(hwnd, content)?;
    let range = CHARRANGE::default();
    send_ref(hwnd, EM_EXSETSEL, 0, &range);
    result
}

fn apply_regions(hwnd: HWND, content: &Content) -> anyhow::Result<()> {
    let format = CHARFORMAT2W {
        Base: CHARFORMATW {
            cbSize: size_of::<CHARFORMAT2W>() as u32,
            dwMask: CFM_LINK | CFM_UNDERLINE,
            dwEffects: CFE_LINK | CFE_UNDERLINE,
            ..Default::default()
        },
        ..Default::default()
    };
    for region in &content.regions {
        if matches!(region.kind, RegionKind::Reference(_)) {
            let range = CHARRANGE {
                cpMin: i32::try_from(region.range.start)?,
                cpMax: i32::try_from(region.range.end)?,
            };
            send_ref(hwnd, EM_EXSETSEL, 0, &range);
            ensure!(
                send_ref(hwnd, EM_SETCHARFORMAT, SCF_SELECTION as usize, &format) != 0,
                "Set declaration link failed"
            );
        }
    }
    Ok(())
}

fn stream_in(hwnd: HWND, data: &[u8]) -> anyhow::Result<()> {
    unsafe extern "system" fn read(
        cookie: usize,
        buffer: *mut u8,
        count: i32,
        written: *mut i32,
    ) -> u32 {
        unsafe {
            let remaining = &mut *(cookie as *mut &[u8]);
            let count = (count.max(0) as usize).min(remaining.len());
            std::ptr::copy_nonoverlapping(remaining.as_ptr(), buffer, count);
            *written = count as i32;
            *remaining = &remaining[count..];
        }
        0
    }
    let mut remaining = data;
    let mut stream = EDITSTREAM {
        dwCookie: &mut remaining as *mut &[u8] as usize,
        dwError: 0,
        pfnCallback: Some(read),
    };
    send_mut(hwnd, EM_STREAMIN, SF_RTF as usize, &mut stream);
    let error = stream.dwError;
    ensure!(error == 0, "Stream declaration into RichEdit: {error:#x}");
    Ok(())
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
            i = text[i..]
                .windows(2)
                .position(|pair| pair == [b'*' as u16, b'/' as u16])
                .map_or(text.len(), |end| i + end + 2);
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
        } else if ch <= 127 && ((ch as u8).is_ascii_alphanumeric() || ch == b'_' as u16) {
            let number = (ch as u8).is_ascii_digit();
            let extra = if number { b'.' } else { b'_' } as u16;
            while i < text.len()
                && text[i] <= 127
                && ((text[i] as u8).is_ascii_alphanumeric() || text[i] == extra)
            {
                i += 1;
            }
            if number {
                Some(0x800080)
            } else {
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
            }
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
pub(crate) struct Editor(pub HWND, pub HWND);

#[cfg(test)]
impl Editor {
    pub fn new(style: u32, area: RECT) -> Self {
        let parent = unsafe { test_window(w!("STATIC"), std::ptr::null(), style, area) };
        let mut editor = Self(parent, std::ptr::null_mut());
        editor.1 = create(parent, 1).unwrap();
        editor
    }
}

#[cfg(test)]
impl Drop for Editor {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_links_preserve_utf16_ranges_recoloring_and_plain_text_fallback() {
        use windows::Win32::UI::Controls::RichEdit::EN_LINK;

        unsafe extern "system" fn observe(
            hwnd: HWND,
            msg: u32,
            w: usize,
            l: isize,
            _id: usize,
            data: usize,
        ) -> isize {
            unsafe {
                if msg == WM_NOTIFY && (l as *const NMHDR).read_unaligned().code == EN_LINK {
                    let link = (l as *const ENLINK).read_unaligned();
                    let received = &mut *(data as *mut Vec<ENLINK>);
                    received.push(link);
                    return 0;
                }
                default_subclass(hwnd, msg, w, l)
            }
        }

        unsafe {
            let fixture = Editor::new(
                WS_POPUP | WS_VISIBLE,
                crate::layout::rect(-10000, -10000, 900, 600),
            );
            let Editor(parent, editor) = fixture;
            place(editor, crate::layout::rect(0, 0, 750, 500));
            let mut received = Vec::<ENLINK>::new();
            assert_ne!(
                set_subclass(parent, Some(observe), 1, &mut received as *mut _ as usize),
                0
            );
            let target = EntityId {
                source_id: 7,
                token: 0x02000002,
            };
            let mut rendered = Rendered::from("// 😀\nTarget value;\nTarget");
            rendered.regions = [6..12, 20..26]
                .into_iter()
                .map(|range| Region {
                    range,
                    kind: RegionKind::Reference(target),
                })
                .collect();
            let mut content = Content::from(rendered);
            for plain in [false, true] {
                if plain {
                    content.rtf = b"invalid RTF".to_vec();
                }
                let result = set_content(editor, &content);
                if !plain {
                    result.unwrap();
                }
                assert_eq!(text(editor).replace("\r\n", "\r"), content.text);
                assert_ne!(send(editor, EM_GETEVENTMASK, 0, 0) & ENM_LINK as isize, 0);
                for refresh in [false, true] {
                    if refresh {
                        let range = CHARRANGE {
                            cpMin: 7,
                            cpMax: 10,
                        };
                        send(editor, EM_EXSETSEL, 0, &range as *const _ as isize);
                        let result = recolor(editor, &content);
                        if !plain {
                            result.unwrap();
                        }
                        let mut actual = CHARRANGE::default();
                        send(editor, EM_EXGETSEL, 0, &mut actual as *mut _ as isize);
                        assert_eq!((actual.cpMin, actual.cpMax), (7, 10));
                    }
                    for index in 0..27 {
                        let selected = CHARRANGE {
                            cpMin: index,
                            cpMax: index + 1,
                        };
                        send(editor, EM_EXSETSEL, 0, &selected as *const _ as isize);
                        let mut format = CHARFORMAT2W {
                            Base: CHARFORMATW {
                                cbSize: size_of::<CHARFORMAT2W>() as u32,
                                ..Default::default()
                            },
                            ..Default::default()
                        };
                        send(
                            editor,
                            EM_GETCHARFORMAT,
                            SCF_SELECTION as usize,
                            &mut format as *mut _ as isize,
                        );
                        let linked = content
                            .regions
                            .iter()
                            .any(|r| r.range.contains(&(index as usize)));
                        assert_eq!(
                            format.Base.dwEffects.contains(CFE_LINK),
                            linked,
                            "index {index}"
                        );
                        assert_eq!(
                            format.Base.dwEffects.contains(CFE_UNDERLINE),
                            linked,
                            "index {index}"
                        );
                    }
                    for region in &content.regions {
                        let mut end = POINT { x: 0, y: 0 };
                        let mut last = POINT { x: 0, y: 0 };
                        send(
                            editor,
                            EM_POSFROMCHAR,
                            &mut end as *mut _ as usize,
                            region.range.end as isize,
                        );
                        send(
                            editor,
                            EM_POSFROMCHAR,
                            &mut last as *mut _ as usize,
                            (region.range.end - 1) as isize,
                        );
                        for (x, expected) in
                            [((last.x + end.x) / 2 + 1, Some(target)), (end.x + 30, None)]
                        {
                            received.clear();
                            let caret = CHARRANGE::default();
                            send(editor, EM_EXSETSEL, 0, &caret as *const _ as isize);
                            let location =
                                (((end.y + 6) as u16 as u32) << 16 | x as u16 as u32) as isize;
                            send(editor, WM_LBUTTONDOWN, 9, location);
                            send(editor, WM_LBUTTONUP, 8, location);
                            let actual = received.iter().find_map(|link| {
                                link_region(editor, &content, link).and_then(Region::reference)
                            });
                            assert_eq!(
                                actual,
                                expected,
                                "plain={plain}, refresh={refresh}, range={:?}, end=({}, {}), notifications={}",
                                region.range,
                                end.x,
                                end.y,
                                received.len()
                            );
                            if expected.is_some() {
                                let range = received.last().unwrap().chrg;
                                assert_eq!(
                                    range.cpMin as usize..range.cpMax as usize,
                                    region.range
                                );
                            }
                        }
                    }
                }
                // Replacing linked text must not leave an inherited link on ordinary text.
                set_content(editor, &Content::new("plain replacement".into())).unwrap();
                let all = CHARRANGE {
                    cpMin: 0,
                    cpMax: -1,
                };
                send(editor, EM_EXSETSEL, 0, &all as *const _ as isize);
                let mut format = CHARFORMAT2W {
                    Base: CHARFORMATW {
                        cbSize: size_of::<CHARFORMAT2W>() as u32,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                send(
                    editor,
                    EM_GETCHARFORMAT,
                    SCF_SELECTION as usize,
                    &mut format as *mut _ as isize,
                );
                assert_eq!(format.Base.dwEffects.0 & (CFE_LINK | CFE_UNDERLINE).0, 0);
            }
            drop(fixture);
        }
    }

    #[test]
    fn native_word_selection_and_navigation_use_identifier_boundaries() {
        unsafe {
            let fixture = Editor::new(WS_POPUP, crate::layout::rect(0, 0, 900, 600));
            let editor = fixture.1;
            place(editor, crate::layout::rect(0, 0, 800, 400));
            let body = "ERROR_ACCESS_DENIED  next_value(x);";
            set_content(editor, &Content::new(body.into())).unwrap();
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
        }
    }

    #[test]
    fn large_declaration_load_keeps_ui_pause_bounded() {
        use std::time::{Duration, Instant};
        use winlex_core::{Architecture, Catalog, EntityCategory, SearchMode, SearchOptions};

        let catalog = Catalog::open(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../inputs/Windows.Win32.winmd"),
        )
        .unwrap();
        let entity = catalog
            .search(
                "Windows.Win32.Graphics.Gdi.Apis",
                SearchOptions {
                    mode: SearchMode::Name,
                    architecture: Some(Architecture::X64),
                    categories: Some(EntityCategory::ALL & !(EntityCategory::TypeReference as u32)),
                    ..Default::default()
                },
            )
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let rendered =
            winlex_format::declaration::render(&catalog, entity.id, Architecture::X64).unwrap();
        let body = rendered.text.clone();
        let content = Content::from(rendered);
        let fixture = Editor::new(WS_POPUP | WS_VISIBLE, crate::layout::rect(0, 0, 900, 600));
        let editor = fixture.1;
        place(editor, crate::layout::rect(0, 0, 750, 500));
        focus(editor);
        let start = Instant::now();
        set_content(editor, &content).unwrap();
        let elapsed = start.elapsed();
        let start = Instant::now();
        recolor(editor, &content).unwrap();
        let recolor_elapsed = start.elapsed();
        let actual = text(editor).replace("\r\n", "\n").replace('\r', "\n");
        drop(fixture);
        assert_eq!(actual, body);
        assert!(
            elapsed < Duration::from_secs(3),
            "UI blocked for {elapsed:?}"
        );
        assert!(
            recolor_elapsed < Duration::from_secs(3),
            "UI blocked during recoloring for {recolor_elapsed:?}"
        );
    }

    #[test]
    fn streamed_content_preserves_text_formats_and_reading_state() {
        use windows::Win32::UI::Controls::RichEdit::SCF_SELECTION;

        unsafe {
            let fixture = Editor::new(WS_POPUP | WS_VISIBLE, crate::layout::rect(0, 0, 900, 600));
            let editor = fixture.1;
            place(editor, crate::layout::rect(0, 0, 750, 500));
            focus(editor);
            let size = CHARFORMATW {
                cbSize: size_of::<CHARFORMATW>() as u32,
                dwMask: CFM_SIZE,
                yHeight: 210,
                ..Default::default()
            };
            send(
                editor,
                EM_SETCHARFORMAT,
                SCF_ALL as usize,
                &size as *const _ as isize,
            );
            for body in [
                "",
                "const UINT32 n = 42;",
                "// 中文😀 {\\}\nconst char16_t *s = \"中文😀\\\\{x}\";\n\tUINT32 n = 42;\n\n",
                "one\r\ntwo\rthree\n",
            ] {
                let content = Content::new(body.into());
                set_content(editor, &content).unwrap();
                assert_eq!(text(editor).replace("\r\n", "\r"), content.text);
                recolor(editor, &content).unwrap();
                assert_eq!(text(editor).replace("\r\n", "\r"), content.text);
            }
            let content = Content::new("// 中文😀\nconst char *s = \"中😀\";".into());
            set_content(editor, &content).unwrap();
            recolor(editor, &content).unwrap();
            for (start, end, expected, color) in [
                (0, 7, "// 中文😀", 0x008000),
                (8, 13, "const", 0x800000),
                (24, 29, "\"中😀\"", 0x000080),
            ] {
                let range = CHARRANGE {
                    cpMin: start,
                    cpMax: end,
                };
                send(editor, EM_EXSETSEL, 0, &range as *const _ as isize);
                let mut selected = vec![0u16; (end - start + 1) as usize];
                let count = send(editor, EM_GETSELTEXT, 0, selected.as_mut_ptr() as isize);
                assert_eq!(
                    String::from_utf16(&selected[..count as usize]).unwrap(),
                    expected
                );
                let mut format = CHARFORMATW {
                    cbSize: size_of::<CHARFORMATW>() as u32,
                    ..Default::default()
                };
                send(
                    editor,
                    EM_GETCHARFORMAT,
                    SCF_SELECTION as usize,
                    &mut format as *mut _ as isize,
                );
                let colored = !high_contrast() && GetSysColor(COLOR_WINDOW) == 0x00ffffff;
                assert_eq!(
                    format.crTextColor.0,
                    if colored {
                        color
                    } else {
                        GetSysColor(COLOR_WINDOWTEXT)
                    }
                );
                assert_eq!(format.yHeight, 210);
            }
            let content = Content::new("const UINT32 value = 42;\n".repeat(500));
            set_content(editor, &content).unwrap();
            let range = CHARRANGE {
                cpMin: 6000,
                cpMax: 6005,
            };
            send(editor, EM_EXSETSEL, 0, &range as *const _ as isize);
            send(editor, EM_LINESCROLL, 0, 30);
            let first = send(editor, EM_GETFIRSTVISIBLELINE, 0, 0);
            assert!(first > 0);
            for _ in 0..2 {
                recolor(editor, &content).unwrap();
                let mut actual = CHARRANGE::default();
                send(editor, EM_EXGETSEL, 0, &mut actual as *mut _ as isize);
                assert_eq!((actual.cpMin, actual.cpMax), (range.cpMin, range.cpMax));
                assert_eq!(send(editor, EM_GETFIRSTVISIBLELINE, 0, 0), first);
                assert_eq!(text(editor).replace("\r\n", "\r"), content.text);
            }
        }
    }

    #[test]
    fn visual_wrapping_preserves_declaration_and_paragraph_format() {
        let fixture = Editor::new(WS_POPUP, crate::layout::rect(0, 0, 900, 600));
        let editor = fixture.1;
        let body = "const UINT32 example = 123; // a long declaration with several words that wrap without changing the copied text 😀";
        place(editor, crate::layout::rect(0, 0, 800, 400));
        set_content(editor, &Content::new(body.into())).unwrap();
        let wide_lines = send(editor, EM_GETLINECOUNT, 0, 0);
        place(editor, crate::layout::rect(0, 0, 180, 400));
        assert!(send(editor, EM_GETLINECOUNT, 0, 0) > wide_lines);
        assert_eq!(text(editor), body);
        recolor(editor, &Content::new(body.into())).unwrap();
        assert_eq!(text(editor), body);
        set_content(
            editor,
            &Content::new("struct Example {\n    UINT32 value;\n};".into()),
        )
        .unwrap();
        assert_eq!(
            text(editor).replace("\r\n", "\n").replace('\r', "\n"),
            "struct Example {\n    UINT32 value;\n};"
        );
    }

    #[test]
    fn cached_module_failure_preserves_original_error_code() {
        use super::*;
        let cache = OnceLock::new();
        let failure = load_module(&cache, "Winlex.Test.Missing.RichEdit.dll")
            .context("Load RichEdit")
            .unwrap_err();
        unsafe {
            SetLastError(ERROR_ACCESS_DENIED);
        }
        let cached = load_module(&cache, "Winlex.Test.Missing.RichEdit.dll")
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
    fn highlighting_keeps_numeric_identifier_and_comment_boundaries() {
        for (body, expected) in [
            ("int1 _int UINT32_ 0xAf", vec![("0xAf", 0x800080)]),
            ("/*/ int", vec![("/*/ int", 0x008000)]),
            ("/**/int", vec![("/**/", 0x008000), ("int", 0x800000)]),
            ("/* **/42", vec![("/* **/", 0x008000), ("42", 0x800080)]),
            (
                "/*\r😀*/UINT32",
                vec![("/*\r😀*/", 0x008000), ("UINT32", 0x800000)],
            ),
            (
                "\"int 123\" // UINT32\rconst",
                vec![
                    ("\"int 123\"", 0x000080),
                    ("// UINT32", 0x008000),
                    ("const", 0x800000),
                ],
            ),
        ] {
            let text: Vec<_> = body.encode_utf16().collect();
            let found: Vec<_> = spans(&text)
                .into_iter()
                .map(|(start, end, color)| (String::from_utf16(&text[start..end]).unwrap(), color))
                .collect();
            let expected: Vec<_> = expected
                .into_iter()
                .map(|(token, color)| (token.to_owned(), color))
                .collect();
            assert_eq!(found, expected, "{body}");
        }
        assert_eq!(
            spans(&[b'1' as u16, 0xd800, b'2' as u16, 0xdc00]),
            [(0, 1, 0x800080), (2, 3, 0x800080)]
        );
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
