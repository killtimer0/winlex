// SPDX-License-Identifier: MPL-2.0
// Standard-control theme integration ported from win32-darkmodelib
// Copyright (c) 2025-2026 ozone10. Rust integration: Winlex contributors.
//! One UI-thread theme engine. Standard controls use the darkmodelib paint contracts;
//! application content consumes the same palette without replacing its input behavior.
mod animation;
mod api;
mod button;
mod controls;
mod input;
mod static_text;
mod task_dialog;

pub(crate) use api::supported;
use api::{native_dark_theme, windows11};
pub(crate) use task_dialog::{
    HookGuard as TaskDialogThemeGuard, apply_native as apply_task_dialog,
};

/// IFileDialog::Show selects AllowDark internally. Restore our preference from
/// its first navigation event, before Shell creates the native client controls.
pub(crate) fn prepare_file_dialog() {
    api::set_app(is_dark(), high_contrast());
}

/// Shell dialogs retain their native client rendering and input ownership.
pub(crate) use task_dialog::apply as apply_file_dialog;

fn refresh_file_dialog(hwnd: HWND) {
    // Shell can replace the process preference even when our palette is unchanged.
    // Restore it before SetWindowTheme lets the existing native view reopen themes.
    prepare_file_dialog();
    api::allow_window(hwnd, is_dark());
    set_native_theme(hwnd, "DarkMode_Explorer", None);
    apply_file_dialog_controls(hwnd);
}

/// Opt standard dialog controls into their native dark classes without changing
/// Shell's painters, window procedures, or control state.
pub(crate) fn apply_file_dialog_controls(hwnd: HWND) {
    if hwnd.is_null() || !supported() {
        return;
    }
    unsafe extern "system" fn child(hwnd: HWND, _: isize) -> BOOL {
        let theme = match class_name(hwnd).as_str() {
            "button" => "DarkMode_Explorer",
            "combobox" | "comboboxex32" => "DarkMode_CFD",
            "directuihwnd" | "shelldll_defview" => {
                api::allow_window(hwnd, is_dark());
                // Reopen the native Shell view's themes without choosing a
                // replacement theme class or taking over its rendering.
                unsafe { SendMessageW(hwnd, WM_THEMECHANGED, 0, 0) };
                return 1;
            }
            _ => return 1,
        };
        api::allow_window(hwnd, is_dark());
        set_native_theme(hwnd, theme, None);
        1
    }
    unsafe { EnumChildWindows(hwnd, Some(child), 0) };
}

use crate::native::*;
use std::{
    cell::{Cell, RefCell},
    ptr,
};
use windows_sys::Win32::Globalization::lstrlenW;
use windows_sys::Win32::UI::Shell::GetWindowSubclass;

const SUBCLASS: usize = 0x574c5448;
pub(crate) const DARK_CONTENT_TEXT: COLORREF = rgb(0xe0e2e4);

// DmlibSubclass.h::BufferData: retain a compatible surface until its size changes.
struct Buffer {
    dc: HDC,
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    width: i32,
    height: i32,
}

