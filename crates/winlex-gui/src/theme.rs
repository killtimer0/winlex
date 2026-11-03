//! UI-thread theme state. Modern entry points are optional, including on XP.
use crate::native::*;
use crate::platform::WindowsVersion;
use std::{
    cell::{Cell, RefCell},
    ptr,
};
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::{Graphics::Dwm::DwmSetWindowAttribute, UI::Shell::GetWindowSubclass};

const SUBCLASS: usize = 0x574c5448;
const COLORS: usize = 31;

#[derive(Default)]
struct State {
    mode: u32,
    dark: bool,
    contrast: bool,
    brushes: Vec<(COLORREF, GdiObject)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    static APIS: Option<Apis> = Apis::load();
    static APPLYING: Cell<bool> = const { Cell::new(false) };
}

type AllowDark = unsafe extern "system" fn(HWND, bool) -> bool;
type PreferredAppMode = unsafe extern "system" fn(i32) -> i32;
type FlushMenuThemes = unsafe extern "system" fn();

struct Apis {
    allow_dark: AllowDark,
    preferred_app_mode: PreferredAppMode,
    flush_menu_themes: FlushMenuThemes,
    dark_theme: bool,
}

impl Apis {
    fn load() -> Option<Self> {
        let version = WindowsVersion::query().ok()?;
        if !version.at_least(WindowsVersion::DARK_MODE) {
            return None;
        }
        unsafe {
            // Existing XP-era theme imports keep the system module loaded.
            let theme = GetModuleHandleW(w!("uxtheme.dll"));
            // This ordinal was unrelated on older Windows; gate it on the real build.
            let allow_dark: AllowDark =
                std::mem::transmute(GetProcAddress(theme, 133usize as *const u8)?);
            let preferred_app_mode: PreferredAppMode =
                std::mem::transmute(GetProcAddress(theme, 135usize as *const u8)?);
            let flush_menu_themes: FlushMenuThemes =
                std::mem::transmute(GetProcAddress(theme, 136usize as *const u8)?);
            Some(Self {
                allow_dark,
                preferred_app_mode,
                flush_menu_themes,
                dark_theme: version.at_least(WindowsVersion::WINDOWS_11_25H2),
            })
        }
    }
}

pub fn supported() -> bool {
    APIS.with(Option::is_some)
}

fn native_dark_theme() -> bool {
    APIS.with(|apis| apis.as_ref().is_some_and(|apis| apis.dark_theme))
}

fn system_dark() -> bool {
    unsafe {
        let mut value = 1u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        ) == 0
            && size == 4
            && value == 0
    }
}

fn effective_dark(mode: u32, system: bool, contrast: bool, supported: bool) -> bool {
    supported
        && !contrast
        && match mode {
            1 => false,
            2 => true,
            _ => system,
        }
}

/// Updates every installed window tree; does not persist the preference.
/// Call before creating GUI windows. Unknown modes use the system preference.
pub fn set_mode(mode: u32) {
    STATE.with(|state| state.borrow_mut().mode = if mode <= 2 { mode } else { 0 });
    refresh();
}

/// Re-reads accessibility and system preference. System-color notifications
/// already reapply each receiving root; only effective-mode changes fan out.
pub fn refresh() -> bool {
    if !supported() {
        return false;
    }
    let contrast = high_contrast();
    let system_dark = system_dark();
    let changed = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let dark = effective_dark(state.mode, system_dark, contrast, supported());
        let changed = state.dark != dark || state.contrast != contrast;
        state.dark = dark;
        state.contrast = contrast;
        changed
    });
    if changed {
        APIS.with(|apis| unsafe {
            if let Some(apis) = apis {
                // Default in high contrast; otherwise force the effective choice
                // so an explicit preference does not follow the OS popup palette.
                (apis.preferred_app_mode)(if contrast {
                    0
                } else if is_dark() {
                    2
                } else {
                    3
                });
                (apis.flush_menu_themes)();
            }
        });
        unsafe extern "system" fn refresh_root(hwnd: HWND, _: isize) -> i32 {
            let mut data = 0;
            // The subclass is the registration: enumerate each installed root
            // once, including owned dialogs, without a second HWND registry.
            if unsafe { GetWindowSubclass(hwnd, Some(subclass), SUBCLASS, &mut data) } != 0 {
                apply(hwnd);
            }
            1
        }
        unsafe {
            EnumThreadWindows(
                windows_sys::Win32::System::Threading::GetCurrentThreadId(),
                Some(refresh_root),
                0,
            );
        }
    }
    changed
}

pub fn is_dark() -> bool {
    STATE.with(|state| state.borrow().dark)
}

fn dark_color(index: i32) -> COLORREF {
    match index {
        COLOR_WINDOW | COLOR_INFOBK => 0x00202020,
        COLOR_WINDOWTEXT | COLOR_BTNTEXT | COLOR_MENUTEXT | COLOR_INFOTEXT | COLOR_CAPTIONTEXT
        | COLOR_HIGHLIGHTTEXT => 0x00eeeeee,
        COLOR_GRAYTEXT | COLOR_INACTIVECAPTIONTEXT => 0x00999999,
        COLOR_HIGHLIGHT => 0x00b86b26,
        COLOR_HOTLIGHT => 0x00ffb469,
        COLOR_3DSHADOW | COLOR_WINDOWFRAME | COLOR_ACTIVEBORDER | COLOR_INACTIVEBORDER => {
            0x00484848
        }
        COLOR_3DHIGHLIGHT | COLOR_3DLIGHT => 0x00555555,
        COLOR_3DDKSHADOW => 0x00181818,
        _ => 0x002b2b2b,
    }
}

pub fn color(index: i32) -> COLORREF {
    if is_dark() {
        dark_color(index)
    } else {
        unsafe { GetSysColor(index) }
    }
}

/// Borrowed brush: never delete it. Dark brushes are retained across switches so
/// a nested paint cannot invalidate a brush still in use by a native control.
pub fn brush(index: i32) -> HBRUSH {
    if !is_dark() {
        return unsafe { GetSysColorBrush(index) };
    }
    let rgb = dark_color(index);
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if let Some((_, brush)) = state.brushes.iter().find(|(value, _)| *value == rgb) {
            return brush.0;
        }
        let handle = unsafe { CreateSolidBrush(rgb) };
        if !handle.is_null() {
            state.brushes.push((rgb, GdiObject(handle)));
        }
        handle
    })
}

fn class_name(hwnd: HWND) -> String {
    let mut name = [0u16; 128];
    let count = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
    String::from_utf16_lossy(&name[..count.max(0) as usize]).to_ascii_lowercase()
}

/// Themes and installs message handling on a window and its descendants.
pub fn apply(hwnd: HWND) {
    if !supported() || hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
        return;
    }
    if APPLYING.with(|applying| applying.replace(true)) {
        return;
    }
    unsafe extern "system" fn child(hwnd: HWND, _: isize) -> i32 {
        apply_one(hwnd);
        1
    }
    apply_one(hwnd);
    unsafe {
        EnumChildWindows(hwnd, Some(child), 0);
        RedrawWindow(
            hwnd,
            ptr::null(),
            ptr::null_mut(),
            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_FRAME,
        );
        if !GetMenu(hwnd).is_null() {
            DrawMenuBar(hwnd);
        }
    }
    APPLYING.with(|applying| applying.set(false));
}

