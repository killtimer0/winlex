//! RichEdit 2.0-compatible declaration presentation. Offsets are UTF-16 positions.
use crate::native::*;
use anyhow::{Context, ensure};
use std::{mem::size_of, sync::OnceLock};
use windows::Win32::UI::Controls::RichEdit::{
    CFM_BOLD, CFM_CHARSET, CFM_COLOR, CFM_FACE, CFM_ITALIC, CFM_SIZE, CFM_UNDERLINE, CHARFORMAT2W,
    CHARFORMATW, CHARRANGE, EDITSTREAM, EM_EXGETSEL, EM_EXSETSEL, EM_GETCHARFORMAT,
    EM_GETOLEINTERFACE, EM_GETSELTEXT, EM_GETZOOM, EM_SETBKGNDCOLOR, EM_SETCHARFORMAT,
    EM_SETPARAFORMAT, EM_SETSCROLLPOS, EM_SETZOOM, EM_STREAMIN, IRichEditOle, ITextDocument,
    PARAFORMAT, PARAFORMAT2, PFM_LINESPACING, PFM_RIGHTINDENT, PFM_STARTINDENT, SCF_ALL, SF_RTF,
};
use windows_core::Interface;
use winlex_core::DeclarationTarget;
use winlex_format::declaration::{Region, RegionKind, Rendered, SymbolRole};

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
const TYPE_STYLE: u8 = 5;
const METHOD_STYLE: u8 = 6;
const MEMBER_STYLE: u8 = 7;

fn role_style(role: SymbolRole) -> u8 {
    match role {
        SymbolRole::Type => TYPE_STYLE,
        SymbolRole::Method => METHOD_STYLE,
        SymbolRole::Member => MEMBER_STYLE,
    }
}

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
        Rendered::from(text).into()
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
        !matches!(region.kind, RegionKind::Symbol)
            && range.start <= range.end
            && region.range.start <= range.start
            && range.end <= region.range.end
            && (range.start < region.range.end || region.number().is_some())
    })
}

/// The nearest insertion position; the right half of a glyph can return its end.
fn char_index(hwnd: HWND, point: &POINT) -> Option<usize> {
    let index = send_ref(hwnd, EM_CHARFROMPOS, 0, point) as i32;
    (index >= 0).then_some(index as usize)
}

pub fn region_at_point(hwnd: HWND, regions: &[Region], point: POINT) -> Option<&Region> {
    use windows::Win32::UI::Controls::RichEdit::{
        tomAllowOffClient, tomClientCoord, tomConstants, tomStart,
    };

    if unsafe { PtInRect(&client(hwnd), point) } == 0 {
        return None;
    }
    let index = char_index(hwnd, &point)?;
    let candidates = [Some(index), index.checked_sub(1)];
    let mut document = None;
    for character in candidates.into_iter().flatten() {
        let Some(region) =
            region_at(regions, character..character).filter(|region| region.target().is_some())
        else {
            continue;
        };
        // Measure an independent character range, never the user's selection.
        // Checking the rectangle also rejects adjacent whitespace and other lines.
        if document.is_none() {
            let mut ole = None::<IRichEditOle>;
            send(hwnd, EM_GETOLEINTERFACE, 0, &mut ole as *mut _ as isize);
            document = ole.and_then(|ole| ole.cast::<ITextDocument>().ok());
        }
        let Some(document) = document.as_ref() else {
            break;
        };
        let character = i32::try_from(character).ok()?;
        let range = unsafe { document.Range(character, character.checked_add(1)?) }.ok()?;
        let mut area = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let flags = tomStart.0 | tomClientCoord.0 | tomAllowOffClient.0;
        unsafe {
            range
                .GetPoint(tomConstants(flags), &mut area.left, &mut area.top)
                .ok()?;
            range
                .GetPoint(
                    tomConstants(flags | TA_RIGHT as i32 | TA_BOTTOM as i32),
                    &mut area.right,
                    &mut area.bottom,
                )
                .ok()?;
            if PtInRect(&area, point) != 0 {
                return Some(region);
            }
        }
    }
    // Numeric caret boundaries retain their existing inclusive-end contract.
    region_at(regions, index..index).filter(|region| region.number().is_some())
}

enum ContextAction {
    Copy(String),
    Navigate(DeclarationTarget),
}