impl Buffer {
    unsafe fn new(target: HDC, width: i32, height: i32) -> Option<Self> {
        unsafe {
            let dc = CreateCompatibleDC(target);
            if dc.is_null() {
                return None;
            }
            let bitmap = CreateCompatibleBitmap(target, width, height);
            if bitmap.is_null() {
                DeleteDC(dc);
                return None;
            }
            let old_bitmap = SelectObject(dc, bitmap);
            if old_bitmap.is_null() || old_bitmap as isize == -1 {
                DeleteObject(bitmap);
                DeleteDC(dc);
                return None;
            }
            Some(Self {
                dc,
                bitmap,
                old_bitmap,
                width,
                height,
            })
        }
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old_bitmap);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

#[derive(Default)]
struct State {
    mode: u32,
    dark: bool,
    contrast: bool,
    initialized: bool,
    brushes: Vec<(COLORREF, GdiObject)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    static APPLYING: Cell<bool> = const { Cell::new(false) };
}

#[derive(Clone, Copy)]
enum Role {
    Background,
    Control,
    Hot,
    Dialog,
    Text,
    DimText,
    DisabledText,
    Link,
    Edge,
    HotEdge,
    DisabledEdge,
    Highlight,
}

const fn rgb(value: u32) -> COLORREF {
    ((value & 0xff0000) >> 16) | (value & 0xff00) | ((value & 0xff) << 16)
}

fn color_role(role: Role) -> COLORREF {
    if !is_dark() {
        return unsafe {
            GetSysColor(match role {
                Role::Background | Role::Dialog => COLOR_BTNFACE,
                Role::Control => COLOR_WINDOW,
                Role::Hot | Role::Highlight => COLOR_HIGHLIGHT,
                Role::Text => COLOR_WINDOWTEXT,
                Role::DimText => COLOR_BTNTEXT,
                Role::DisabledText | Role::DisabledEdge => COLOR_GRAYTEXT,
                Role::Link => COLOR_HOTLIGHT,
                Role::Edge => COLOR_3DSHADOW,
                Role::HotEdge => COLOR_HIGHLIGHT,
            })
        };
    }
    // DmlibColor.h: default black tone, kept separate from view/content colors.
    rgb(match role {
        Role::Background | Role::Dialog => 0x202020,
        Role::Control => 0x383838,
        Role::Hot => 0x454545,
        Role::Text => 0xe0e0e0,
        Role::DimText => 0xc0c0c0,
        Role::DisabledText => 0x808080,
        Role::Link | Role::Highlight => 0x60cdff,
        Role::Edge => 0x646464,
        Role::HotEdge => 0x9b9b9b,
        Role::DisabledEdge => 0x484848,
    })
}

fn cached_brush(value: COLORREF) -> HBRUSH {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if let Some((_, brush)) = state.brushes.iter().find(|(rgb, _)| *rgb == value) {
            return brush.0;
        }
        let handle = unsafe { CreateSolidBrush(value) };
        if !handle.is_null() {
            state.brushes.push((value, GdiObject(handle)));
        }
        handle
    })
}

fn brush_role(role: Role) -> HBRUSH {
    cached_brush(color_role(role))
}

pub fn is_dark() -> bool {
    STATE.with(|state| state.borrow().dark)
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

/// The application owns preference and accessibility policy, never the painters.
pub fn set_mode(mode: u32) {
    STATE.with(|state| state.borrow_mut().mode = if mode <= 2 { mode } else { 0 });
    if refresh_palette() {
        apply_roots();
    }
}

fn refresh_palette() -> bool {
    if !supported() {
        return false;
    }
    let contrast = high_contrast();
    let system = api::system_dark();
    let changed = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let dark = effective_dark(state.mode, system, contrast, true);
        let changed = !state.initialized || state.dark != dark || state.contrast != contrast;
        state.initialized = true;
        state.dark = dark;
        state.contrast = contrast;
        changed
    });
    if changed {
        api::set_app(is_dark(), contrast);
    }
    changed
}

fn apply_roots() {
    unsafe extern "system" fn refresh(hwnd: HWND, _: isize) -> BOOL {
        let mut data = 0;
        if unsafe { GetWindowSubclass(hwnd, Some(subclass), SUBCLASS, &mut data) } != 0 {
            apply(hwnd);
        }
        1
    }
    unsafe {
        EnumThreadWindows(
            windows_sys::Win32::System::Threading::GetCurrentThreadId(),
            Some(refresh),
            0,
        );
    }
}

/// System-color compatibility for Winlex's custom content. Standard controls use roles.
pub fn color(index: i32) -> COLORREF {
    if !is_dark() {
        return unsafe { GetSysColor(index) };
    }
    match index {
        COLOR_WINDOW => rgb(0x293134),
        COLOR_WINDOWTEXT => DARK_CONTENT_TEXT,
        COLOR_INFOBK | COLOR_MENUBAR | COLOR_MENU => color_role(Role::Background),
        COLOR_BTNTEXT | COLOR_MENUTEXT | COLOR_INFOTEXT | COLOR_CAPTIONTEXT => {
            color_role(Role::Text)
        }
        COLOR_GRAYTEXT | COLOR_INACTIVECAPTIONTEXT => color_role(Role::DisabledText),
        COLOR_HIGHLIGHT => rgb(0x0078d7),
        COLOR_HIGHLIGHTTEXT => color_role(Role::Text),
        COLOR_HOTLIGHT => color_role(Role::Link),
        COLOR_3DSHADOW | COLOR_WINDOWFRAME | COLOR_ACTIVEBORDER | COLOR_INACTIVEBORDER => {
            color_role(Role::Edge)
        }
        COLOR_3DHIGHLIGHT | COLOR_3DLIGHT => color_role(Role::Hot),
        COLOR_3DDKSHADOW => color_role(Role::DisabledEdge),
        _ => color_role(Role::Dialog),
    }
}