fn apply_one(hwnd: HWND) {
    let dark = is_dark();
    let class = class_name(hwnd);
    unsafe {
        set_subclass(hwnd, Some(subclass), SUBCLASS, 0);
        APIS.with(|apis| {
            let Some(apis) = apis else { return };
            (apis.allow_dark)(hwnd, dark);
            // Application classes paint their own palette. Setting a native theme
            // on the frame would synchronously reenter its recolor handler and
            // stream every open declaration again during the same theme switch.
            if !class.starts_with("winlex") {
                let name = if dark {
                    if apis.dark_theme {
                        w!("DarkMode_DarkTheme")
                    } else if class == "syslistview32" {
                        w!("Explorer")
                    } else if class == "combobox" {
                        w!("DarkMode_CFD")
                    } else {
                        w!("DarkMode_Explorer")
                    }
                } else if class == "syslistview32" && !high_contrast() {
                    w!("Explorer")
                } else {
                    ptr::null()
                };
                SetWindowTheme(hwnd, name, ptr::null());
            }
            if GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CHILD == 0 {
                let enabled = i32::from(dark);
                let data = (&enabled as *const i32).cast();
                DwmSetWindowAttribute(hwnd, 20, data, 4);
            }
        });
        if class == "syslistview32" {
            send(hwnd, LVM_SETBKCOLOR, 0, color(COLOR_WINDOW) as isize);
            send(hwnd, LVM_SETTEXTBKCOLOR, 0, color(COLOR_WINDOW) as isize);
            send(hwnd, LVM_SETTEXTCOLOR, 0, color(COLOR_WINDOWTEXT) as isize);
            let tooltip = send(hwnd, LVM_GETTOOLTIPS, 0, 0) as HWND;
            if !tooltip.is_null() {
                apply_one(tooltip);
            }
        }
    }
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    w: usize,
    l: isize,
    _: usize,
    _: usize,
) -> isize {
    unsafe {
        if msg == WM_NCDESTROY {
            remove_subclass(hwnd, Some(subclass), SUBCLASS);
            return default_subclass(hwnd, msg, w, l);
        }
        if matches!(msg, WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED) {
            // Let controls discard their cached theme before applying our colors.
            let result = default_subclass(hwnd, msg, w, l);
            let applying = APPLYING.with(Cell::get);
            let root = GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CHILD == 0;
            if !applying && root && !refresh() {
                apply(hwnd);
            }
            return result;
        }
        if is_dark() {
            match msg {
                WM_NOTIFY if l != 0 => {
                    let notification = &*(l as *const NMHDR);
                    if notification.code == NM_CUSTOMDRAW {
                        let result = default_subclass(hwnd, msg, w, l);
                        if result & CDRF_SKIPDEFAULT as isize != 0 {
                            return result;
                        }
                        let draw = &*(l as *const NMCUSTOMDRAW);
                        return result
                            | match class_name(notification.hwndFrom).as_str() {
                                "button" if !native_dark_theme() => checkbox_text(draw),
                                "toolbarwindow32" => {
                                    paint_toolbar(&mut *(l as *mut NMTBCUSTOMDRAW))
                                }
                                "syslink" if draw.dwDrawStage == CDDS_PREPAINT => {
                                    CDRF_NOTIFYITEMDRAW as isize
                                }
                                "syslink" if draw.dwDrawStage == CDDS_ITEMPREPAINT => {
                                    SetTextColor(
                                        draw.hdc,
                                        color(if draw.uItemState & CDIS_DISABLED != 0 {
                                            COLOR_GRAYTEXT
                                        } else {
                                            COLOR_HOTLIGHT
                                        }),
                                    );
                                    SetBkColor(draw.hdc, color(COLOR_BTNFACE));
                                    CDRF_NEWFONT as isize
                                }
                                _ => 0,
                            };
                    }
                }
                0x91 | 0x92 => {
                    if let Some(result) = paint_menu(hwnd, msg, l) {
                        return result;
                    }
                }
                WM_CTLCOLORBTN | WM_CTLCOLORDLG | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX
                | WM_CTLCOLORSTATIC | WM_CTLCOLORSCROLLBAR => {
                    let dc = w as HDC;
                    let original = default_subclass(hwnd, msg, w, l);
                    // Preserve owner-provided backgrounds (for example a preview
                    // swatch); only replace the standard system-color path.
                    if original != 0
                        && !(0..COLORS as i32)
                            .any(|index| original == GetSysColorBrush(index) as isize)
                    {
                        return original;
                    }
                    let edit = matches!(msg, WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX)
                        || (msg == WM_CTLCOLORSTATIC && class_name(l as HWND) == "edit");
                    let background = if edit { COLOR_WINDOW } else { COLOR_BTNFACE };
                    let foreground = GetTextColor(dc);
                    if foreground == GetSysColor(COLOR_WINDOWTEXT)
                        || foreground == GetSysColor(COLOR_BTNTEXT)
                        || foreground == GetSysColor(COLOR_GRAYTEXT)
                    {
                        SetTextColor(
                            dc,
                            color(if IsWindowEnabled(l as HWND) == 0 {
                                COLOR_GRAYTEXT
                            } else {
                                COLOR_WINDOWTEXT
                            }),
                        );
                    }
                    SetBkColor(dc, color(background));
                    return brush(background) as isize;
                }
                WM_ERASEBKGND => {
                    let class = class_name(hwnd);
                    // Native controls have their own paint contracts. Only replace
                    // container erasure, never list/edit/tree scrolling erasure.
                    if class == "#32770"
                        || class == "mdiclient"
                        || class.starts_with("winlex")
                        || (class == "systabcontrol32" && can_paint_tabs(hwnd))
                    {
                        FillRect(w as HDC, &client(hwnd), brush(COLOR_BTNFACE));
                        return 1;
                    }
                }
                WM_PAINT | WM_PRINTCLIENT => {
                    let painter: Option<unsafe fn(HWND, HDC)> = match class_name(hwnd).as_str() {
                        "systabcontrol32" if can_paint_tabs(hwnd) => Some(paint_tabs),
                        "msctls_statusbar32" if !native_dark_theme() && can_paint_status(hwnd) => {
                            Some(paint_status)
                        }
                        _ => None,
                    };
                    if let Some(painter) = painter {
                        let mut paint: PAINTSTRUCT = std::mem::zeroed();
                        let dc = if msg == WM_PAINT {
                            BeginPaint(hwnd, &mut paint)
                        } else {
                            w as HDC
                        };
                        if !dc.is_null() {
                            let saved = SaveDC(dc);
                            control_font(hwnd, dc);
                            painter(hwnd, dc);
                            if saved != 0 {
                                RestoreDC(dc, saved);
                            }
                        }
                        if msg == WM_PAINT {
                            EndPaint(hwnd, &paint);
                        }
                        return 0;
                    }
                }
                _ => {}
            }
        }
        default_subclass(hwnd, msg, w, l)
    }
}

unsafe fn checkbox_text(draw: &NMCUSTOMDRAW) -> isize {
    unsafe {
        // The themed button has already drawn its glyph at PREPAINT.
        if draw.dwDrawStage != CDDS_PREPAINT
            || GetWindowLongW(draw.hdr.hwndFrom, GWL_STYLE) as u32 & BS_TYPEMASK as u32
                != BS_AUTOCHECKBOX as u32
        {
            return 0;
        }
        let hwnd = draw.hdr.hwndFrom;
        let dc = draw.hdc;
        let theme = GetWindowTheme(hwnd);
        let saved = SaveDC(dc);
        if saved == 0 {
            return 0;
        }
        control_font(hwnd, dc);
        let mut glyph: SIZE = std::mem::zeroed();
        let mut gap: SIZE = std::mem::zeroed();
        let mut area = draw.rc;
        let text = text_wide(hwnd);
        let ui = send(hwnd, WM_QUERYUISTATE, 0, 0) as u32;
        let mut flags = DT_LEFT | DT_VCENTER | DT_SINGLELINE;
        if ui & UISF_HIDEACCEL != 0 {
            flags |= DT_HIDEPREFIX;
        }
        let mut result = 0;
        if GetThemePartSize(
            theme,
            dc,
            BP_CHECKBOX,
            CBS_UNCHECKEDNORMAL,
            ptr::null(),
            TS_DRAW,
            &mut glyph,
        ) >= 0
            && GetTextExtentPoint32W(dc, w!("0"), 1, &mut gap) != 0
        {
            area.left += glyph.cx + gap.cx / 2;
            DrawTextW(dc, text.as_ptr(), -1, &mut area, flags);
            if GetFocus() == hwnd && ui & UISF_HIDEFOCUS == 0 {
                let mut focus = area;
                if DrawTextW(dc, text.as_ptr(), -1, &mut focus, flags | DT_CALCRECT) > 0 {
                    let height = focus.bottom - focus.top;
                    focus.top = area.top + (area.bottom - area.top - height) / 2;
                    focus.bottom = focus.top + height;
                    InflateRect(&mut focus, 1, 1);
                    let bounds = focus;
                    IntersectRect(&mut focus, &bounds, &draw.rc);
                    DrawFocusRect(dc, &focus);
                }
            }
            result = CDRF_SKIPDEFAULT as isize;
        }
        RestoreDC(dc, saved);
        result
    }
}

