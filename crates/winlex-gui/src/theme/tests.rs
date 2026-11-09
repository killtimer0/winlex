use super::*;

#[test]
fn accessibility_and_system_capabilities_override_dark_preferences() {
    for mode in [0, 1, 2, u32::MAX] {
        for system in [false, true] {
            assert!(!effective_dark(mode, system, true, true));
            assert!(!effective_dark(mode, system, false, false));
            assert_eq!(
                effective_dark(mode, system, false, true),
                match mode {
                    1 => false,
                    2 => true,
                    _ => system,
                }
            );
        }
    }
}

// Library test executables do not embed the application's common-controls manifest.
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

pub(crate) struct CommonControls(HANDLE, usize, HMODULE);
impl CommonControls {
    pub(crate) fn activate() -> Self {
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
            let mut activation = Self(handle, 0, ptr::null_mut());
            assert_ne!(ActivateActCtx(handle, &mut activation.1), 0);
            activation.2 = LoadLibraryW(w!("comctl32.dll"));
            assert!(!activation.2.is_null());
            activation
        }
    }

    pub(crate) fn module(&self) -> HMODULE {
        self.2
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
            if !self.2.is_null() {
                FreeLibrary(self.2);
            }
            if self.1 != 0 {
                DeactivateActCtx(0, self.1);
            }
            ReleaseActCtx(self.0);
        }
    }
}

#[test]
fn root_notifications_reapply_after_default_without_touching_other_roots() {
    if !supported() {
        return;
    }
    let controls = CommonControls::activate();
    controls.initialize(ICC_LISTVIEW_CLASSES);
    struct Notification {
        list: HWND,
        calls: usize,
    }
    unsafe extern "system" fn observe(
        hwnd: HWND,
        msg: u32,
        w: usize,
        l: isize,
        _: usize,
        data: usize,
    ) -> isize {
        unsafe {
            if matches!(msg, WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED) {
                let notification = &mut *(data as *mut Notification);
                notification.calls += 1;
                send(notification.list, LVM_SETTEXTCOLOR, 0, 0x00123456);
                return 73;
            }
            default_subclass(hwnd, msg, w, l)
        }
    }
    unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(DefWindowProcW),
            hInstance: instance(),
            lpszClassName: w!("Winlex.Test.ThemeNotifications"),
            ..std::mem::zeroed()
        };
        assert!(RegisterClassW(&class) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS);
        let area = crate::layout::rect(0, 0, 200, 100);
        let parent = test_window(class.lpszClassName, ptr::null(), WS_POPUP, area);
        let other = test_window(w!("#32770"), ptr::null(), WS_POPUP, area);
        SetWindowLongPtrW(other, GWLP_HWNDPARENT, parent as _);
        let list = child(parent, w!("SysListView32"), &[0], LVS_REPORT, 1).unwrap();
        let other_list = child(other, w!("SysListView32"), &[0], LVS_REPORT, 1).unwrap();
        let mut notification = Notification { list, calls: 0 };
        set_subclass(
            parent,
            Some(observe),
            97,
            &mut notification as *mut _ as usize,
        );
        set_mode(1);
        apply(parent);
        apply(other);
        for mode in [2, 1] {
            set_mode(mode);
            assert_eq!(
                send(other_list, LVM_GETTEXTCOLOR, 0, 0) as u32,
                color(COLOR_WINDOWTEXT)
            );
            for msg in [WM_SETTINGCHANGE, WM_SYSCOLORCHANGE, WM_THEMECHANGED] {
                send(other_list, LVM_SETTEXTCOLOR, 0, 0x00654321);
                let before = notification.calls;
                assert_eq!(send(parent, msg, 0, 0), 73);
                assert_eq!(notification.calls, before + 1);
                assert_eq!(
                    send(list, LVM_GETTEXTCOLOR, 0, 0) as u32,
                    color(COLOR_WINDOWTEXT)
                );
                assert_eq!(send(other_list, LVM_GETTEXTCOLOR, 0, 0), 0x00654321);
            }
            send(
                other_list,
                LVM_SETTEXTCOLOR,
                0,
                color(COLOR_WINDOWTEXT) as isize,
            );
        }
        DestroyWindow(other);
        DestroyWindow(parent);
        set_mode(0);
    }
}