/// Borrowed brushes survive a switch, including nested native painting.
pub fn brush(index: i32) -> HBRUSH {
    if is_dark() {
        cached_brush(color(index))
    } else {
        unsafe { GetSysColorBrush(index) }
    }
}

fn class_name(hwnd: HWND) -> String {
    let mut name = [0u16; 128];
    let count = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
    String::from_utf16_lossy(&name[..count.max(0) as usize]).to_ascii_lowercase()
}

fn set_native_theme(hwnd: HWND, dark_name: &str, light_name: Option<&str>) {
    let name = if is_dark() {
        (!dark_name.is_empty()).then_some(dark_name)
    } else {
        light_name
    }
    .map(wide);
    unsafe {
        SetWindowTheme(
            hwnd,
            name.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
            ptr::null(),
        );
    }
}

/// Register before first display, and reapply after native theme cache invalidation.
pub fn apply(hwnd: HWND) {
    if !supported() || hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
        return;
    }
    if APPLYING.with(|applying| applying.replace(true)) {
        return;
    }
    struct Applying;
    impl Drop for Applying {
        fn drop(&mut self) {
            APPLYING.with(|value| value.set(false));
        }
    }
    let _applying = Applying;
    unsafe extern "system" fn child(hwnd: HWND, _: isize) -> BOOL {
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
}

