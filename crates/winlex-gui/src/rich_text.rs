//! RichEdit 2.0-compatible declaration presentation. Offsets are UTF-16 positions.
use crate::native::*;
use anyhow::{Context, ensure};
use std::{mem::size_of, sync::OnceLock};
use windows::Win32::UI::Controls::RichEdit::{
    CFE_LINK, CFE_LINKPROTECTED, CFE_UNDERLINE, CFM_COLOR, CFM_FACE, CFM_LINK, CFM_LINKPROTECTED,
    CFM_SIZE, CFM_UNDERLINE, CHARFORMAT2W, CHARFORMATW, CHARRANGE, EDITSTREAM, EM_EXGETSEL,
    EM_EXSETSEL, EM_GETCHARFORMAT, EM_GETEVENTMASK, EM_GETOLEINTERFACE, EM_GETSELTEXT, EM_GETZOOM,
    EM_SETBKGNDCOLOR, EM_SETCHARFORMAT, EM_SETEDITSTYLEEX, EM_SETEVENTMASK, EM_SETPARAFORMAT,
    EM_SETSCROLLPOS, EM_SETZOOM, EM_STREAMIN, ENLINK, ENM_LINK, IRichEditOle, ITextDocument,
    PARAFORMAT, PARAFORMAT2, PFM_LINESPACING, PFM_RIGHTINDENT, PFM_STARTINDENT, SCF_ALL,
    SCF_SELECTION, SES_EX_HANDLEFRIENDLYURL, SF_RTF,
};
use windows_core::Interface;
use winlex_core::EntityId;
use winlex_format::declaration::{Region, RegionKind, Rendered};

mod rtf;
mod scroll;
mod word_break;

struct EditorState {
    wheel: scroll::WheelState,
    height: i32,
    dpi: u32,
}

const KEYWORD_STYLE: u8 = 1;
const COMMENT_STYLE: u8 = 2;
const STRING_STYLE: u8 = 3;
const NUMBER_STYLE: u8 = 4;

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
            rtf: rtf::render(&text, &body.regions),
            text,
            regions: body.regions,
        }
    }
}