unsafe fn paint_toolbar(draw: &mut NMTBCUSTOMDRAW) -> isize {
    unsafe {
        match draw.nmcd.dwDrawStage {
            CDDS_PREPAINT => {
                FillRect(draw.nmcd.hdc, &draw.nmcd.rc, brush(COLOR_BTNFACE));
                CDRF_NOTIFYITEMDRAW as isize
            }
            CDDS_ITEMPREPAINT => {
                let state = draw.nmcd.uItemState;
                let background = if state & CDIS_DISABLED != 0 {
                    COLOR_BTNFACE
                } else if state & CDIS_SELECTED != 0 {
                    COLOR_3DDKSHADOW
                } else if state & CDIS_HOT != 0 {
                    COLOR_3DLIGHT
                } else if state & CDIS_CHECKED != 0 {
                    COLOR_3DSHADOW
                } else {
                    COLOR_BTNFACE
                };
                FillRect(draw.nmcd.hdc, &draw.nmcd.rc, brush(background));
                draw.clrText = color(if state & CDIS_DISABLED != 0 {
                    COLOR_GRAYTEXT
                } else {
                    COLOR_BTNTEXT
                });
                draw.clrTextHighlight = draw.clrText;
                draw.clrBtnFace = color(background);
                draw.clrBtnHighlight = color(background);
                draw.clrHighlightHotTrack = color(background);
                (TBCDRF_USECDCOLORS | TBCDRF_NOBACKGROUND | TBCDRF_NOEDGES | TBCDRF_NOOFFSET)
                    as isize
            }
            _ => CDRF_DODEFAULT as isize,
        }
    }
}

// UAH is deliberately restricted to Windows builds with dark menu support. We
// only read the stable message prefix, leave menu items/data untouched, and let
// native menu tracking, mnemonics, MDI system buttons and accessibility run.
#[repr(C)]
struct UahMenu {
    menu: HMENU,
    dc: HDC,
    flags: u32,
}

#[repr(C)]
struct UahMenuItem {
    draw: DRAWITEMSTRUCT,
    menu: UahMenu,
    position: i32,
}

unsafe fn paint_menu(hwnd: HWND, msg: u32, data: isize) -> Option<isize> {
    unsafe {
        if data == 0 {
            return None;
        }
        if msg == 0x91 {
            let menu = &*(data as *const UahMenu);
            let mut info: MENUBARINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MENUBARINFO>() as u32;
            if GetMenuBarInfo(hwnd, OBJID_MENU, 0, &mut info) == 0 {
                return None;
            }
            let mut window: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut window);
            OffsetRect(&mut info.rcBar, -window.left, -window.top);
            FillRect(menu.dc, &info.rcBar, brush(COLOR_MENUBAR));
        } else {
            let item = &*(data as *const UahMenuItem);
            let mut info: MENUITEMINFOW = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MENUITEMINFOW>() as u32;
            info.fMask = MIIM_STRING | MIIM_FTYPE | MIIM_BITMAP | MIIM_DATA | MIIM_ID;
            if GetMenuItemInfoW(item.menu.menu, item.position as u32, 1, &mut info) == 0
                || (info.fType & MFT_OWNERDRAW != 0 && !mdi_menu_command(&info))
                || (!info.hbmpItem.is_null() && !mdi_menu_bitmap(info.hbmpItem))
            {
                return None;
            }
            let mut label = vec![0u16; info.cch as usize + 1];
            info.dwTypeData = label.as_mut_ptr();
            info.cch = label.len() as u32;
            if GetMenuItemInfoW(item.menu.menu, item.position as u32, 1, &mut info) == 0 {
                return None;
            }
            let selected = item.draw.itemState & (ODS_SELECTED | ODS_HOTLIGHT) != 0;
            let disabled = item.draw.itemState & (ODS_DISABLED | ODS_GRAYED) != 0;
            FillRect(
                item.menu.dc,
                &item.draw.rcItem,
                brush(if selected {
                    COLOR_3DSHADOW
                } else {
                    COLOR_MENUBAR
                }),
            );
            if !info.hbmpItem.is_null() {
                paint_mdi_menu_bitmap(hwnd, item, info.hbmpItem, info.dwItemData as HWND, disabled);
                return Some(0);
            }
            let saved = SaveDC(item.menu.dc);
            SetBkMode(item.menu.dc, TRANSPARENT as i32);
            SetTextColor(
                item.menu.dc,
                color(if disabled {
                    COLOR_GRAYTEXT
                } else {
                    COLOR_MENUTEXT
                }),
            );
            let mut area = item.draw.rcItem;
            let flags = DT_CENTER
                | DT_VCENTER
                | DT_SINGLELINE
                | if item.draw.itemState & ODS_NOACCEL != 0 {
                    DT_HIDEPREFIX
                } else {
                    0
                };
            DrawTextW(item.menu.dc, label.as_ptr(), -1, &mut area, flags);
            if saved != 0 {
                RestoreDC(item.menu.dc, saved);
            }
        }
        Some(0)
    }
}

fn mdi_menu_bitmap(bitmap: HBITMAP) -> bool {
    matches!(bitmap as usize, 1 | 2 | 3 | 5 | 6 | 7)
}

fn mdi_menu_command(info: &MENUITEMINFOW) -> bool {
    // The theme layer marks merged MDI buttons as owner-drawn too. Recognize only
    // their native bitmap/command pairs; unrelated owner-draw items stay native.
    matches!(
        (info.hbmpItem as usize, info.wID),
        (2, SC_RESTORE) | (3 | 7, SC_MINIMIZE) | (5 | 6, SC_CLOSE)
    )
}