fn apply_one(hwnd: HWND) {
    let class = class_name(hwnd);
    let dark = is_dark();
    unsafe {
        set_subclass(hwnd, Some(subclass), SUBCLASS, 0);
        api::allow_window(hwnd, dark);
        // Application classes own content, not a uxtheme paint surface.
        if !class.starts_with("winlex") {
            match class.as_str() {
                // Dialogs provide the parent color/erase surface. Explorer's
                // control theme belongs to their children, not the sheet/page.
                "#32770" => set_native_theme(hwnd, "", None),
                "combobox" | "edit" | "listbox" => {
                    input::install(hwnd, &class);
                    input::apply(hwnd, &class);
                }
                "syslistview32" => {
                    set_native_theme(hwnd, "Explorer", (!high_contrast()).then_some("Explorer"));
                    send(hwnd, LVM_SETBKCOLOR, 0, color(COLOR_WINDOW) as isize);
                    send(hwnd, LVM_SETTEXTBKCOLOR, 0, color(COLOR_WINDOW) as isize);
                    send(hwnd, LVM_SETTEXTCOLOR, 0, color(COLOR_WINDOWTEXT) as isize);
                }
                "button" => {
                    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
                    let kind = style & BS_TYPEMASK as u32;
                    let check = matches!(kind, 2..=7 | 9);
                    if kind == BS_GROUPBOX as u32
                        || check && style & BS_PUSHLIKE as u32 == 0 && !windows11()
                    {
                        set_native_theme(hwnd, "", None);
                    } else {
                        set_native_theme(hwnd, "DarkMode_Explorer", None);
                    }
                    button::install(hwnd);
                }
                "systabcontrol32" => {
                    set_native_theme(
                        hwnd,
                        if native_dark_theme() {
                            "DarkMode_DarkTheme"
                        } else {
                            ""
                        },
                        None,
                    );
                    controls::install(hwnd, &class);
                }
                "msctls_statusbar32" => {
                    set_native_theme(
                        hwnd,
                        if native_dark_theme() {
                            "DarkMode_DarkTheme"
                        } else {
                            "DarkMode_Explorer"
                        },
                        None,
                    );
                    controls::install(hwnd, &class);
                }
                "toolbarwindow32" => {
                    set_native_theme(hwnd, "DarkMode_Explorer", None);
                    let scheme = COLORSCHEME {
                        dwSize: std::mem::size_of::<COLORSCHEME>() as u32,
                        clrBtnHighlight: if dark {
                            color_role(Role::Dialog)
                        } else {
                            CLR_DEFAULT as u32
                        },
                        clrBtnShadow: if dark {
                            color_role(Role::Dialog)
                        } else {
                            CLR_DEFAULT as u32
                        },
                    };
                    send_ref(hwnd, TB_SETCOLORSCHEME, 0, &scheme);
                }
                "static" => static_text::install(hwnd),
                "syslink" => {}
                _ => set_native_theme(hwnd, "DarkMode_Explorer", None),
            }
        }
        if GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CHILD == 0 {
            api::titlebar(hwnd, dark);
        }
        let message = match class.as_str() {
            "syslistview32" => LVM_GETTOOLTIPS,
            "toolbarwindow32" => TB_GETTOOLTIPS,
            "systabcontrol32" => TCM_GETTOOLTIPS,
            _ => 0,
        };
        if message != 0 {
            let tooltip = send(hwnd, message, 0, 0) as HWND;
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
        if msg == WM_PARENTNOTIFY
            && w as u16 as u32 == WM_CREATE
            && native_dark_theme()
            && class_name(hwnd) == "systabcontrol32"
            && class_name(l as HWND) == "msctls_updown32"
        {
            // Native tabs can create their overflow after the initial walk.
            // Opt that child into the current theme without taking over paint
            // or input; a later full walk handles existing controls normally.
            apply_one(l as HWND);
        }
        if matches!(msg, WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED) {
            let applying = APPLYING.with(Cell::get);
            let root = GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CHILD == 0;
            let changed = if !applying && root {
                if msg == WM_SETTINGCHANGE && l != 0 {
                    let value = std::slice::from_raw_parts(
                        l as *const u16,
                        lstrlenW(l as *const u16) as usize,
                    );
                    if String::from_utf16_lossy(value).eq_ignore_ascii_case("ImmersiveColorSet") {
                        api::refresh_policy();
                    }
                }
                refresh_palette()
            } else {
                false
            };
            let result = default_subclass(hwnd, msg, w, l);
            if !applying && root {
                if changed {
                    apply_roots();
                } else {
                    apply(hwnd);
                }
            }
            return result;
        }
        if is_dark() {
            match msg {
                WM_NOTIFY if l != 0 => {
                    if let Some(result) = notify(hwnd, msg, w, l) {
                        return result;
                    }
                }
                0x91 | 0x92 => {
                    if let Some(result) = paint_menu(hwnd, msg, l) {
                        return result;
                    }
                }
                WM_NCPAINT | WM_NCACTIVATE if !GetMenu(hwnd).is_null() => {
                    let result = default_subclass(hwnd, msg, w, l);
                    menu_bottom_line(hwnd);
                    return result;
                }
                WM_CTLCOLORBTN | WM_CTLCOLORDLG | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX
                | WM_CTLCOLORSTATIC | WM_CTLCOLORSCROLLBAR => {
                    return control_color(hwnd, msg, w, l);
                }
                WM_ERASEBKGND => {
                    let class = class_name(hwnd);
                    if class == "mdiclient" {
                        FillRect(w as HDC, &client(hwnd), brush(COLOR_WINDOW));
                        return 1;
                    }
                    if class == "#32770" || class.starts_with("winlex") {
                        FillRect(w as HDC, &client(hwnd), brush_role(Role::Dialog));
                        return 1;
                    }
                }
                WM_PRINTCLIENT if class_name(hwnd) == "#32770" => {
                    // Match the upstream parent-color subclass: native themed
                    // children may request their dialog's background, but the
                    // default dialog printer must not repaint a light surface.
                    return 1;
                }
                _ => {}
            }
        }
        default_subclass(hwnd, msg, w, l)
    }
}

unsafe fn notify(hwnd: HWND, msg: u32, w: usize, l: isize) -> Option<isize> {
    unsafe {
        let hdr = &*(l as *const NMHDR);
        if hdr.code != NM_CUSTOMDRAW {
            return None;
        }
        // Business custom drawing (result text / icons) retains ownership.
        let result = default_subclass(hwnd, msg, w, l);
        if result & CDRF_SKIPDEFAULT as isize != 0 {
            return Some(result);
        }
        let themed = match class_name(hdr.hwndFrom).as_str() {
            "toolbarwindow32" => controls::toolbar(&mut *(l as *mut NMTBCUSTOMDRAW)),
            "syslink" => {
                let draw = &*(l as *const NMCUSTOMDRAW);
                match draw.dwDrawStage {
                    CDDS_PREPAINT => CDRF_NOTIFYITEMDRAW as isize,
                    CDDS_ITEMPREPAINT => {
                        SetTextColor(
                            draw.hdc,
                            color_role(if draw.uItemState & CDIS_DISABLED != 0 {
                                Role::DisabledText
                            } else if draw.dwItemSpec == usize::MAX {
                                // SysLink also custom-draws non-anchor text runs.
                                Role::Text
                            } else {
                                Role::Link
                            }),
                        );
                        SetBkColor(draw.hdc, color_role(Role::Dialog));
                        CDRF_NEWFONT as isize
                    }
                    _ => 0,
                }
            }
            _ => 0,
        };
        Some(result | themed)
    }
}

unsafe fn control_color(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    unsafe {
        let dc = w as HDC;
        let original = default_subclass(hwnd, msg, w, l);
        if original != 0 && !(0..31).any(|index| original == GetSysColorBrush(index) as isize) {
            return original;
        }
        let class = class_name(l as HWND);
        let child = l as HWND;
        let enabled = if class == "static" {
            static_text::enabled(child)
        } else {
            IsWindowEnabled(child) != 0
        };
        let role = if !enabled {
            Role::Dialog
        } else if msg == WM_CTLCOLOREDIT {
            Role::Control
        } else if msg == WM_CTLCOLORLISTBOX {
            if GetWindowLongW(child, GWL_STYLE) & LBS_COMBOBOX != 0 {
                Role::Background
            } else {
                Role::Control
            }
        } else if msg == WM_CTLCOLORSTATIC && class == "edit" {
            Role::Background
        } else {
            Role::Dialog
        };
        SetTextColor(
            dc,
            color_role(if !enabled {
                Role::DisabledText
            } else {
                Role::Text
            }),
        );
        SetBkColor(dc, color_role(role));
        brush_role(role) as isize
    }
}

// UAH menu painting follows DmlibSubclassWindow.cpp. The MDI bitmap extension is
// Winlex-specific: retain native menu tracking and merged child-window commands.
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
            FillRect(menu.dc, &info.rcBar, brush_role(Role::Dialog));
            return Some(0);
        }
        let item = &*(data as *const UahMenuItem);
        let mut info: MENUITEMINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MENUITEMINFOW>() as u32;
        info.fMask = MIIM_STRING | MIIM_FTYPE | MIIM_BITMAP | MIIM_DATA | MIIM_ID;
        if GetMenuItemInfoW(item.menu.menu, item.position as u32, 1, &mut info) == 0
            || (info.fType & MFT_OWNERDRAW != 0 && !mdi_menu_command(&info))
            || (!info.hbmpItem.is_null()
                && !matches!(info.hbmpItem as usize, 1 | 2 | 3 | 5 | 6 | 7))
        {
            return None;
        }
        let mut text = vec![0u16; info.cch as usize + 1];
        info.dwTypeData = text.as_mut_ptr();
        info.cch = text.len() as u32;
        if GetMenuItemInfoW(item.menu.menu, item.position as u32, 1, &mut info) == 0 {
            return None;
        }
        let pushed = item.draw.itemState & ODS_SELECTED != 0;
        let hot = item.draw.itemState & ODS_HOTLIGHT != 0;
        let disabled = item.draw.itemState & (ODS_DISABLED | ODS_GRAYED | ODS_INACTIVE) != 0;
        FillRect(
            item.menu.dc,
            &item.draw.rcItem,
            brush_role(if pushed {
                Role::Control
            } else if hot {
                Role::Hot
            } else {
                Role::Dialog
            }),
        );
        if !info.hbmpItem.is_null() {
            paint_mdi_menu_bitmap(hwnd, item, info.hbmpItem, info.dwItemData as HWND, disabled);
            return Some(0);
        }
        let theme = OpenThemeData(hwnd, w!("Menu"));
        let state = menu_text_state(item.draw.itemState);
        let mut options: DTTOPTS = std::mem::zeroed();
        options.dwSize = std::mem::size_of::<DTTOPTS>() as u32;
        options.dwFlags = DTT_TEXTCOLOR;
        options.crText = color_role(if matches!(state, MBI_DISABLED | MBI_DISABLEDHOT) {
            Role::DisabledText
        } else {
            Role::Text
        });
        let mut area = item.draw.rcItem;
        let flags = DT_CENTER
            | DT_VCENTER
            | DT_SINGLELINE
            | if item.draw.itemState & ODS_NOACCEL != 0 {
                DT_HIDEPREFIX
            } else {
                0
            };
        if theme != 0 {
            DrawThemeTextEx(
                theme,
                item.menu.dc,
                MENU_BARITEM,
                state,
                text.as_ptr(),
                -1,
                flags,
                &mut area,
                &options,
            );
            CloseThemeData(theme);
        }
        Some(0)
    }
}