pub fn region_at(regions: &[Region], range: std::ops::Range<usize>) -> Option<&Region> {
    regions.iter().find(|region| {
        !matches!(region.kind, RegionKind::Type)
            && range.start <= range.end
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
    let location = point_from_lparam(link.lParam.0);
    let index = send_ref(hwnd, EM_CHARFROMPOS, 0, &location);
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
    let mut point = point_from_lparam(location);
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
            RegionKind::Type => {}
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
    static MODERN: OnceLock<Result<(), u32>> = OnceLock::new();
    // RichEdit 2.0 ignores explicit hyperlink colors when painting.
    let modern = crate::theme::supported() && load_module(&MODERN, "msftedit.dll").is_ok();
    let class = if modern {
        w!("RICHEDIT50W")
    } else {
        load_module(&LOADED, "riched20.dll").context("Load RichEdit")?;
        w!("RichEdit20W")
    };
    let hwnd = unsafe {
        child(
            parent,
            class,
            &[0],
            WS_TABSTOP
                | WS_VSCROLL
                | ES_MULTILINE as u32
                | ES_NOHIDESEL as u32
                | ES_READONLY as u32
                | ES_AUTOVSCROLL as u32,
            id,
        )
    }?;
    let state = Box::into_raw(Box::new(EditorState {
        wheel: scroll::WheelState::default(),
        height: 0,
        dpi: 0,
    }));
    if unsafe { set_subclass(hwnd, Some(procedure), 1, state as usize) } == 0 {
        unsafe { drop(Box::from_raw(state)) };
        unsafe { DestroyWindow(hwnd) };
        anyhow::bail!("Subclass RichEdit failed");
    }
    send(hwnd, EM_SETLIMITTEXT, 0x7ffffffe, 0);
    if crate::theme::supported() {
        send(
            hwnd,
            EM_SETEDITSTYLEEX,
            0,
            SES_EX_HANDLEFRIENDLYURL as isize,
        );
    }
    let mask = send(hwnd, EM_GETEVENTMASK, 0, 0);
    send(hwnd, EM_SETEVENTMASK, 0, mask | ENM_LINK as isize);
    send(
        hwnd,
        EM_SETWORDBREAKPROC,
        0,
        word_break::callback as *const () as isize,
    );
    background(hwnd);
    let format = CHARFORMATW {
        cbSize: size_of::<CHARFORMATW>() as u32,
        dwMask: CFM_COLOR,
        crTextColor: windows::Win32::Foundation::COLORREF(crate::theme::color(COLOR_WINDOWTEXT)),
        ..Default::default()
    };
    send_ref(hwnd, EM_SETCHARFORMAT, SCF_ALL as usize, &format);
    Ok(hwnd)
}

unsafe extern "system" fn procedure(
    hwnd: HWND,
    msg: u32,
    w: usize,
    l: isize,
    id: usize,
    data: usize,
) -> isize {
    unsafe {
        match msg {
            WM_GETDLGCODE if w == VK_TAB as usize => {
                return (DLGC_WANTARROWS | DLGC_WANTCHARS) as isize;
            }
            WM_RBUTTONDOWN => return 0,
            WM_RBUTTONUP => {
                let view = ViewState::capture(hwnd);
                focus(hwnd);
                view.restore(hwnd);
            }
            WM_CONTEXTMENU => return send(GetParent(hwnd), msg, hwnd as usize, l),
            WM_KEYDOWN
                if w == VK_APPS as usize
                    || w == VK_F10 as usize && GetKeyState(VK_SHIFT as i32) < 0 =>
            {
                return send(GetParent(hwnd), WM_CONTEXTMENU, hwnd as usize, -1);
            }
            WM_KEYUP
                if w == VK_APPS as usize
                    || w == VK_F10 as usize && GetKeyState(VK_SHIFT as i32) < 0 =>
            {
                return 0;
            }
            WM_MOUSEWHEEL if w & 0xffff == 0 && GetKeyState(VK_MENU as i32) >= 0 => {
                let delta = (w >> 16) as i16;
                if delta != 0 {
                    let mut lines = 3u32;
                    SystemParametersInfoW(
                        SPI_GETWHEELSCROLLLINES,
                        0,
                        (&mut lines as *mut u32).cast(),
                        0,
                    );
                    let (direction, count) = (*(data as *mut EditorState)).wheel.step(delta, lines);
                    scroll::wheel(hwnd, direction, count);
                }
                return 0;
            }
            // Modern RichEdit changes its internal rendering DPI during the
            // BEFORE notification. Own scaling in prepare() for both engines
            // and awareness modes, including controls created on a secondary
            // monitor, instead of compounding native DPI scaling with zoom.
            WM_DPICHANGED_BEFOREPARENT | WM_DPICHANGED_AFTERPARENT => return 0,
            WM_NCDESTROY => {
                remove_subclass(hwnd, Some(procedure), id);
                drop(Box::from_raw(data as *mut EditorState));
            }
            _ => {}
        }
        default_subclass(hwnd, msg, w, l)
    }
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

pub fn prepare(hwnd: HWND, system_font: HFONT, dpi: crate::dpi::Dpi) -> anyhow::Result<()> {
    // Store document sizes in points, independent of the current monitor.
    // WM_SETFONT would reset the document's character colors and effects.
    let (height, render_dpi) = unsafe {
        let dc = GetDC(hwnd);
        anyhow::ensure!(!dc.is_null(), "Get RichEdit font measurement DC failed");
        let previous = SelectObject(dc, system_font);
        let mut metrics: TEXTMETRICW = std::mem::zeroed();
        let measured = GetTextMetricsW(dc, &mut metrics);
        let render_dpi = GetDeviceCaps(dc, LOGPIXELSY as i32);
        SelectObject(dc, previous);
        ReleaseDC(hwnd, dc);
        anyhow::ensure!(measured != 0, "Measure RichEdit font failed");
        (
            MulDiv(
                metrics.tmHeight - metrics.tmInternalLeading,
                1440,
                dpi.system() as i32,
            ),
            render_dpi,
        )
    };
    let mut data = 0;
    anyhow::ensure!(
        unsafe {
            windows_sys::Win32::UI::Shell::GetWindowSubclass(hwnd, Some(procedure), 1, &mut data)
        } != 0,
        "Read RichEdit state failed"
    );
    let state = data as *mut EditorState;
    let target_dpi = dpi.window(hwnd);
    let font_changed = unsafe { (*state).height != height };
    if !font_changed && unsafe { (*state).dpi == target_dpi } {
        return Ok(());
    }
    let view = ViewState::capture(hwnd);
    if font_changed {
        let format = CHARFORMATW {
            cbSize: size_of::<CHARFORMATW>() as u32,
            dwMask: CFM_SIZE,
            yHeight: height,
            ..Default::default()
        };
        send_ref(hwnd, EM_SETCHARFORMAT, SCF_ALL as usize, &format);
        self::font(hwnd);
        unsafe {
            (*state).height = height;
        }
    }
    // Without native DPI updates, both engines render at the system DC DPI,
    // not necessarily the DPI of the monitor where the control was created.
    anyhow::ensure!(
        send(hwnd, EM_SETZOOM, target_dpi as usize, render_dpi as isize) != 0,
        "Set RichEdit DPI zoom failed"
    );
    unsafe {
        (*state).dpi = target_dpi;
    }
    view.restore(hwnd);
    Ok(())
}

fn font(hwnd: HWND) {
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
    let (mut numerator, mut denominator) = (0i32, 0i32);
    send(
        hwnd,
        EM_GETZOOM,
        &mut numerator as *mut _ as usize,
        &mut denominator as *mut _ as isize,
    );
    send(hwnd, WM_SETREDRAW, 0, 0);
    let frozen = freeze(hwnd);
    let result = replace(hwnd, content);
    // Both RTF streaming and plain-text replacement reset display zoom.
    let zoomed = send(hwnd, EM_SETZOOM, numerator as usize, denominator as isize);
    redraw(hwnd, frozen);
    result?;
    ensure!(zoomed != 0, "Restore RichEdit zoom failed");
    Ok(())
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

fn background(hwnd: HWND) {
    send(
        hwnd,
        EM_SETBKGNDCOLOR,
        0,
        crate::theme::color(COLOR_WINDOW) as isize,
    );
}

fn replace(hwnd: HWND, content: &Content) -> anyhow::Result<()> {
    let mut format = CHARFORMATW {
        cbSize: size_of::<CHARFORMATW>() as u32,
        ..Default::default()
    };
    send_mut(hwnd, EM_GETCHARFORMAT, 0, &mut format);
    background(hwnd);
    let colored = syntax_enabled();
    let result = if colored {
        stream_in(hwnd, &rtf::themed(&content.rtf, crate::theme::is_dark()))
    } else {
        Ok(())
    };
    if !colored || result.is_err() {
        set_text(hwnd, &content.text);
        let plain = CHARFORMAT2W {
            Base: CHARFORMATW {
                cbSize: size_of::<CHARFORMAT2W>() as u32,
                dwMask: CFM_COLOR
                    | CFM_LINK
                    | CFM_UNDERLINE
                    | if crate::theme::supported() {
                        CFM_LINKPROTECTED
                    } else {
                        Default::default()
                    },
                crTextColor: windows::Win32::Foundation::COLORREF(crate::theme::color(
                    COLOR_WINDOWTEXT,
                )),
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

fn syntax_enabled() -> bool {
    !high_contrast() && (crate::theme::is_dark() || crate::theme::color(COLOR_WINDOW) == 0x00ffffff)
}

fn apply_regions(hwnd: HWND, content: &Content) -> anyhow::Result<()> {
    use windows::Win32::UI::Controls::RichEdit::ITextSelection2;

    let ranges = content
        .regions
        .iter()
        .filter(|region| matches!(region.kind, RegionKind::Reference(_)))
        .map(|region| {
            Ok(CHARRANGE {
                cpMin: i32::try_from(region.range.start)?,
                cpMax: i32::try_from(region.range.end)?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let Some(first) = ranges.first() else {
        return Ok(());
    };
    let modern = crate::theme::supported();
    let mut format = CHARFORMAT2W {
        Base: CHARFORMATW {
            cbSize: size_of::<CHARFORMAT2W>() as u32,
            dwMask: CFM_LINK | CFM_UNDERLINE,
            dwEffects: CFE_LINK | CFE_UNDERLINE,
            crTextColor: windows::Win32::Foundation::COLORREF(if high_contrast() {
                crate::theme::color(COLOR_WINDOWTEXT)
            } else {
                crate::theme::color(COLOR_HOTLIGHT)
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    if modern {
        format.Base.dwMask |= CFM_COLOR | CFM_LINKPROTECTED;
        format.Base.dwEffects |= CFE_LINKPROTECTED;
    }
    let apply = || {
        ensure!(
            send_ref(hwnd, EM_SETCHARFORMAT, SCF_SELECTION as usize, &format) != 0,
            "Set declaration link failed"
        );
        Ok(())
    };
    let mut ole = None::<IRichEditOle>;
    send(hwnd, EM_GETOLEINTERFACE, 0, &mut ole as *mut _ as isize);
    let selection = ole
        .and_then(|ole| ole.cast::<ITextDocument>().ok())
        .and_then(|document| unsafe { document.GetSelection().ok() })
        .and_then(|selection| selection.cast::<ITextSelection2>().ok());
    if let Some(selection) = selection {
        // One native formatting operation avoids repeatedly updating every UIA
        // text range while keeping the control's original hyperlink semantics.
        let mut saved = CHARRANGE::default();
        send_mut(hwnd, EM_EXGETSEL, 0, &mut saved);
        let result = (|| unsafe {
            selection.SetRange(first.cpMin, first.cpMax)?;
            for range in &ranges[1..] {
                selection.AddSubrange(range.cpMin, range.cpMax, 0)?;
            }
            apply()
        })();
        // SetRange also removes the temporary subranges, including on failure.
        let restored = unsafe { selection.SetRange(saved.cpMin, saved.cpMax) };
        result.and(restored.map_err(Into::into))
    } else {
        // RichEdit20 lacks TOM2. Balance only our own caret hide, so formatting
        // neither steals focus nor makes an already hidden caret visible.
        let hidden = unsafe { HideCaret(hwnd) } != 0;
        let result = ranges.iter().try_for_each(|range| {
            send_ref(hwnd, EM_EXSETSEL, 0, range);
            apply()
        });
        if hidden {
            unsafe { ShowCaret(hwnd) };
        }
        result
    }
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

fn spans(text: &[u16], regions: &[Region]) -> Vec<(usize, usize, u8)> {
    let mut result = Vec::new();
    let mut types = regions
        .iter()
        .filter(|region| {
            matches!(region.kind, RegionKind::Type | RegionKind::Reference(_))
                && region.range.start < region.range.end
                && region.range.end <= text.len()
        })
        .peekable();
    let mut i = 0;
    while i < text.len() {
        while types.peek().is_some_and(|region| region.range.start < i) {
            types.next();
        }
        if let Some(region) = types.peek().filter(|region| region.range.start == i) {
            result.push((i, region.range.end, KEYWORD_STYLE));
            i = region.range.end;
            types.next();
            continue;
        }
        let start = i;
        let ch = text[i];
        i += 1;
        let value = if ch == b'/' as u16 && text.get(i) == Some(&(b'/' as u16)) {
            while i < text.len() && text[i] != 13 {
                i += 1;
            }
            Some(COMMENT_STYLE)
        } else if ch == b'/' as u16 && text.get(i) == Some(&(b'*' as u16)) {
            i += 1;
            i = text[i..]
                .windows(2)
                .position(|pair| pair == [b'*' as u16, b'/' as u16])
                .map_or(text.len(), |end| i + end + 2);
            Some(COMMENT_STYLE)
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
            Some(STRING_STYLE)
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
                Some(NUMBER_STYLE)
            } else {
                [
                    "struct",
                    "class",
                    "delegate",
                    "union",
                    "enum",
                    "typedef",
                    "const",
                    "void",
                    "interface",
                    "return",
                ]
                .iter()
                .any(|word| text[start..i].iter().copied().eq(word.encode_utf16()))
                .then_some(KEYWORD_STYLE)
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

    pub fn select(&self, start: i32, end: i32) {
        let range = CHARRANGE {
            cpMin: start,
            cpMax: end,
        };
        send_ref(self.1, EM_EXSETSEL, 0, &range);
    }

    pub fn selection(&self) -> (i32, i32) {
        let mut range = CHARRANGE::default();
        send_mut(self.1, EM_EXGETSEL, 0, &mut range);
        (range.cpMin, range.cpMax)
    }

    pub fn character_format(&self) -> CHARFORMAT2W {
        let mut format = CHARFORMAT2W {
            Base: CHARFORMATW {
                cbSize: size_of::<CHARFORMAT2W>() as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        send_mut(
            self.1,
            EM_GETCHARFORMAT,
            SCF_SELECTION as usize,
            &mut format,
        );
        format
    }

    pub fn font_size(&self, height: i32) {
        let format = CHARFORMATW {
            cbSize: size_of::<CHARFORMATW>() as u32,
            dwMask: CFM_SIZE,
            yHeight: height,
            ..Default::default()
        };
        send_ref(self.1, EM_SETCHARFORMAT, SCF_ALL as usize, &format);
    }

    pub fn character_position(&self, index: i32) -> POINT {
        let mut point = POINT { x: 0, y: 0 };
        send(
            self.1,
            EM_POSFROMCHAR,
            &mut point as *mut _ as usize,
            index as isize,
        );
        point
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
    fn many_disjoint_links_preserve_plain_gaps_and_single_selection() {
        let mut rendered = Rendered::default();
        let target = EntityId {
            source_id: 0,
            token: 0x02000001,
        };
        let mut position = 0;
        for _ in 0..1084 {
            rendered.text.push_str("😀 Target plain\n");
            rendered.regions.push(Region {
                range: position + 3..position + 9,
                kind: RegionKind::Reference(target),
            });
            position += 16;
        }
        let content = Content::from(rendered);
        for modern in [true, false] {
            let fixture = if modern {
                Editor::new(
                    WS_POPUP | WS_VISIBLE,
                    crate::layout::rect(-10000, -10000, 900, 600),
                )
            } else {
                static LEGACY: OnceLock<Result<(), u32>> = OnceLock::new();
                load_module(&LEGACY, "riched20.dll").unwrap();
                let parent = unsafe {
                    test_window(
                        w!("STATIC"),
                        std::ptr::null(),
                        WS_POPUP | WS_VISIBLE,
                        crate::layout::rect(-10000, -10000, 900, 600),
                    )
                };
                let editor = unsafe {
                    child(
                        parent,
                        w!("RichEdit20W"),
                        &[0],
                        WS_VISIBLE | ES_MULTILINE as u32 | ES_READONLY as u32 | WS_VSCROLL,
                        1,
                    )
                }
                .unwrap();
                send(editor, EM_SETLIMITTEXT, 0x7ffffffe, 0);
                Editor(parent, editor)
            };
            set_bounds(fixture.1, crate::layout::rect(0, 0, 750, 500)).unwrap();
            focus(fixture.1);
            set_content(fixture.1, &content).unwrap();
            for mode in [1, 2] {
                crate::theme::set_mode(mode);
                fixture.select(3, 9);
                send(fixture.1, EM_LINESCROLL, 0, 30);
                let view = ViewState::capture(fixture.1);
                recolor(fixture.1, &content).unwrap();
                assert_eq!(ViewState::capture(fixture.1), view);
                assert_eq!(unsafe { GetFocus() }, fixture.1);
                assert_eq!(text(fixture.1).replace("\r\n", "\r"), content.text);
                let mut selected = [0u16; 32];
                assert_eq!(
                    send(fixture.1, EM_GETSELTEXT, 0, selected.as_mut_ptr() as isize),
                    6
                );
                assert_eq!(String::from_utf16(&selected[..6]).unwrap(), "Target");
                for region in &content.regions {
                    for (start, end, linked) in [
                        (region.range.start, region.range.end, true),
                        (region.range.start - 1, region.range.start, false),
                        (region.range.end, region.range.end + 1, false),
                    ] {
                        fixture.select(start as i32, end as i32);
                        let format = fixture.character_format().Base;
                        assert_eq!(format.dwEffects.contains(CFE_LINK), linked);
                        assert_eq!(format.dwEffects.contains(CFE_UNDERLINE), linked);
                    }
                }
                // Subsequent ordinary formatting must affect only the requested
                // selection, not any of the temporary disjoint link ranges.
                fixture.select(10, 15);
                let plain = CHARFORMATW {
                    cbSize: size_of::<CHARFORMATW>() as u32,
                    dwMask: CFM_UNDERLINE,
                    ..Default::default()
                };
                send_ref(fixture.1, EM_SETCHARFORMAT, SCF_SELECTION as usize, &plain);
                fixture.select(3, 9);
                assert!(
                    fixture
                        .character_format()
                        .Base
                        .dwEffects
                        .contains(CFE_UNDERLINE)
                );
                send(fixture.1, WM_CHAR, b'X' as usize, 0);
                assert_eq!(text(fixture.1).replace("\r\n", "\r"), content.text);
            }
        }
        crate::theme::set_mode(0);
    }

    #[test]
    fn content_replacement_preserves_zoom_and_rendered_size() {
        let editor = Editor::new(WS_POPUP, crate::layout::rect(0, 0, 900, 600));
        set_bounds(editor.1, crate::layout::rect(0, 0, 750, 500)).unwrap();
        let body = "const UINT32 value = 42;\n";
        let content = Content::new(body.into());
        set_content(editor.1, &content).unwrap();
        for (numerator, denominator) in [(1, 2), (3, 2), (0, 0)] {
            assert_ne!(send(editor.1, EM_SETZOOM, numerator, denominator), 0);
            let before = editor.character_position(5).x - editor.character_position(0).x;
            assert!(before > 0);
            for failed_stream in [false, true] {
                let mut replacement = Content::new(body.into());
                if failed_stream {
                    replacement.rtf = b"not RTF".to_vec();
                }
                let result = set_content(editor.1, &replacement);
                if failed_stream && syntax_enabled() {
                    assert!(result.is_err());
                } else {
                    result.unwrap();
                }
                assert_eq!(text(editor.1).replace("\r\n", "\r"), content.text);
                let (mut actual_numerator, mut actual_denominator) = (0i32, 0i32);
                send(
                    editor.1,
                    EM_GETZOOM,
                    &mut actual_numerator as *mut _ as usize,
                    &mut actual_denominator as *mut _ as isize,
                );
                assert_eq!(
                    (actual_numerator, actual_denominator),
                    (numerator as i32, denominator as i32)
                );
                assert_eq!(
                    editor.character_position(5).x - editor.character_position(0).x,
                    before
                );
            }
        }
    }

    #[test]
    fn dpi_zoom_preserves_character_colors_sizes_and_links() {
        let fixture = Editor::new(WS_POPUP, crate::layout::rect(0, 0, 900, 600));
        let reference = Editor::new(WS_POPUP, crate::layout::rect(0, 0, 900, 600));
        for editor in [fixture.1, reference.1] {
            set_bounds(editor, crate::layout::rect(0, 0, 750, 500)).unwrap();
        }
        let dpi = crate::dpi::Dpi {
            mode: crate::dpi::Mode::System,
        };
        let initial_font = crate::dpi::Font::new(dpi.system()).unwrap();
        prepare(fixture.1, initial_font.handle.0, dpi).unwrap();
        send(reference.1, WM_SETFONT, initial_font.handle.0 as usize, 0);
        super::font(reference.1);
        let initial_height = reference.character_format().Base.yHeight;
        assert_eq!(fixture.character_format().Base.yHeight, initial_height);
        let body = "const UINT32 value = 42; // comment";
        for mode in [2, 1] {
            crate::theme::set_mode(mode);
            let content = Content::new(body.into());
            set_content(fixture.1, &content).unwrap();
            set_content(reference.1, &content).unwrap();
            assert_eq!(
                fixture.character_format().Base.yHeight,
                reference.character_format().Base.yHeight,
                "content loading must retain the previously applied font size"
            );
            // A native link is another independently formatted range, not just
            // the document's default foreground color.
            fixture.select(13, 18);
            let link = CHARFORMATW {
                cbSize: size_of::<CHARFORMATW>() as u32,
                dwMask: CFM_LINK | CFM_COLOR | CFM_UNDERLINE,
                dwEffects: CFE_LINK | CFE_UNDERLINE,
                crTextColor: windows::Win32::Foundation::COLORREF(crate::theme::color(
                    COLOR_HOTLIGHT,
                )),
                ..Default::default()
            };
            send_ref(fixture.1, EM_SETCHARFORMAT, SCF_SELECTION as usize, &link);
            fixture.select(6, 12);
            let large = CHARFORMATW {
                cbSize: size_of::<CHARFORMATW>() as u32,
                dwMask: CFM_SIZE,
                yHeight: 360,
                ..Default::default()
            };
            send_ref(fixture.1, EM_SETCHARFORMAT, SCF_SELECTION as usize, &large);
            let samples = [(0, 5), (6, 12), (13, 18), (26, 33)];
            let before: Vec<_> = samples
                .iter()
                .map(|&(start, end)| {
                    fixture.select(start, end);
                    let format = fixture.character_format().Base;
                    (format.crTextColor.0, format.dwEffects, format.yHeight)
                })
                .collect();
            for (numerator, denominator) in [(2, 1), (1, 1), (3, 2), (1, 1)] {
                fixture.select(6, 12);
                assert_ne!(send(fixture.1, EM_SETZOOM, numerator, denominator), 0);
                // Refreshing unchanged settings must not flatten mixed sizes.
                prepare(fixture.1, initial_font.handle.0, dpi).unwrap();
                assert_eq!(fixture.selection(), (6, 12));
                for (&(start, end), expected) in samples.iter().zip(&before) {
                    fixture.select(start, end);
                    let format = fixture.character_format().Base;
                    assert_eq!(
                        (format.crTextColor.0, format.dwEffects, format.yHeight),
                        *expected,
                        "zoom changed formatting at {start}..{end}, mode={mode}"
                    );
                }
            }
        }
        crate::theme::set_mode(0);
    }

    #[test]
    fn dpi_zoom_preserves_scrolled_reading_anchor() {
        let fixture = Editor::new(WS_POPUP, crate::layout::rect(0, 0, 1000, 600));
        set_bounds(fixture.1, crate::layout::rect(0, 0, 950, 500)).unwrap();
        let line = "const UINT32 value = 42;\n";
        set_content(fixture.1, &Content::new(line.repeat(500))).unwrap();
        fixture.select(6000, 6005);
        send(fixture.1, EM_LINESCROLL, 0, 100);
        let view = ViewState::capture(fixture.1);
        assert!(view.anchor > 0);
        for (numerator, denominator) in [(2, 1), (1, 1), (3, 2), (1, 1)] {
            let before = ViewState::capture(fixture.1);
            assert_ne!(send(fixture.1, EM_SETZOOM, numerator, denominator), 0);
            before.restore(fixture.1);
            let current = ViewState::capture(fixture.1);
            assert_eq!(current.selection, view.selection);
            assert_eq!(
                current.anchor, view.anchor,
                "font update must preserve the first visible logical line"
            );
        }
    }

    #[test]
    fn native_link_pixels_use_theme_color_after_dark_and_light_recoloring() {
        unsafe fn painted_pixels(hwnd: HWND, color: u32) -> usize {
            unsafe {
                let area = client(hwnd);
                let dc = CreateCompatibleDC(std::ptr::null_mut());
                assert!(!dc.is_null());
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: area.right,
                        biHeight: -area.bottom,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..std::mem::zeroed()
                    },
                    ..std::mem::zeroed()
                };
                let mut pixels = std::ptr::null_mut();
                let bitmap = GdiObject(CreateDIBSection(
                    dc,
                    &info,
                    DIB_RGB_COLORS,
                    &mut pixels,
                    std::ptr::null_mut(),
                    0,
                ));
                assert!(!bitmap.0.is_null());
                let old = SelectObject(dc, bitmap.0);
                send(hwnd, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
                GdiFlush();
                let rgb = (color & 255) << 16 | (color & 0xff00) | (color >> 16 & 255);
                let count = std::slice::from_raw_parts(
                    pixels.cast::<u32>(),
                    (area.right * area.bottom) as usize,
                )
                .iter()
                .filter(|&&pixel| pixel & 0xffffff == rgb)
                .count();
                SelectObject(dc, old);
                DeleteDC(dc);
                count
            }
        }

        let fixture = Editor::new(
            WS_POPUP | WS_VISIBLE,
            crate::layout::rect(-10000, -10000, 600, 200),
        );
        set_bounds(fixture.1, crate::layout::rect(0, 0, 500, 100)).unwrap();
        fixture.font_size(360);
        let mut rendered = Rendered::from("Target");
        rendered.regions.push(Region {
            range: 0..6,
            kind: RegionKind::Reference(EntityId {
                source_id: 0,
                token: 0x02000001,
            }),
        });
        let content = Content::from(rendered);
        for mode in [2, 1, 2, 1] {
            crate::theme::set_mode(mode);
            recolor(fixture.1, &content).unwrap();
            let color = crate::theme::color(if high_contrast() {
                COLOR_WINDOWTEXT
            } else {
                COLOR_HOTLIGHT
            });
            assert!(
                unsafe { painted_pixels(fixture.1, color) } > 20,
                "missing rendered hyperlink color {color:06x} in mode {mode}"
            );
        }
        drop(fixture);
        crate::theme::set_mode(0);
    }

    #[test]
    fn native_links_preserve_utf16_ranges_recoloring_and_plain_text_fallback() {
        use windows::Win32::UI::Controls::RichEdit::EN_LINK;

        unsafe {
            let fixture = Editor::new(
                WS_POPUP | WS_VISIBLE,
                crate::layout::rect(-10000, -10000, 900, 600),
            );
            let Editor(parent, editor) = fixture;
            set_bounds(editor, crate::layout::rect(0, 0, 750, 500)).unwrap();
            let received = std::cell::RefCell::new(Vec::<ENLINK>::new());
            let observe = |_, msg, _, l| {
                if msg == WM_NOTIFY && (l as *const NMHDR).read_unaligned().code == EN_LINK {
                    received
                        .borrow_mut()
                        .push((l as *const ENLINK).read_unaligned());
                    return Some(0);
                }
                None
            };
            let callback = crate::test_support::observe_messages(parent, 1, &observe);
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
                for mode in [None, Some(2), Some(1), Some(2), Some(1)] {
                    let refresh = mode.is_some();
                    if let Some(mode) = mode {
                        crate::theme::set_mode(mode);
                    }
                    if refresh {
                        fixture.select(7, 10);
                        let result = recolor(editor, &content);
                        if !plain {
                            result.unwrap();
                        }
                        assert_eq!(fixture.selection(), (7, 10));
                    }
                    for index in 0..27 {
                        fixture.select(index, index + 1);
                        let format = fixture.character_format();
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
                        if linked && crate::theme::supported() {
                            assert!(
                                !format.Base.dwEffects.contains(
                                    windows::Win32::UI::Controls::RichEdit::CFE_AUTOCOLOR
                                )
                            );
                            assert_eq!(
                                format.Base.crTextColor.0,
                                crate::theme::color(if high_contrast() {
                                    COLOR_WINDOWTEXT
                                } else {
                                    COLOR_HOTLIGHT
                                }),
                                "link color at {index}, mode={mode:?}"
                            );
                        }
                    }
                    for region in &content.regions {
                        let end = fixture.character_position(region.range.end as i32);
                        let last = fixture.character_position((region.range.end - 1) as i32);
                        for (x, expected) in
                            [((last.x + end.x) / 2 + 1, Some(target)), (end.x + 30, None)]
                        {
                            received.borrow_mut().clear();
                            fixture.select(0, 0);
                            let location =
                                (((end.y + 6) as u16 as u32) << 16 | x as u16 as u32) as isize;
                            send(editor, WM_LBUTTONDOWN, 9, location);
                            send(editor, WM_LBUTTONUP, 8, location);
                            let links = received.take();
                            let actual = links.iter().find_map(|link| {
                                link_region(editor, &content, link).and_then(Region::reference)
                            });
                            assert_eq!(
                                actual,
                                expected,
                                "plain={plain}, refresh={refresh}, range={:?}, end=({}, {}), notifications={}",
                                region.range,
                                end.x,
                                end.y,
                                links.len()
                            );
                            if expected.is_some() {
                                let range = links.last().unwrap().chrg;
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
                fixture.select(0, -1);
                let format = fixture.character_format();
                assert_eq!(format.Base.dwEffects.0 & (CFE_LINK | CFE_UNDERLINE).0, 0);
            }
            remove_subclass(parent, callback, 1);
            drop(fixture);
            crate::theme::set_mode(0);
        }
    }

    #[test]
    fn signature_types_are_colored_without_creating_navigation_links() {
        let fixture = Editor::new(WS_POPUP, crate::layout::rect(0, 0, 900, 600));
        let mut rendered = Rendered::from("// 😀\nconst Guid int32;\nusize string;\nstring Guid;");
        for ty in ["Guid", "usize", "string Guid"] {
            let byte_start = rendered.text.find(ty).unwrap();
            let start = rendered.text[..byte_start].encode_utf16().count();
            let length = if ty == "string Guid" {
                6
            } else {
                ty.encode_utf16().count()
            };
            rendered.regions.push(Region {
                range: start..start + length,
                kind: RegionKind::Type,
            });
        }
        let content = Content::from(rendered);
        for mode in [1, 2] {
            crate::theme::set_mode(mode);
            set_content(fixture.1, &content).unwrap();
            fixture.select(6, 11); // const, after the UTF-16 comment and newline
            let keyword_color = fixture.character_format().Base.crTextColor;
            for region in &content.regions {
                for index in region.range.clone() {
                    fixture.select(index as i32, index as i32 + 1);
                    let format = fixture.character_format();
                    assert_eq!(format.Base.crTextColor, keyword_color);
                    assert_eq!(format.Base.dwEffects.0 & (CFE_LINK | CFE_UNDERLINE).0, 0);
                    assert!(region_at(&content.regions, index..index).is_none());
                }
            }
            for word in ["int32", "string;", "Guid;"] {
                let byte_start = content.text.find(word).unwrap();
                let start = content.text[..byte_start].encode_utf16().count();
                fixture.select(start as i32, start as i32 + 1);
                assert_ne!(fixture.character_format().Base.crTextColor, keyword_color);
            }
        }
        crate::theme::set_mode(0);
    }

    #[test]
    fn native_word_selection_and_navigation_use_identifier_boundaries() {
        unsafe {
            let fixture = Editor::new(WS_POPUP, crate::layout::rect(0, 0, 900, 600));
            let editor = fixture.1;
            set_bounds(editor, crate::layout::rect(0, 0, 800, 400)).unwrap();
            let body = "ERROR_ACCESS_DENIED  next_value(x);";
            set_content(editor, &Content::new(body.into())).unwrap();
            for (index, expected) in [(8, (0, 19)), (25, (21, 31)), (31, (31, 32))] {
                let point = fixture.character_position(index);
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
                assert_eq!(fixture.selection(), expected);
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
                fixture.select(start, start);
                let mut keys = original;
                keys[VK_CONTROL as usize] = 0x80;
                keys[VK_SHIFT as usize] = if shift { 0x80 } else { 0 };
                assert_ne!(SetKeyboardState(keys.as_ptr()), 0);
                send(editor, WM_KEYDOWN, key as usize, 0);
                send(editor, WM_KEYUP, key as usize, 0);
                let actual = fixture.selection();
                assert_ne!(SetKeyboardState(original.as_ptr()), 0);
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn large_declaration_load_keeps_ui_pause_bounded() {
        use std::time::{Duration, Instant};
        use winlex_core::{Architecture, EntityCategory, SearchMode, SearchOptions};

        let catalog = crate::test_support::catalog();
        let entity = crate::test_support::first_match(catalog.search(
            "Windows.Win32.Graphics.Gdi.Apis",
            SearchOptions {
                mode: SearchMode::Name,
                architecture: Some(Architecture::X64),
                categories: Some(EntityCategory::ALL & !(EntityCategory::TypeReference as u32)),
                ..Default::default()
            },
        ));
        let rendered =
            winlex_format::declaration::render(&catalog, entity.id, Architecture::X64).unwrap();
        let body = rendered.text.clone();
        let content = Content::from(rendered);
        let fixture = Editor::new(WS_POPUP | WS_VISIBLE, crate::layout::rect(0, 0, 900, 600));
        let editor = fixture.1;
        set_bounds(editor, crate::layout::rect(0, 0, 750, 500)).unwrap();
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

        let fixture = Editor::new(WS_POPUP | WS_VISIBLE, crate::layout::rect(0, 0, 900, 600));
        let editor = fixture.1;
        set_bounds(editor, crate::layout::rect(0, 0, 750, 500)).unwrap();
        focus(editor);
        fixture.font_size(210);
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
        for (start, end, expected, light, dark) in [
            (0, 7, "// 中文😀", 0x008000, 0x6a9957),
            (8, 13, "const", 0x800000, 0xd69c56),
            (24, 29, "\"中😀\"", 0x000080, 0x789cce),
        ] {
            fixture.select(start, end);
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
            let colored = syntax_enabled();
            assert_eq!(
                format.crTextColor.0,
                if colored {
                    if crate::theme::is_dark() { dark } else { light }
                } else {
                    crate::theme::color(COLOR_WINDOWTEXT)
                }
            );
            assert_eq!(format.yHeight, 210);
        }
        let content = Content::new("const UINT32 value = 42;\n".repeat(500));
        set_content(editor, &content).unwrap();
        fixture.select(6000, 6005);
        send(editor, EM_LINESCROLL, 0, 30);
        let first = send(editor, EM_GETFIRSTVISIBLELINE, 0, 0);
        assert!(first > 0);
        for _ in 0..2 {
            recolor(editor, &content).unwrap();
            assert_eq!(fixture.selection(), (6000, 6005));
            assert_eq!(send(editor, EM_GETFIRSTVISIBLELINE, 0, 0), first);
            assert_eq!(text(editor).replace("\r\n", "\r"), content.text);
        }
    }

    #[test]
    fn visual_wrapping_preserves_declaration_and_paragraph_format() {
        let fixture = Editor::new(WS_POPUP, crate::layout::rect(0, 0, 900, 600));
        let editor = fixture.1;
        let body = "const UINT32 example = 123; // a long declaration with several words that wrap without changing the copied text 😀";
        set_bounds(editor, crate::layout::rect(0, 0, 800, 400)).unwrap();
        set_content(editor, &Content::new(body.into())).unwrap();
        let wide_lines = send(editor, EM_GETLINECOUNT, 0, 0);
        set_bounds(editor, crate::layout::rect(0, 0, 180, 400)).unwrap();
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
    fn dark_rtf_preserves_text_escapes_and_assigns_explicit_default_color() {
        let content = Content::new("const char *s = \"\\cf0 中文😀\"; // comment\n42".into());
        let light = String::from_utf8(content.rtf.clone()).unwrap();
        let dark = String::from_utf8(rtf::themed(&content.rtf, true).into_owned()).unwrap();
        assert_eq!(
            light.split_once(r"\pard\f0 ").unwrap().1,
            dark.split_once(r"\pard\f0\cf0 ").unwrap().1,
        );
        let default = crate::theme::DARK_CONTENT_TEXT;
        assert!(dark.contains(&format!(
            r"{{\colortbl\red{}\green{}\blue{};",
            default & 255,
            default >> 8 & 255,
            default >> 16 & 255
        )));
        crate::theme::set_mode(2);
        if crate::theme::is_dark() {
            assert_eq!(crate::theme::color(COLOR_WINDOWTEXT), default);
        }
        crate::theme::set_mode(0);
        let fixture = Editor::new(WS_POPUP, crate::layout::rect(0, 0, 900, 600));
        stream_in(fixture.1, dark.as_bytes()).unwrap();
        assert_eq!(text(fixture.1).replace("\r\n", "\r"), content.text);
        for (start, end, color) in [(0, 5, 0xd69c56), (5, 6, default)] {
            fixture.select(start, end);
            let mut format = CHARFORMATW {
                cbSize: size_of::<CHARFORMATW>() as u32,
                ..Default::default()
            };
            send_mut(
                fixture.1,
                EM_GETCHARFORMAT,
                SCF_SELECTION as usize,
                &mut format,
            );
            assert_eq!(format.crTextColor.0, color);
        }
    }

    #[test]
    fn highlighting_keeps_numeric_identifier_and_comment_boundaries() {
        for (body, expected) in [
            ("int1 _int UINT32_ 0xAf", vec![("0xAf", NUMBER_STYLE)]),
            ("/*/ int", vec![("/*/ int", COMMENT_STYLE)]),
            (
                "/**/const",
                vec![("/**/", COMMENT_STYLE), ("const", KEYWORD_STYLE)],
            ),
            (
                "/* **/42",
                vec![("/* **/", COMMENT_STYLE), ("42", NUMBER_STYLE)],
            ),
            (
                "/*\r😀*/struct",
                vec![("/*\r😀*/", COMMENT_STYLE), ("struct", KEYWORD_STYLE)],
            ),
            (
                "\"int 123\" // UINT32\rconst",
                vec![
                    ("\"int 123\"", STRING_STYLE),
                    ("// UINT32", COMMENT_STYLE),
                    ("const", KEYWORD_STYLE),
                ],
            ),
        ] {
            let text: Vec<_> = body.encode_utf16().collect();
            let found: Vec<_> = spans(&text, &[])
                .into_iter()
                .map(|(start, end, style)| (String::from_utf16(&text[start..end]).unwrap(), style))
                .collect();
            let expected: Vec<_> = expected
                .into_iter()
                .map(|(token, style)| (token.to_owned(), style))
                .collect();
            assert_eq!(found, expected, "{body}");
        }
        assert_eq!(
            spans(&[b'1' as u16, 0xd800, b'2' as u16, 0xdc00], &[]),
            [(0, 1, NUMBER_STYLE), (2, 3, NUMBER_STYLE)]
        );
    }

    #[test]
    fn highlighting_uses_utf16_and_keeps_comments_and_strings_whole() {
        let text: Vec<_> =
            "// 😀 struct 42\rconst char *s = \"a\\\"b\"; /* int 7 */ 123 UINT32 UINT_PTR char16_t"
                .encode_utf16()
                .collect();
        let tokens: Vec<_> = super::spans(&text, &[])
            .into_iter()
            .map(|(start, end, _)| String::from_utf16(&text[start..end]).unwrap())
            .collect();
        assert_eq!(
            tokens,
            [
                "// 😀 struct 42",
                "const",
                "\"a\\\"b\"",
                "/* int 7 */",
                "123",
            ]
        );
    }
}