#[test]
fn applying_and_switching_updates_existing_native_controls() {
    if !supported() {
        set_mode(2);
        assert!(!is_dark());
        set_mode(0);
        return;
    }
    let controls = CommonControls::activate();
    controls.initialize(ICC_LISTVIEW_CLASSES | ICC_BAR_CLASSES);
    unsafe {
        let parent = test_window(
            w!("STATIC"),
            ptr::null(),
            WS_POPUP,
            crate::layout::rect(0, 0, 400, 300),
        );
        let list = child(parent, w!("SysListView32"), &[0], LVS_REPORT, 1).unwrap();
        let edit = child(parent, w!("EDIT"), &[0], 0, 3).unwrap();
        set_mode(2);
        // Reapplication must not recurse or accumulate different subclasses.
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
    let controls = CommonControls::activate();
    controls.initialize(ICC_TAB_CLASSES | ICC_BAR_CLASSES);
    unsafe {
        let parent = test_window(
            w!("STATIC"),
            ptr::null(),
            WS_POPUP,
            crate::layout::rect(0, 0, 400, 300),
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
        set_bounds(button, crate::layout::rect(200, 0, 300, 30)).unwrap();
        set_bounds(checkbox, crate::layout::rect(0, 0, 180, 30)).unwrap();
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
        let tabs = child(parent, w!("SysTabControl32"), &[0], WS_TABSTOP, 2).unwrap();
        set_bounds(tabs, crate::layout::rect(0, 40, 200, 160)).unwrap();
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
            // These button types are behavior, not a choice of painting strategy.
            assert_eq!(
                GetWindowLongW(checkbox, GWL_STYLE) as u32 & BS_TYPEMASK as u32,
                BS_AUTOCHECKBOX as u32
            );
            assert_eq!(
                GetWindowLongW(button, GWL_STYLE) as u32 & BS_TYPEMASK as u32,
                BS_DEFPUSHBUTTON as u32
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
                // Native controls own their PRINTCLIENT colors and DC state;
                // only our fallback painters promise these palette/DC values.
                // Native tab input/selection is exercised above, displayed
                // colors by the GUI suite.
                if (control == tabs || control == status) && native_dark_theme() {
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
fn dark_status_grip_tracks_style_maximized_parent_and_layout() {
    if !supported() || high_contrast() || native_dark_theme() {
        return;
    }
    let controls = CommonControls::activate();
    controls.initialize(ICC_BAR_CLASSES);
    unsafe {
        let parent = test_window(
            w!("STATIC"),
            ptr::null(),
            WS_POPUP,
            crate::layout::rect(0, 0, 240, 100),
        );
        let status = child(parent, w!("msctls_statusbar32"), &[0], 0, 1).unwrap();
        set_bounds(status, crate::layout::rect(0, 0, 240, 24)).unwrap();
        set_mode(2);
        apply(parent);
        let dc = CreateCompatibleDC(ptr::null_mut());
        let screen = GetDC(parent);
        let bitmap = GdiObject(CreateCompatibleBitmap(screen, 240, 24));
        ReleaseDC(parent, screen);
        assert!(!dc.is_null() && !bitmap.0.is_null());
        let previous = SelectObject(dc, bitmap.0);
        for (grip, maximized, rtl, expected_left, expected_right) in [
            (false, false, false, false, false),
            (true, false, false, false, true),
            (true, true, false, false, false),
            (true, false, true, true, false),
        ] {
            let style = GetWindowLongW(status, GWL_STYLE) as u32;
            SetWindowLongW(
                status,
                GWL_STYLE,
                (style & !SBARS_SIZEGRIP | if grip { SBARS_SIZEGRIP } else { 0 }) as i32,
            );
            let style = GetWindowLongW(parent, GWL_STYLE) as u32;
            SetWindowLongW(
                parent,
                GWL_STYLE,
                (style & !WS_MAXIMIZE | if maximized { WS_MAXIMIZE } else { 0 }) as i32,
            );
            let style = GetWindowLongW(status, GWL_EXSTYLE) as u32;
            SetWindowLongW(
                status,
                GWL_EXSTYLE,
                (style & !WS_EX_LAYOUTRTL | if rtl { WS_EX_LAYOUTRTL } else { 0 }) as i32,
            );
            send(status, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
            let bounds = client(status);
            let mut left = false;
            let mut right = false;
            for y in 0..bounds.bottom.min(24) {
                for x in 0..30.min(bounds.right) {
                    let pixel = GetPixel(dc, x, y);
                    assert_ne!(pixel, CLR_INVALID);
                    left |= pixel != color(COLOR_BTNFACE);
                }
                for x in (bounds.right - 30).max(0)..bounds.right.min(240) {
                    let pixel = GetPixel(dc, x, y);
                    assert_ne!(pixel, CLR_INVALID);
                    right |= pixel != color(COLOR_BTNFACE);
                }
            }
            assert_eq!(
                (left, right),
                (expected_left, expected_right),
                "grip={grip}, maximized={maximized}, rtl={rtl}"
            );
        }
        SelectObject(dc, previous);
        DeleteDC(dc);
        DestroyWindow(parent);
        set_mode(0);
    }
}

#[test]
fn dark_checkbox_keeps_text_contrast_focus_and_check_state() {
    if !supported() || high_contrast() {
        return;
    }
    let _controls = CommonControls::activate();
    unsafe {
        let area = crate::layout::rect(0, 0, 220, 40);
        let parent = test_window(w!("STATIC"), ptr::null(), WS_POPUP, area);
        let checkbox = child(
            parent,
            w!("BUTTON"),
            &wide("&Checkbox label"),
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
            1,
        )
        .unwrap();
        set_bounds(checkbox, area).unwrap();
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
            FillRect(dc, &area, brush(COLOR_BTNFACE));
            send(checkbox, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
            let foreground = color(if disabled {
                COLOR_GRAYTEXT
            } else {
                COLOR_BTNTEXT
            });
            assert_ne!(foreground, color(COLOR_BTNFACE));
            // Exclude the glyph: custom Win10 and native Win11 glyphs are both valid.
            let mut text_pixels = 0;
            for y in 0..area.bottom {
                for x in 40..area.right {
                    text_pixels += usize::from(GetPixel(dc, x, y) == foreground);
                }
            }
            assert!(
                text_pixels > 0,
                "Checkbox label must use a readable foreground"
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
                let mut hidden_focus = Vec::new();
                for y in 0..area.bottom {
                    for x in 40..area.right {
                        hidden_focus.push(GetPixel(dc, x, y));
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
                    for x in 40..area.right {
                        changed |= GetPixel(dc, x, y) != hidden_focus[index];
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
fn syslink_mixed_text_keeps_normal_link_and_disabled_colors_and_native_state() {
    if !supported() {
        return;
    }
    let controls = CommonControls::activate();
    controls.initialize(ICC_LINK_CLASS);
    unsafe {
        let parent = test_window(
            w!("STATIC"),
            ptr::null(),
            WS_POPUP,
            crate::layout::rect(0, 0, 600, 100),
        );
        let link = child(
            parent,
            w!("SysLink"),
            &wide(
                "Ordinary before <a href=\"https://example.com\">Readable link</a> ordinary between \
                 <a href=\"https://example.org\">Disabled link</a> ordinary after",
            ),
            WS_TABSTOP,
            1,
        )
        .unwrap();
        set_bounds(link, crate::layout::rect(0, 0, 600, 80)).unwrap();
        let mut item = LITEM {
            mask: LIF_ITEMINDEX | LIF_STATE,
            iLink: 0,
            stateMask: LIS_VISITED,
            state: LIS_VISITED,
            ..std::mem::zeroed()
        };
        assert_ne!(send_mut(link, LM_SETITEM, 0, &mut item), 0);
        item.iLink = 1;
        item.stateMask = LIS_ENABLED;
        item.state = 0;
        assert_ne!(send_mut(link, LM_SETITEM, 0, &mut item), 0);
        item.stateMask = LIS_ENABLED | LIS_FOCUSED | LIS_VISITED | LIS_DEFAULTCOLORS;
        let initial = [0, 1].map(|index| {
            item.iLink = index;
            assert_ne!(send_mut(link, LM_GETITEM, 0, &mut item), 0);
            item.state
        });
        let dc = CreateCompatibleDC(ptr::null_mut());
        let screen = GetDC(parent);
        let bitmap = GdiObject(CreateCompatibleBitmap(screen, 600, 80));
        ReleaseDC(parent, screen);
        assert!(!dc.is_null() && !bitmap.0.is_null());
        let original = SelectObject(dc, bitmap.0);
        apply(parent);
        for mode in [2, 1, 2] {
            set_mode(mode);
            if is_dark() {
                FillRect(dc, &client(link), brush(COLOR_BTNFACE));
                send(link, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
                let mut normal_pixels = 0;
                let mut link_pixels = 0;
                let mut disabled_pixels = 0;
                for y in 0..80 {
                    for x in 0..600 {
                        let pixel = GetPixel(dc, x, y);
                        assert_ne!(pixel, CLR_INVALID);
                        normal_pixels += usize::from(pixel == color(COLOR_BTNTEXT));
                        link_pixels += usize::from(pixel == color(COLOR_HOTLIGHT));
                        disabled_pixels += usize::from(pixel == color(COLOR_GRAYTEXT));
                    }
                }
                assert!(
                    normal_pixels > 0,
                    "Ordinary SysLink text must not use hyperlink color"
                );
                assert!(
                    link_pixels > 0,
                    "Enabled links must use the dark hyperlink palette"
                );
                assert!(
                    disabled_pixels > 0,
                    "Disabled links must use the disabled text palette"
                );
            }
            for (index, state) in initial.into_iter().enumerate() {
                item.iLink = index as i32;
                assert_ne!(send_mut(link, LM_GETITEM, 0, &mut item), 0);
                assert_eq!(
                    item.state, state,
                    "Theme switches must preserve native link state"
                );
            }
        }
        SelectObject(dc, original);
        DeleteDC(dc);
        DestroyWindow(parent);
        set_mode(0);
    }
}

#[test]
fn edit_and_combo_list_backgrounds_follow_upstream_color_contract() {
    if !supported() || high_contrast() {
        return;
    }
    let _controls = CommonControls::activate();
    unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(DefWindowProcW),
            hInstance: instance(),
            lpszClassName: w!("Winlex.Test.ControlColors"),
            ..std::mem::zeroed()
        };
        assert!(RegisterClassW(&class) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS);
        let parent = test_window(
            class.lpszClassName,
            ptr::null(),
            WS_POPUP,
            crate::layout::rect(0, 0, 300, 120),
        );
        let edit = child(
            parent,
            w!("EDIT"),
            &wide("Read only"),
            ES_READONLY as u32,
            1,
        )
        .unwrap();
        let combo = child(parent, w!("COMBOBOX"), &[0], CBS_DROPDOWNLIST as u32, 2).unwrap();
        let mut info: COMBOBOXINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of_val(&info) as u32;
        assert_ne!(GetComboBoxInfo(combo, &mut info), 0);
        let list = child(parent, w!("LISTBOX"), &[0], 0, 3).unwrap();
        set_mode(2);
        apply(parent);
        let dc = GetDC(parent);
        // Darkmodelib onCtlColor uses background; editable controls use ctrlBackground.
        for (control, message, expected) in [
            (edit, WM_CTLCOLORSTATIC, rgb(0x202020)),
            (info.hwndList, WM_CTLCOLORLISTBOX, rgb(0x202020)),
            (list, WM_CTLCOLORLISTBOX, rgb(0x383838)),
        ] {
            assert_ne!(send(parent, message, dc as usize, control as isize), 0);
            assert_eq!(GetBkColor(dc), expected);
            assert_eq!(GetTextColor(dc), rgb(0xe0e0e0));
        }
        EnableWindow(edit, 0);
        send(parent, WM_CTLCOLORSTATIC, dc as usize, edit as isize);
        assert_eq!(GetBkColor(dc), rgb(0x202020));
        assert_eq!(GetTextColor(dc), rgb(0x808080));
        ReleaseDC(parent, dc);
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
            crate::layout::rect(0, 0, 100, 40),
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
        assert!(!dc.is_null() && !bitmap.0.is_null());
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
            item.draw.rcItem = crate::layout::rect(0, 0, 30, 24);
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
            retained.fMask = MIIM_BITMAP | MIIM_FTYPE | MIIM_STATE;
            assert_ne!(GetMenuItemInfoW(menu, position as u32, 1, &mut retained), 0);
            assert_eq!(retained.hbmpItem, glyph as HBITMAP);
            assert_eq!(retained.fType, native.fType);
            assert_eq!(retained.fState, native.fState);
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
    unsafe extern "system" fn observe_page(hwnd: HWND, message: u32, _: usize, l: isize) -> isize {
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

    // Library tests load dialog templates from the separately built Debug GUI.
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
            std::slice::from_raw_parts(data.cast::<u8>(), SizeofResource(module, resource) as usize)
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
        header.dwFlags = PSH_PROPSHEETPAGE | PSH_NOAPPLYNOW | PSH_NOCONTEXTHELP;
        header.hInstance = instance();
        header.pszCaption = w!("Property sheet rendering contract");
        header.nPages = pages.len() as u32;
        header.Anonymous3.ppsp = pages.as_mut_ptr();
        set_mode(2);
        let expected = color(COLOR_BTNFACE);
        OBSERVATIONS.with(|current| current.set(&observations));
        let result =
            property_sheet::show(&mut header, Dpi { mode: Mode::System }, baseline, callback);
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