fn menu_text_state(state: u32) -> i32 {
    if state & ODS_SELECTED != 0 {
        MBI_PUSHED
    } else if state & ODS_HOTLIGHT != 0 {
        if state & ODS_INACTIVE != 0 {
            MBI_DISABLEDHOT
        } else {
            MBI_HOT
        }
    } else if state & (ODS_DISABLED | ODS_GRAYED | ODS_INACTIVE) != 0 {
        MBI_DISABLED
    } else {
        MBI_NORMAL
    }
}

#[cfg(test)]
mod menu_state_tests {
    use super::*;

    #[test]
    fn menubar_text_states_follow_native_tracking_precedence() {
        for (flags, expected) in [
            (0, MBI_NORMAL),
            (ODS_DISABLED, MBI_DISABLED),
            (ODS_GRAYED, MBI_DISABLED),
            (ODS_INACTIVE, MBI_DISABLED),
            (ODS_HOTLIGHT, MBI_HOT),
            (ODS_HOTLIGHT | ODS_INACTIVE, MBI_DISABLEDHOT),
            (ODS_SELECTED | ODS_HOTLIGHT | ODS_INACTIVE, MBI_PUSHED),
        ] {
            assert_eq!(menu_text_state(flags), expected);
            assert_eq!(menu_text_state(flags | ODS_NOACCEL), expected);
        }
    }
}