/// Capture context before TrackPopupMenu enters its nested message loop.
pub fn context_menu(
    hwnd: HWND,
    location: isize,
    regions: &[Region],
) -> anyhow::Result<Option<DeclarationTarget>> {
    use crate::resources::{
        self, IDS_COPY_DECIMAL, IDS_COPY_HEX, IDS_COPY_SELECTION, IDS_COPY_VALUE,
        IDS_GO_TO_DECLARATION, IDS_VIEW_GENERIC_DEFINITION,
    };
    let mut selection = CHARRANGE::default();
    send_mut(hwnd, EM_EXGETSEL, 0, &mut selection);
    let mut point = point_from_lparam(location);
    let mut mouse_region = None;
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
        let index = char_index(hwnd, &local);
        mouse_region = region_at_point(hwnd, regions, local);
        let selected_link = mouse_region.is_some_and(|region| {
            selection.cpMin as usize <= region.range.start
                && region.range.end <= selection.cpMax as usize
        });
        if let Some(index) = index
            && !(selection.cpMin..selection.cpMax).contains(&(index as i32))
            && !selected_link
        {
            selection = CHARRANGE {
                cpMin: index as i32,
                cpMax: index as i32,
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
            RegionKind::Symbol => {}
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
                    Some(ContextAction::Navigate(target.clone())),
                ));
                if !target.arguments.is_empty() {
                    actions.push((
                        IDS_VIEW_GENERIC_DEFINITION,
                        Some(ContextAction::Navigate(target.id.into())),
                    ));
                }
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
                ContextAction::Navigate(target) => return Ok(Some(target.clone())),
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
            WM_CONTEXTMENU | WM_KEYDOWN | WM_KEYUP => {
                if let Some(result) = forward_context_menu(hwnd, msg, w, l) {
                    return result;
                }
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
        // Plain replacement inherits the control default, not the imported body font.
        let plain = CHARFORMAT2W {
            Base: CHARFORMATW {
                cbSize: size_of::<CHARFORMAT2W>() as u32,
                dwMask: CFM_COLOR | CFM_UNDERLINE | CFM_BOLD | CFM_ITALIC | CFM_CHARSET,
                bCharSet: windows::Win32::Graphics::Gdi::DEFAULT_CHARSET,
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
    let range = CHARRANGE::default();
    send_ref(hwnd, EM_EXSETSEL, 0, &range);
    result
}

fn syntax_enabled() -> bool {
    !high_contrast() && (crate::theme::is_dark() || crate::theme::color(COLOR_WINDOW) == 0x00ffffff)
}

/// Toggle underline on one symbol without changing its semantic color.
/// An independent TOM range leaves the user's selection and reading position alone.
pub fn set_underline(
    hwnd: HWND,
    range: &std::ops::Range<usize>,
    enabled: bool,
) -> anyhow::Result<()> {
    use windows::Win32::UI::Controls::RichEdit::{tomNone, tomSingle};

    ensure!(range.start < range.end, "Empty underline range");
    let start = i32::try_from(range.start)?;
    let end = i32::try_from(range.end)?;
    let mut ole = None::<IRichEditOle>;
    send(hwnd, EM_GETOLEINTERFACE, 0, &mut ole as *mut _ as isize);
    let document = ole
        .context("Get declaration RichEdit object")?
        .cast::<ITextDocument>()?;
    let font = unsafe { document.Range(start, end)?.GetFont()? };
    // TOM rejects font changes in read-only controls. Only this synchronous
    // formatting operation is writable; restore read-only before returning errors.
    let readonly = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } & ES_READONLY != 0;
    if readonly {
        ensure!(
            send(hwnd, EM_SETREADONLY, 0, 0) != 0,
            "Unlock declaration formatting failed"
        );
    }
    let result = unsafe { font.SetUnderline(if enabled { tomSingle.0 } else { tomNone.0 }) };
    if readonly {
        ensure!(
            send(hwnd, EM_SETREADONLY, 1, 0) != 0,
            "Restore read-only declaration failed"
        );
    }
    result.context("Set declaration underline failed")
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
    let mut roles: Vec<_> = regions
        .iter()
        .filter_map(|region| {
            let role = region.role?;
            (region.range.start < region.range.end && region.range.end <= text.len())
                .then_some((region.range.clone(), role_style(role)))
        })
        .collect();
    roles.sort_by_key(|(range, _)| range.start);
    let mut roles = roles.into_iter().peekable();
    let mut i = 0;
    while i < text.len() {
        while roles.peek().is_some_and(|(range, _)| range.start < i) {
            roles.next();
        }
        if let Some((range, style)) = roles.peek().filter(|(range, _)| range.start == i) {
            result.push((i, range.end, *style));
            i = range.end;
            roles.next();
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
                    "static",
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