unsafe fn paint_mdi_menu_bitmap(
    hwnd: HWND,
    item: &UahMenuItem,
    bitmap: HBITMAP,
    child: HWND,
    disabled: bool,
) {
    unsafe {
        let dc = item.menu.dc;
        let area = item.draw.rcItem;
        if bitmap as usize == 1 {
            use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
            // HBMMENU_SYSTEM carries the child HWND. Document windows install
            // their DPI-sized small icon; use native menu small-icon metrics.
            let dpi = GetDpiForWindow(hwnd);
            let width = GetSystemMetricsForDpi(SM_CXSMICON, dpi);
            let height = GetSystemMetricsForDpi(SM_CYSMICON, dpi);
            let icon = send(child, WM_GETICON, ICON_SMALL as usize, 0) as HICON;
            DrawIconEx(
                dc,
                area.left + (area.right - area.left - width) / 2,
                area.top + (area.bottom - area.top - height) / 2,
                icon,
                width,
                height,
                0,
                ptr::null_mut(),
                DI_NORMAL,
            );
            return;
        }
        let saved = SaveDC(dc);
        if saved == 0 {
            return;
        }
        let foreground = if disabled || matches!(bitmap as usize, 6 | 7) {
            COLOR_GRAYTEXT
        } else {
            COLOR_MENUTEXT
        };
        // user32's caption glyphs use Marlett, not application-drawn strokes.
        // Fit the native symbol cell to the menu's small-icon metrics.
        use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
        let size = GetSystemMetricsForDpi(SM_CYSMICON, GetDpiForWindow(hwnd))
            .min(area.bottom - area.top)
            .min(area.right - area.left);
        let mut font: LOGFONTW = std::mem::zeroed();
        font.lfHeight = size;
        font.lfWeight = FW_NORMAL as i32;
        font.lfCharSet = SYMBOL_CHARSET;
        font.lfFaceName[..7].copy_from_slice(&wide("Marlett")[..7]);
        let font = GdiObject(CreateFontIndirectW(&font));
        SelectObject(dc, font.0);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextAlign(dc, TA_LEFT | TA_TOP);
        SetTextColor(dc, color(foreground));
        let x = area.left + (area.right - area.left - size) / 2;
        let y = area.top + (area.bottom - area.top - size) / 2;
        let glyph = match bitmap as usize {
            3 | 7 => b'0',
            2 => b'2',
            _ => b'r',
        } as u16;
        if !font.0.is_null() {
            TextOutW(dc, x, y, &glyph, 1);
        }
        RestoreDC(dc, saved);
    }
}

unsafe fn control_font(hwnd: HWND, dc: HDC) {
    unsafe {
        let font = send(hwnd, WM_GETFONT, 0, 0) as HFONT;
        SelectObject(
            dc,
            if font.is_null() {
                GetStockObject(DEFAULT_GUI_FONT)
            } else {
                font
            },
        );
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(
            dc,
            color(if IsWindowEnabled(hwnd) == 0 {
                COLOR_GRAYTEXT
            } else {
                COLOR_BTNTEXT
            }),
        );
    }
}

fn can_paint_tabs(hwnd: HWND) -> bool {
    unsafe {
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        // Keep native painting for image lists and nonstandard tab arrangements.
        style & (TCS_OWNERDRAWFIXED | TCS_BUTTONS | TCS_VERTICAL | TCS_BOTTOM) == 0
            && send(hwnd, TCM_GETIMAGELIST, 0, 0) == 0
    }
}

unsafe fn paint_tabs(hwnd: HWND, dc: HDC) {
    unsafe {
        let mut body = client(hwnd);
        FillRect(dc, &body, brush(COLOR_BTNFACE));
        send_mut(hwnd, TCM_ADJUSTRECT, 0, &mut body);
        InflateRect(&mut body, 2, 2);
        FrameRect(dc, &body, brush(COLOR_3DSHADOW));
        let selected = send(hwnd, TCM_GETCURSEL, 0, 0);
        let focused = send(hwnd, TCM_GETCURFOCUS, 0, 0);
        let ui = send(hwnd, WM_QUERYUISTATE, 0, 0) as u32;
        for index in 0..send(hwnd, TCM_GETITEMCOUNT, 0, 0).max(0) as usize {
            let mut area: RECT = std::mem::zeroed();
            if send_mut(hwnd, TCM_GETITEMRECT, index, &mut area) == 0 {
                continue;
            }
            let mut text = [0u16; 1024];
            let mut item = TCITEMW {
                mask: TCIF_TEXT,
                pszText: text.as_mut_ptr(),
                cchTextMax: text.len() as i32,
                ..std::mem::zeroed()
            };
            if send_mut(hwnd, TCM_GETITEMW, index, &mut item) == 0 {
                continue;
            }
            FillRect(
                dc,
                &area,
                brush(if index as isize == selected {
                    COLOR_WINDOW
                } else {
                    COLOR_BTNFACE
                }),
            );
            FrameRect(dc, &area, brush(COLOR_3DSHADOW));
            InflateRect(&mut area, -4, -3);
            DrawTextW(
                dc,
                text.as_ptr(),
                -1,
                &mut area,
                DT_CENTER
                    | DT_VCENTER
                    | DT_SINGLELINE
                    | DT_END_ELLIPSIS
                    | if ui & UISF_HIDEACCEL != 0 {
                        DT_HIDEPREFIX
                    } else {
                        0
                    },
            );
            if index as isize == focused && GetFocus() == hwnd && ui & UISF_HIDEFOCUS == 0 {
                DrawFocusRect(dc, &area);
            }
        }
    }
}

fn can_paint_status(hwnd: HWND) -> bool {
    // Owner-drawn status panes must remain with their original owner.
    let count = send(hwnd, SB_GETPARTS, 0, 0).max(1) as usize;
    !(0..count)
        .any(|index| send(hwnd, SB_GETTEXTLENGTHW, index, 0) as u32 >> 16 & SBT_OWNERDRAW != 0)
}