unsafe fn menu_bottom_line(hwnd: HWND) {
    unsafe {
        let mut info: MENUBARINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MENUBARINFO>() as u32;
        if GetMenuBarInfo(hwnd, OBJID_MENU, 0, &mut info) == 0 {
            return;
        }
        let mut rect = client(hwnd);
        MapWindowPoints(hwnd, ptr::null_mut(), (&mut rect as *mut RECT).cast(), 2);
        let mut window: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut window);
        OffsetRect(&mut rect, -window.left, -window.top);
        rect.bottom = rect.top;
        rect.top -= 1;
        let dc = GetWindowDC(hwnd);
        if !dc.is_null() {
            FillRect(dc, &rect, brush_role(Role::Dialog));
            ReleaseDC(hwnd, dc);
        }
    }
}

fn mdi_menu_command(info: &MENUITEMINFOW) -> bool {
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
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    unsafe {
        let dc = item.menu.dc;
        let area = item.draw.rcItem;
        let dpi = GetDpiForWindow(hwnd);
        if bitmap as usize == 1 {
            let width = GetSystemMetricsForDpi(SM_CXSMICON, dpi);
            let height = GetSystemMetricsForDpi(SM_CYSMICON, dpi);
            DrawIconEx(
                dc,
                area.left + (area.right - area.left - width) / 2,
                area.top + (area.bottom - area.top - height) / 2,
                send(child, WM_GETICON, ICON_SMALL as usize, 0) as HICON,
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
        let size = GetSystemMetricsForDpi(SM_CYSMICON, dpi)
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
        SetTextColor(
            dc,
            color_role(if disabled || matches!(bitmap as usize, 6 | 7) {
                Role::DisabledText
            } else {
                Role::Text
            }),
        );
        let glyph = match bitmap as usize {
            3 | 7 => b'0',
            2 => b'2',
            _ => b'r',
        } as u16;
        if !font.0.is_null() {
            TextOutW(
                dc,
                area.left + (area.right - area.left - size) / 2,
                area.top + (area.bottom - area.top - size) / 2,
                &glyph,
                1,
            );
        }
        RestoreDC(dc, saved);
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod file_dialog_tests {
    use super::*;

    #[test]
    fn native_file_dialog_refresh_restores_policy_even_when_palette_is_unchanged() {
        if !supported()
            || high_contrast()
            || !crate::platform::WindowsVersion::query()
                .is_ok_and(|version| version.build() >= 18362)
        {
            return;
        }
        let _controls = tests::CommonControls::activate();
        type PreferredApp = unsafe extern "system" fn(i32) -> i32;
        unsafe {
            let preferred: PreferredApp = std::mem::transmute(
                GetProcAddress(GetModuleHandleW(w!("uxtheme.dll")), 135usize as *const u8).unwrap(),
            );
            let hwnd = test_window(
                w!("#32770"),
                ptr::null(),
                WS_POPUP,
                crate::layout::rect(0, 0, 100, 100),
            );
            for mode in [2, 1, 2] {
                set_mode(mode);
                let expected = if is_dark() { 2 } else { 3 };
                for _ in 0..2 {
                    // Model a native Shell policy override without changing Winlex's
                    // effective palette or navigating/recreating the dialog.
                    preferred(if expected == 2 { 3 } else { 2 });
                    assert!(!refresh_palette());
                    refresh_file_dialog(hwnd);
                    assert_eq!(
                        preferred(expected),
                        expected,
                        "native theme refresh must restore the application preference"
                    );
                }
            }
            DestroyWindow(hwnd);
            set_mode(0);
        }
    }
}