unsafe fn paint_status(hwnd: HWND, dc: HDC) {
    unsafe {
        let count = send(hwnd, SB_GETPARTS, 0, 0).max(1) as usize;
        FillRect(dc, &client(hwnd), brush(COLOR_BTNFACE));
        let simple = send(hwnd, SB_ISSIMPLE, 0, 0) != 0;
        for part in 0..if simple { 1 } else { count } {
            let index = if simple { SB_SIMPLEID as usize } else { part };
            let length = send(hwnd, SB_GETTEXTLENGTHW, index, 0) as usize & 0xffff;
            let mut text = vec![0u16; length + 1];
            send(hwnd, SB_GETTEXTW, index, text.as_mut_ptr() as isize);
            let mut area = client(hwnd);
            if !simple {
                send_mut(hwnd, SB_GETRECT, index, &mut area);
            }
            area.left += 4;
            area.right -= 4;
            DrawTextW(
                dc,
                text.as_ptr(),
                length as i32,
                &mut area,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test executables do not embed the GUI's common-controls manifest. Activate
    // that same manifest so pixel contracts exercise the actual v6 controls.
    #[repr(C)]
    struct ActCtx {
        size: u32,
        flags: u32,
        source: *const u16,
        processor: u16,
        language: u16,
        directory: *const u16,
        resource: *const u16,
        application: *const u16,
        module: HMODULE,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateActCtxW(context: *const ActCtx) -> HANDLE;
        fn ActivateActCtx(context: HANDLE, cookie: *mut usize) -> i32;
        fn DeactivateActCtx(flags: u32, cookie: usize) -> i32;
        fn ReleaseActCtx(context: HANDLE);
    }

    struct CommonControls(HANDLE, usize, HMODULE);
    impl CommonControls {
        fn activate() -> Self {
            use std::os::windows::ffi::OsStrExt;
            let manifest = std::env::var_os("WINLEX_TEST_GUI_MANIFEST")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("res/gui.manifest")
                });
            let source: Vec<_> = manifest.as_os_str().encode_wide().chain(Some(0)).collect();
            unsafe {
                let context = ActCtx {
                    size: std::mem::size_of::<ActCtx>() as u32,
                    source: source.as_ptr(),
                    ..std::mem::zeroed()
                };
                let handle = CreateActCtxW(&context);
                assert_ne!(handle, INVALID_HANDLE_VALUE);
                let mut cookie = 0;
                assert_ne!(ActivateActCtx(handle, &mut cookie), 0);
                let module = LoadLibraryW(w!("comctl32.dll"));
                assert!(!module.is_null());
                Self(handle, cookie, module)
            }
        }
        fn initialize(&self, classes: u32) {
            unsafe {
                let proc = GetProcAddress(self.2, c"InitCommonControlsEx".as_ptr().cast()).unwrap();
                let initialize: unsafe extern "system" fn(*const INITCOMMONCONTROLSEX) -> i32 =
                    std::mem::transmute(proc);
                let controls = INITCOMMONCONTROLSEX {
                    dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                    dwICC: classes,
                };
                assert_ne!(initialize(&controls), 0);
            }
        }
    }
    impl Drop for CommonControls {
        fn drop(&mut self) {
            unsafe {
                FreeLibrary(self.2);
                DeactivateActCtx(0, self.1);
                ReleaseActCtx(self.0);
            }
        }
    }

    #[test]
    fn accessibility_overrides_every_requested_mode() {
        for mode in [0, 1, 2, 99] {
            for system in [false, true] {
                assert!(!effective_dark(mode, system, true, true));
                for contrast in [false, true] {
                    assert!(!effective_dark(mode, system, contrast, false));
                }
            }
        }
        assert!(effective_dark(0, true, false, true));
        assert!(!effective_dark(0, false, false, true));
        assert!(!effective_dark(1, true, false, true));
        assert!(effective_dark(2, false, false, true));
        assert_eq!(
            effective_dark(99, true, false, true),
            effective_dark(0, true, false, true)
        );
    }

    #[test]
    fn dark_palette_keeps_text_and_surfaces_distinct() {
        for (foreground, background) in [
            (COLOR_WINDOWTEXT, COLOR_WINDOW),
            (COLOR_BTNTEXT, COLOR_BTNFACE),
            (COLOR_INFOTEXT, COLOR_INFOBK),
            (COLOR_HIGHLIGHTTEXT, COLOR_HIGHLIGHT),
        ] {
            assert_ne!(dark_color(foreground), dark_color(background));
        }
        assert_eq!(dark_color(COLOR_WINDOWTEXT), dark_color(COLOR_BTNTEXT));
        assert_ne!(dark_color(COLOR_GRAYTEXT), dark_color(COLOR_WINDOWTEXT));
    }

    #[test]
    fn applying_and_switching_updates_existing_native_controls() {
        if !supported() {
            set_mode(2);
            assert!(!is_dark());
            return;
        }
        let controls = CommonControls::activate();
        controls.initialize(ICC_LISTVIEW_CLASSES | ICC_BAR_CLASSES);
        unsafe {
            let parent = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                RECT {
                    left: 0,
                    top: 0,
                    right: 400,
                    bottom: 300,
                },
            );
            let list = child(parent, w!("SysListView32"), &[0], LVS_REPORT, 1).unwrap();
            let edit = child(parent, w!("EDIT"), &[0], 0, 3).unwrap();
            set_mode(2);
            // Applying twice must neither recurse through WM_THEMECHANGED nor
            // accumulate subclasses with different identities.
            apply(parent);
            apply(parent);
            assert_eq!(is_dark(), !high_contrast());
            assert_eq!(send(list, LVM_GETBKCOLOR, 0, 0) as u32, color(COLOR_WINDOW));
            assert_eq!(
                send(list, LVM_GETTEXTCOLOR, 0, 0) as u32,
                color(COLOR_WINDOWTEXT)
            );
            let tooltip = send(list, LVM_GETTOOLTIPS, 0, 0) as HWND;
            assert!(!tooltip.is_null());
            // Themed tooltip pixels are covered by the real-input GUI contract;
            // TTM_GETTIP* colors do not describe visual-style rendering.
            set_text(edit, "Retained text");
            set_mode(1);
            assert!(!is_dark());
            assert_eq!(
                send(list, LVM_GETBKCOLOR, 0, 0) as u32,
                GetSysColor(COLOR_WINDOW)
            );
            assert_eq!(text(edit), "Retained text");
            assert_eq!(send(list, LVM_GETTOOLTIPS, 0, 0) as HWND, tooltip);
            assert_ne!(DestroyWindow(parent), 0);
            set_mode(2);
            set_mode(0);
        }
    }

    #[test]
    fn theme_switches_preserve_native_control_input_and_status_contracts() {
        if !supported() {
            return;
        }
        let _common_controls = CommonControls::activate();
        _common_controls.initialize(ICC_TAB_CLASSES | ICC_BAR_CLASSES);
        unsafe {
            let controls = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_TAB_CLASSES | ICC_BAR_CLASSES,
            };
            assert_ne!(InitCommonControlsEx(&controls), 0);
            let parent = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                RECT {
                    left: 0,
                    top: 0,
                    right: 400,
                    bottom: 300,
                },
            );
            let checkbox = child(
                parent,
                w!("BUTTON"),
                &wide("&Check"),
                BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                1,
            )
            .unwrap();
            let button = child(
                parent,
                w!("BUTTON"),
                &wide("&Apply"),
                BS_DEFPUSHBUTTON as u32 | WS_TABSTOP,
                3,
            )
            .unwrap();
            place(
                button,
                RECT {
                    left: 200,
                    top: 0,
                    right: 300,
                    bottom: 30,
                },
            );
            let status = child(parent, w!("msctls_statusbar32"), &[0], 0, 4).unwrap();
            let parts = [150i32, -1];
            assert_ne!(
                send(status, SB_SETPARTS, parts.len(), parts.as_ptr() as isize),
                0
            );
            assert_ne!(
                send(status, SB_SETTEXTW, 0, wide("Ready").as_ptr() as isize),
                0
            );
            place(
                checkbox,
                RECT {
                    left: 0,
                    top: 0,
                    right: 180,
                    bottom: 30,
                },
            );
            let tabs = child(parent, w!("SysTabControl32"), &[0], WS_TABSTOP, 2).unwrap();
            place(
                tabs,
                RECT {
                    left: 0,
                    top: 40,
                    right: 200,
                    bottom: 160,
                },
            );
            for (index, title) in ["First", "Second"].iter().enumerate() {
                let mut label = wide(title);
                let item = TCITEMW {
                    mask: TCIF_TEXT,
                    pszText: label.as_mut_ptr(),
                    ..std::mem::zeroed()
                };
                assert_eq!(
                    send_ref(tabs, TCM_INSERTITEMW, index, &item),
                    index as isize
                );
            }
            send(tabs, TCM_SETCURSEL, 1, 0);
            set_mode(2);
            apply(parent);
            ShowWindow(parent, SW_SHOW);
            for mode in [2, 1] {
                set_mode(mode);
                assert_eq!(
                    GetWindowLongW(checkbox, GWL_STYLE) as u32 & BS_TYPEMASK as u32,
                    BS_AUTOCHECKBOX as u32
                );
                assert_eq!(
                    GetWindowLongW(button, GWL_STYLE) as u32 & BS_TYPEMASK as u32,
                    BS_DEFPUSHBUTTON as u32
                );
                assert_eq!(
                    GetWindowLongW(tabs, GWL_STYLE) as u32 & TCS_OWNERDRAWFIXED,
                    0
                );
                SetFocus(checkbox);
                assert_eq!(GetFocus(), checkbox);
                send(checkbox, WM_KEYDOWN, VK_SPACE as usize, 0);
                send(checkbox, WM_KEYUP, VK_SPACE as usize, 0);
                assert_eq!(send(checkbox, BM_GETCHECK, 0, 0) as u32, BST_CHECKED);
                EnableWindow(checkbox, 0);
                EnableWindow(button, 0);
                set_mode(if mode == 2 { 1 } else { 2 });
                assert_eq!(IsWindowEnabled(checkbox), 0);
                assert_eq!(IsWindowEnabled(button), 0);
                assert_eq!(send(checkbox, BM_GETCHECK, 0, 0) as u32, BST_CHECKED);
                EnableWindow(checkbox, 1);
                EnableWindow(button, 1);
                SetFocus(button);
                set_mode(mode);
                assert_eq!(GetFocus(), button);
                assert_ne!(send(button, BM_GETSTATE, 0, 0) as u32 & BST_FOCUS, 0);
                send(button, BM_SETSTATE, 1, 0);
                assert_ne!(send(button, BM_GETSTATE, 0, 0) as u32 & BST_PUSHED, 0);
                send(button, BM_SETSTATE, 0, 0);
                send(checkbox, BM_CLICK, 0, 0);
                assert_eq!(send(checkbox, BM_GETCHECK, 0, 0) as u32, BST_UNCHECKED);
                assert_eq!(send(tabs, TCM_GETCURSEL, 0, 0), 1);
                UpdateWindow(status);
                assert_eq!(send(status, SB_GETPARTS, 0, 0), 2);
                let mut status_text = [0u16; 32];
                send(status, SB_GETTEXTW, 0, status_text.as_mut_ptr() as isize);
                assert_eq!(&status_text[..6], wide("Ready").as_slice());
                send(status, SB_SIMPLE, 1, 0);
                set_mode(if mode == 2 { 1 } else { 2 });
                assert_ne!(send(status, SB_ISSIMPLE, 0, 0), 0);
                send(status, SB_SIMPLE, 0, 0);
            }
            if !high_contrast() {
                set_mode(2);
                let dc = CreateCompatibleDC(ptr::null_mut());
                let screen = GetDC(parent);
                let bitmap = GdiObject(CreateCompatibleBitmap(screen, 400, 300));
                ReleaseDC(parent, screen);
                assert!(!dc.is_null() && !bitmap.0.is_null());
                let original = SelectObject(dc, bitmap.0);
                for control in [tabs, status] {
                    if control == status && native_dark_theme() {
                        continue;
                    }
                    let font = GetCurrentObject(dc, OBJ_FONT as u32);
                    SetTextColor(dc, 0x00123456);
                    SetBkMode(dc, OPAQUE as i32);
                    SetPixel(dc, 1, 1, 0x00ff00ff);
                    send(control, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
                    assert_eq!(GetPixel(dc, 1, 1), color(COLOR_BTNFACE));
                    assert_eq!(GetTextColor(dc), 0x00123456);
                    assert_eq!(GetBkMode(dc), OPAQUE as i32);
                    assert_eq!(GetCurrentObject(dc, OBJ_FONT as u32), font);
                }
                assert_eq!(send(tabs, TCM_GETCURSEL, 0, 0), 1);
                assert_eq!(send(status, SB_GETPARTS, 0, 0), 2);
                SelectObject(dc, original);
                DeleteDC(dc);
            }
            DestroyWindow(parent);
            set_mode(0);
        }
    }

    #[test]
    fn older_dark_checkbox_keeps_native_glyph_and_recolors_enabled_and_disabled_text() {
        if !supported() || high_contrast() || native_dark_theme() {
            return;
        }
        let _controls = CommonControls::activate();
        unsafe extern "system" fn native_glyph(
            hwnd: HWND,
            msg: u32,
            w: usize,
            l: isize,
            _: usize,
            only_glyph: usize,
        ) -> isize {
            unsafe {
                if only_glyph != 0 && msg == WM_NOTIFY && l != 0 {
                    let draw = &*(l as *const NMCUSTOMDRAW);
                    if draw.hdr.code == NM_CUSTOMDRAW && draw.dwDrawStage == CDDS_PREPAINT {
                        return CDRF_SKIPDEFAULT as isize;
                    }
                }
                default_subclass(hwnd, msg, w, l)
            }
        }
        unsafe {
            let parent = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                RECT {
                    left: 0,
                    top: 0,
                    right: 220,
                    bottom: 40,
                },
            );
            let checkbox = child(
                parent,
                w!("BUTTON"),
                &wide("&Native checkbox"),
                BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                1,
            )
            .unwrap();
            let area = RECT {
                left: 0,
                top: 0,
                right: 220,
                bottom: 40,
            };
            place(checkbox, area);
            set_subclass(parent, Some(native_glyph), SUBCLASS + 1, 0);
            set_mode(2);
            apply(parent);
            send(checkbox, BM_SETCHECK, BST_CHECKED as usize, 0);
            let dc = CreateCompatibleDC(ptr::null_mut());
            let screen = GetDC(parent);
            let bitmap = GdiObject(CreateCompatibleBitmap(screen, 220, 40));
            ReleaseDC(parent, screen);
            assert!(!dc.is_null() && !bitmap.0.is_null());
            let original = SelectObject(dc, bitmap.0);
            for disabled in [false, true] {
                EnableWindow(checkbox, i32::from(!disabled));
                let mut glyph: SIZE = std::mem::zeroed();
                assert!(
                    GetThemePartSize(
                        GetWindowTheme(checkbox),
                        dc,
                        BP_CHECKBOX,
                        CBS_UNCHECKEDNORMAL,
                        ptr::null(),
                        TS_DRAW,
                        &mut glyph
                    ) >= 0
                );
                set_subclass(parent, Some(native_glyph), SUBCLASS + 1, 1);
                FillRect(dc, &area, brush(COLOR_BTNFACE));
                send(checkbox, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
                let mut native_pixels = Vec::new();
                for y in 0..area.bottom {
                    for x in 0..glyph.cx {
                        native_pixels.push(GetPixel(dc, x, y));
                    }
                }
                assert!(
                    native_pixels
                        .iter()
                        .any(|&pixel| pixel != color(COLOR_BTNFACE))
                );
                set_subclass(parent, Some(native_glyph), SUBCLASS + 1, 0);
                FillRect(dc, &area, brush(COLOR_BTNFACE));
                send(checkbox, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
                let mut index = 0;
                let mut text_pixels = 0;
                let foreground = color(if disabled {
                    COLOR_GRAYTEXT
                } else {
                    COLOR_BTNTEXT
                });
                for y in 0..area.bottom {
                    for x in 0..area.right {
                        let pixel = GetPixel(dc, x, y);
                        if x < glyph.cx {
                            assert_eq!(
                                pixel, native_pixels[index],
                                "Native glyph must be retained"
                            );
                            index += 1;
                        } else if pixel == foreground {
                            text_pixels += 1;
                        }
                    }
                }
                assert!(
                    text_pixels > 0,
                    "Checkbox text must use the requested foreground"
                );
                if !disabled {
                    ShowWindow(parent, SW_SHOW);
                    SetFocus(checkbox);
                    send(
                        checkbox,
                        WM_CHANGEUISTATE,
                        (UIS_SET | UISF_HIDEFOCUS << 16) as usize,
                        0,
                    );
                    FillRect(dc, &area, brush(COLOR_BTNFACE));
                    send(checkbox, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
                    let mut label_pixels = Vec::new();
                    for y in 0..area.bottom {
                        for x in glyph.cx..area.right {
                            label_pixels.push(GetPixel(dc, x, y));
                        }
                    }
                    send(
                        checkbox,
                        WM_CHANGEUISTATE,
                        (UIS_CLEAR | UISF_HIDEFOCUS << 16) as usize,
                        0,
                    );
                    assert_eq!(GetFocus(), checkbox);
                    FillRect(dc, &area, brush(COLOR_BTNFACE));
                    send(checkbox, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
                    let mut changed = false;
                    let mut index = 0;
                    for y in 0..area.bottom {
                        for x in glyph.cx..area.right {
                            changed |= GetPixel(dc, x, y) != label_pixels[index];
                            index += 1;
                        }
                    }
                    assert!(
                        changed,
                        "Keyboard focus must remain visible around the label"
                    );
                    SetFocus(parent);
                }
                assert_eq!(send(checkbox, BM_GETCHECK, 0, 0) as u32, BST_CHECKED);
            }
            SelectObject(dc, original);
            DeleteDC(dc);
            DestroyWindow(parent);
            set_mode(0);
        }
    }

    #[test]
    fn syslink_native_draw_uses_readable_link_color_without_changing_link_state() {
        if !supported() {
            return;
        }
        let _common_controls = CommonControls::activate();
        _common_controls.initialize(ICC_LINK_CLASS);
        unsafe {
            let parent = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                RECT {
                    left: 0,
                    top: 0,
                    right: 300,
                    bottom: 100,
                },
            );
            let link = child(
                parent,
                w!("SysLink"),
                &wide("<a href=\"https://example.com\">Readable link</a>"),
                WS_TABSTOP,
                1,
            )
            .unwrap();
            place(
                link,
                RECT {
                    left: 0,
                    top: 0,
                    right: 250,
                    bottom: 40,
                },
            );
            let mut item = LITEM {
                mask: LIF_ITEMINDEX | LIF_STATE,
                iLink: 0,
                stateMask: LIS_ENABLED | LIS_FOCUSED,
                ..std::mem::zeroed()
            };
            assert_ne!(send_mut(link, LM_GETITEM, 0, &mut item), 0);
            let initial = item.state;
            set_mode(2);
            apply(parent);
            if !high_contrast() {
                let dc = CreateCompatibleDC(ptr::null_mut());
                let screen = GetDC(parent);
                let bitmap = GdiObject(CreateCompatibleBitmap(screen, 250, 40));
                ReleaseDC(parent, screen);
                assert!(!dc.is_null() && !bitmap.0.is_null());
                let original = SelectObject(dc, bitmap.0);
                FillRect(dc, &client(link), brush(COLOR_BTNFACE));
                send(link, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
                let mut readable_pixels = 0;
                for y in 0..40 {
                    for x in 0..250 {
                        if GetPixel(dc, x, y) == color(COLOR_HOTLIGHT) {
                            readable_pixels += 1;
                        }
                    }
                }
                SelectObject(dc, original);
                DeleteDC(dc);
                assert!(
                    readable_pixels > 0,
                    "Native SysLink must use the dark hyperlink palette"
                );
            }
            assert_ne!(send_mut(link, LM_GETITEM, 0, &mut item), 0);
            assert_eq!(item.state, initial);
            set_mode(1);
            assert_ne!(send_mut(link, LM_GETITEM, 0, &mut item), 0);
            assert_eq!(item.state, initial);
            DestroyWindow(parent);
            set_mode(0);
        }
    }

    #[test]
    fn toolbar_states_use_dark_pixels_without_changing_native_button_state() {
        if !supported() || high_contrast() {
            return;
        }
        let controls = CommonControls::activate();
        controls.initialize(ICC_BAR_CLASSES);
        unsafe {
            let parent = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                RECT {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 40,
                },
            );
            let toolbar = child(parent, w!("ToolbarWindow32"), &[0], TBSTYLE_FLAT, 1).unwrap();
            send(
                toolbar,
                TB_BUTTONSTRUCTSIZE,
                std::mem::size_of::<TBBUTTON>(),
                0,
            );
            let button = TBBUTTON {
                iBitmap: I_IMAGENONE,
                idCommand: 10,
                fsState: TBSTATE_ENABLED as u8 | TBSTATE_CHECKED as u8,
                fsStyle: BTNS_CHECK as u8,
                ..std::mem::zeroed()
            };
            assert_ne!(send_ref(toolbar, TB_ADDBUTTONS, 1, &button), 0);
            set_mode(2);
            apply(parent);
            let dc = CreateCompatibleDC(ptr::null_mut());
            let screen = GetDC(parent);
            let bitmap = GdiObject(CreateCompatibleBitmap(screen, 40, 30));
            ReleaseDC(parent, screen);
            let original = SelectObject(dc, bitmap.0);
            for (state, background) in [
                (0, COLOR_BTNFACE),
                (CDIS_CHECKED, COLOR_3DSHADOW),
                (CDIS_HOT, COLOR_3DLIGHT),
                (CDIS_SELECTED, COLOR_3DDKSHADOW),
                (CDIS_DISABLED | CDIS_HOT, COLOR_BTNFACE),
            ] {
                let mut draw: NMTBCUSTOMDRAW = std::mem::zeroed();
                draw.nmcd.hdr.hwndFrom = toolbar;
                draw.nmcd.hdr.code = NM_CUSTOMDRAW;
                draw.nmcd.dwDrawStage = CDDS_ITEMPREPAINT;
                draw.nmcd.hdc = dc;
                draw.nmcd.rc = RECT {
                    left: 0,
                    top: 0,
                    right: 40,
                    bottom: 30,
                };
                draw.nmcd.uItemState = state;
                let result = send_mut(parent, WM_NOTIFY, 1, &mut draw);
                assert_ne!(result & TBCDRF_NOBACKGROUND as isize, 0);
                assert_eq!(GetPixel(dc, 20, 15), color(background));
                assert_eq!(send(toolbar, TB_ISBUTTONCHECKED, 10, 0), 1);
            }
            send(toolbar, TB_CHECKBUTTON, 10, 0);
            assert_eq!(send(toolbar, TB_ISBUTTONCHECKED, 10, 0), 0);
            set_mode(1);
            assert_eq!(send(toolbar, TB_ISBUTTONCHECKED, 10, 0), 0);
            SelectObject(dc, original);
            DeleteDC(dc);
            DestroyWindow(parent);
            set_mode(0);
        }
    }

    #[test]
    fn mdi_menu_glyph_paint_keeps_native_menu_commands_and_states() {
        if !supported() || high_contrast() {
            return;
        }
        unsafe {
            set_mode(2);
            let parent = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                RECT {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 40,
                },
            );
            let menu = CreateMenu();
            send(
                parent,
                WM_SETICON,
                ICON_SMALL as usize,
                LoadIconW(ptr::null_mut(), IDI_APPLICATION) as isize,
            );
            let dc = CreateCompatibleDC(ptr::null_mut());
            let screen = GetDC(parent);
            let bitmap = GdiObject(CreateCompatibleBitmap(screen, 30, 24));
            ReleaseDC(parent, screen);
            let original = SelectObject(dc, bitmap.0);
            for (position, (glyph, command)) in [
                (1usize, 0),
                (3, SC_MINIMIZE),
                (2, SC_RESTORE),
                (5, SC_CLOSE),
            ]
            .into_iter()
            .enumerate()
            {
                let native = MENUITEMINFOW {
                    cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_BITMAP | MIIM_ID | MIIM_STATE | MIIM_FTYPE | MIIM_DATA,
                    wID: command,
                    fType: if glyph == 1 {
                        0
                    } else {
                        MFT_OWNERDRAW | MFT_RIGHTJUSTIFY
                    },
                    dwItemData: parent as usize,
                    hbmpItem: glyph as HBITMAP,
                    ..std::mem::zeroed()
                };
                assert_ne!(InsertMenuItemW(menu, position as u32, 1, &native), 0);
                let mut item: UahMenuItem = std::mem::zeroed();
                item.menu.menu = menu;
                item.menu.dc = dc;
                item.position = position as i32;
                item.draw.rcItem = RECT {
                    left: 0,
                    top: 0,
                    right: 30,
                    bottom: 24,
                };
                assert_eq!(
                    paint_menu(parent, 0x92, (&item as *const UahMenuItem) as isize),
                    Some(0)
                );
                assert_eq!(GetPixel(dc, 1, 1), color(COLOR_MENUBAR));
                let mut painted = false;
                for y in 4..20 {
                    for x in 4..26 {
                        painted |= GetPixel(dc, x, y) != color(COLOR_MENUBAR);
                    }
                }
                assert!(painted, "MDI icon/button must remain visible");
                assert_eq!(GetMenuItemID(menu, position as i32), command);
                let mut retained: MENUITEMINFOW = std::mem::zeroed();
                retained.cbSize = std::mem::size_of::<MENUITEMINFOW>() as u32;
                retained.fMask = MIIM_BITMAP | MIIM_FTYPE;
                assert_ne!(GetMenuItemInfoW(menu, position as u32, 1, &mut retained), 0);
                assert_eq!(retained.hbmpItem, glyph as HBITMAP);
                assert_eq!(retained.fType, native.fType);
                if glyph != 1 {
                    let custom = MENUITEMINFOW {
                        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                        fMask: MIIM_ID,
                        wID: 100,
                        ..std::mem::zeroed()
                    };
                    assert_ne!(SetMenuItemInfoW(menu, position as u32, 1, &custom), 0);
                    assert_eq!(
                        paint_menu(parent, 0x92, (&item as *const UahMenuItem) as isize),
                        None,
                        "Application-owned menu painting must remain with its owner"
                    );
                }
            }
            SelectObject(dc, original);
            DeleteDC(dc);
            DestroyMenu(menu);
            DestroyWindow(parent);
            set_mode(0);
        }
    }

    #[test]
    fn property_sheet_is_dark_before_page_initialization_and_first_display() {
        use crate::{
            dialog::{property_sheet, template_size},
            dpi::{Dpi, Mode},
            resources::{IDD_SETTINGS_GENERAL, IDD_SETTINGS_SEARCH},
        };
        use std::{cell::Cell, os::windows::ffi::OsStrExt};

        if !supported() || high_contrast() {
            eprintln!("Skipping dark property-sheet contract: unsupported or high contrast");
            return;
        }
        let controls = CommonControls::activate();
        controls.initialize(ICC_TAB_CLASSES);
        struct Observations {
            dc: HDC,
            early: Cell<Option<COLORREF>>,
            ready: Cell<Option<(COLORREF, COLORREF)>>,
            installed: Cell<bool>,
        }
        thread_local! {
            static OBSERVATIONS: Cell<*const Observations> = const { Cell::new(ptr::null()) };
        }
        fn erased_pixel(hwnd: HWND, dc: HDC) -> COLORREF {
            unsafe {
                SetPixel(dc, 0, 0, 0x00ff00ff);
                send(hwnd, WM_ERASEBKGND, dc as usize, 0);
                GetPixel(dc, 0, 0)
            }
        }
        unsafe extern "system" fn observe(
            hwnd: HWND,
            message: u32,
            w: usize,
            l: isize,
            _: usize,
            data: usize,
        ) -> isize {
            unsafe {
                let observations = &*(data as *const Observations);
                if message == WM_WINDOWPOSCHANGED
                    && (*(l as *const WINDOWPOS)).flags & SWP_SHOWWINDOW != 0
                    && observations.ready.get().is_none()
                {
                    let page = send(hwnd, PSM_GETCURRENTPAGEHWND, 0, 0) as HWND;
                    observations.ready.set(Some((
                        erased_pixel(hwnd, observations.dc),
                        erased_pixel(page, observations.dc),
                    )));
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                }
                if message == WM_NCDESTROY {
                    remove_subclass(hwnd, Some(observe), 97);
                }
                default_subclass(hwnd, message, w, l)
            }
        }
        unsafe extern "system" fn callback(hwnd: HWND, message: u32, _: isize) -> i32 {
            if message == PSCB_INITIALIZED {
                OBSERVATIONS.with(|current| unsafe {
                    if let Some(observations) = current.get().as_ref() {
                        let installed =
                            set_subclass(hwnd, Some(observe), 97, current.get() as usize) != 0;
                        observations.installed.set(installed);
                        if !installed {
                            PostMessageW(hwnd, WM_CLOSE, 0, 0);
                        }
                    }
                });
            }
            0
        }
        unsafe extern "system" fn observe_page(
            hwnd: HWND,
            message: u32,
            _: usize,
            l: isize,
        ) -> isize {
            unsafe {
                if message == WM_INITDIALOG {
                    let page = &*(l as *const PROPSHEETPAGEW);
                    let observations = &*(page.lParam as *const Observations);
                    if observations.early.get().is_none() {
                        observations
                            .early
                            .set(Some(erased_pixel(GetParent(hwnd), observations.dc)));
                    }
                    return 1;
                }
                0
            }
        }

        // Library test executables do not embed the application's dialog resources.
        let executable = std::env::var_os("WINLEX_TEST_GUI_EXE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_exe()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("winlex-gui.exe")
            });
        let executable: Vec<_> = executable
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        unsafe {
            let module = LoadLibraryExW(
                executable.as_ptr(),
                ptr::null_mut(),
                LOAD_LIBRARY_AS_DATAFILE,
            );
            assert!(
                !module.is_null(),
                "Build the Debug GUI or set WINLEX_TEST_GUI_EXE"
            );
            let templates = [IDD_SETTINGS_GENERAL, IDD_SETTINGS_SEARCH].map(|id| {
                let resource = FindResourceW(module, id as usize as *const u16, RT_DIALOG);
                assert!(!resource.is_null());
                let data = LockResource(LoadResource(module, resource));
                assert!(!data.is_null());
                std::slice::from_raw_parts(
                    data.cast::<u8>(),
                    SizeofResource(module, resource) as usize,
                )
            });
            let baseline = templates
                .map(|template| template_size(template).unwrap())
                .to_vec();
            let dc = CreateCompatibleDC(ptr::null_mut());
            let screen = GetDC(ptr::null_mut());
            let bitmap = GdiObject(CreateCompatibleBitmap(screen, 1, 1));
            ReleaseDC(ptr::null_mut(), screen);
            assert!(!dc.is_null() && !bitmap.0.is_null());
            let original = SelectObject(dc, bitmap.0);
            let observations = Observations {
                dc,
                early: Cell::new(None),
                ready: Cell::new(None),
                installed: Cell::new(false),
            };
            let mut pages: [PROPSHEETPAGEW; 2] = std::mem::zeroed();
            for (page, template) in pages.iter_mut().zip(templates) {
                page.dwSize = std::mem::size_of::<PROPSHEETPAGEW>() as u32;
                page.dwFlags = PSP_DLGINDIRECT | PSP_PREMATURE;
                page.hInstance = instance();
                page.Anonymous1.pResource = template.as_ptr().cast_mut().cast();
                page.pfnDlgProc = Some(observe_page);
                page.lParam = &observations as *const Observations as isize;
            }
            let mut header: PROPSHEETHEADERW_V2 = std::mem::zeroed();
            header.dwSize = std::mem::size_of::<PROPSHEETHEADERW_V2>() as u32;
            header.dwFlags =
                PSH_PROPSHEETPAGE | PSH_NOAPPLYNOW | PSH_NOCONTEXTHELP | PSH_USECALLBACK;
            header.hInstance = instance();
            header.pszCaption = w!("Property sheet rendering contract");
            header.nPages = pages.len() as u32;
            header.Anonymous3.ppsp = pages.as_mut_ptr();
            header.pfnCallback = Some(callback);
            set_mode(2);
            let expected = color(COLOR_BTNFACE);
            OBSERVATIONS.with(|current| current.set(&observations));
            let result = property_sheet::show(&mut header, Dpi { mode: Mode::System }, baseline);
            OBSERVATIONS.with(|current| current.set(ptr::null()));
            SelectObject(dc, original);
            DeleteDC(dc);
            FreeLibrary(module);
            set_mode(0);
            assert!(result.is_ok(), "{result:?}");
            assert!(observations.installed.get());
            assert_eq!(
                observations.early.get(),
                Some(expected),
                "root before first page init"
            );
            assert_eq!(
                observations.ready.get(),
                Some((expected, expected)),
                "root and page before queued preparation"
            );
        }
    }
}
