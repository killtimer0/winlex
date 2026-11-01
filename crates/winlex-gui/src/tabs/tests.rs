use super::*;
use windows::{
    Win32::{Foundation::HWND as Window, System::Com::*, UI::Accessibility::*},
    core::{BSTR, GUID, Interface, PCWSTR, VARIANT},
};

thread_local! {
    static REQUESTS: RefCell<Vec<(u32, u64, usize)>> = const { RefCell::new(vec![]) };
    static EVENTS: RefCell<Vec<(u32, i32)>> = const { RefCell::new(vec![]) };
}
unsafe extern "system" fn host(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    if msg == WM_NOTIFY {
        let n = unsafe { &*(l as *const Notification) };
        REQUESTS.with(|r| r.borrow_mut().push((n.header.code, n.id, n.index)));
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}
struct Fixture {
    parent: HWND,
    tabs: HWND,
    _apartment: crate::input::Apartment,
}
impl Fixture {
    fn new() -> Self {
        let apartment = crate::input::Apartment::initialize().unwrap();
        unsafe {
            let init = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_WIN95_CLASSES,
            };
            assert_ne!(InitCommonControlsEx(&init), 0);
            let class = WNDCLASSW {
                lpfnWndProc: Some(host),
                hInstance: instance(),
                lpszClassName: w!("Winlex.Test.TabHost"),
                ..std::mem::zeroed()
            };
            RegisterClassW(&class);
            let parent = CreateWindowExW(
                0,
                class.lpszClassName,
                w!("Tabs contract"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                600,
                150,
                ptr::null_mut(),
                ptr::null_mut(),
                instance(),
                ptr::null(),
            );
            assert!(!parent.is_null());
            let tabs = create(parent, 42, "Pages".into(), "Select".into()).unwrap();
            place(
                tabs,
                RECT {
                    left: 0,
                    top: 0,
                    right: 500,
                    bottom: 32,
                },
            );
            REQUESTS.with(|r| r.borrow_mut().clear());
            Self {
                parent,
                tabs,
                _apartment: apartment,
            }
        }
    }
    fn accessible(&self) -> IAccessible {
        let mut raw = ptr::null_mut();
        unsafe {
            AccessibleObjectFromWindow(
                Window(self.tabs),
                OBJID_CLIENT as u32,
                &IAccessible::IID,
                &mut raw,
            )
            .unwrap();
            IAccessible::from_raw(raw)
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.parent);
        }
    }
}
fn item(id: u64, title: &str) -> Item {
    Item {
        id,
        title: title.into(),
        description: format!("source {id}"),
        italic: false,
        icon: ptr::null_mut(),
    }
}
fn children(root: &IAccessible) -> Vec<IAccessible> {
    unsafe {
        let count = root.accChildCount().unwrap();
        let mut values = (0..count).map(|_| VARIANT::new()).collect::<Vec<_>>();
        let mut fetched = 0;
        AccessibleChildren(root, 0, &mut values, &mut fetched).unwrap();
        assert_eq!(fetched, count);
        values
            .iter()
            .map(|v| IDispatch::try_from(v).unwrap().cast().unwrap())
            .collect()
    }
}
fn same(a: &IAccessible, b: &IAccessible) -> bool {
    let child = VARIANT::from(0i32);
    unsafe {
        a.get_accName(&child).unwrap() == b.get_accName(&child).unwrap()
            && a.get_accDescription(&child).unwrap() == b.get_accDescription(&child).unwrap()
            && i32::try_from(&a.get_accRole(&child).unwrap()).unwrap()
                == i32::try_from(&b.get_accRole(&child).unwrap()).unwrap()
    }
}
fn requests() -> Vec<(u32, u64, usize)> {
    REQUESTS.with(|r| std::mem::take(&mut *r.borrow_mut()))
}

#[test]
fn empty_tabs_start_usable_and_can_add_remove_the_first_child() {
    let f = Fixture::new();
    let root = f.accessible();
    let child = VARIANT::from(0i32);
    unsafe {
        assert_eq!(root.accChildCount().unwrap(), 0);
        assert_eq!(root.get_accName(&child).unwrap(), BSTR::from("Pages"));
        assert_eq!(
            i32::try_from(&root.get_accRole(&child).unwrap()).unwrap(),
            ROLE_SYSTEM_PAGETABLIST as i32
        );
        assert_eq!(
            root.accSelection().unwrap().as_raw().Anonymous.Anonymous.vt,
            windows::Win32::System::Variant::VT_EMPTY.0
        );
        let font = send(f.tabs, WM_GETFONT, 0, 0) as HFONT;
        assert!(!font.is_null());
        assert_eq!(GetObjectType(font), OBJ_FONT as u32);
    }
    for key in [VK_LEFT, VK_RIGHT, VK_HOME, VK_END] {
        send(f.tabs, WM_KEYDOWN, key as usize, 0);
    }
    send(f.tabs, WM_LBUTTONDOWN, 0, 0);
    send(f.tabs, WM_LBUTTONUP, 0, 0);
    send(f.tabs, WM_MOUSELEAVE, 0, 0);
    assert!(requests().is_empty());
    set_items(f.tabs, vec![item(7, "First")]);
    let first = children(&root);
    assert_eq!(first.len(), 1);
    unsafe {
        assert_eq!(first[0].get_accName(&child).unwrap(), BSTR::from("First"));
    }
    set_items(f.tabs, vec![]);
    unsafe {
        assert_eq!(root.accChildCount().unwrap(), 0);
        assert!(first[0].get_accName(&child).is_err());
    }
}

#[test]
fn italic_titles_keep_overhang_without_painting_outside_their_bounds() {
    unsafe {
        for dpi in [96, 144, 192] {
            let screen = GetDC(ptr::null_mut());
            let dc = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, 320, 128);
            ReleaseDC(ptr::null_mut(), screen);
            assert!(!dc.is_null() && !bitmap.is_null());
            let old_bitmap = SelectObject(dc, bitmap);
            let mut definition: LOGFONTW = std::mem::zeroed();
            definition.lfHeight = -MulDiv(16, dpi, 96);
            definition.lfItalic = 1;
            definition.lfQuality = NONANTIALIASED_QUALITY;
            let face = wide("Arial");
            definition.lfFaceName[..face.len()].copy_from_slice(&face);
            let font = CreateFontIndirectW(&definition);
            assert!(!font.is_null());
            let old_font = SelectObject(dc, font);
            SetBkMode(dc, TRANSPARENT as i32);
            SetTextColor(dc, 0);
            let canvas = RECT {
                left: 0,
                top: 0,
                right: 320,
                bottom: 128,
            };
            let white = GetStockObject(WHITE_BRUSH);
            let title = "Wf".encode_utf16().collect::<Vec<_>>();
            let mut size = SIZE { cx: 0, cy: 0 };
            assert_ne!(
                GetTextExtentPoint32W(dc, title.as_ptr(), title.len() as i32, &mut size),
                0
            );
            let mut full = RECT {
                left: 16,
                top: 8,
                right: 300,
                bottom: 120,
            };
            let pixels = || {
                (0..128)
                    .flat_map(|y| (0..320).map(move |x| GetPixel(dc, x, y)))
                    .collect::<Vec<_>>()
            };
            FillRect(dc, &canvas, white);
            DrawTextW(
                dc,
                title.as_ptr(),
                title.len() as i32,
                &mut full,
                DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
            );
            let reference = pixels();
            let overhang = title_overhang(dc, &title);
            let bounds = RECT {
                right: full.left + size.cx + overhang,
                ..full
            };
            FillRect(dc, &canvas, white);
            draw_title(dc, &title, bounds, overhang);
            let actual = pixels();
            let narrow = RECT {
                right: bounds.left + size.cx / 2 + overhang,
                ..bounds
            };
            FillRect(dc, &canvas, white);
            draw_title(dc, &title, narrow, overhang);
            let clipped = pixels();
            SelectObject(dc, old_font);
            DeleteObject(font);
            SelectObject(dc, old_bitmap);
            DeleteObject(bitmap);
            DeleteDC(dc);
            assert!(
                overhang > 0,
                "fixture must exercise italic overhang at {dpi} DPI"
            );
            assert!(
                reference.iter().enumerate().any(|(index, &pixel)| {
                    (index % 320) as i32 >= full.left + size.cx && pixel != 0x00ffffff
                }),
                "fixture must draw beyond its advance width at {dpi} DPI"
            );
            assert_eq!(
                actual, reference,
                "italic edge must match unconstrained rendering at {dpi} DPI"
            );
            assert!(
                clipped.iter().any(|&pixel| pixel != 0x00ffffff),
                "truncated title must still be drawn at {dpi} DPI"
            );
            assert!(
                clipped.iter().enumerate().all(|(index, &pixel)| {
                    let point = POINT {
                        x: (index % 320) as i32,
                        y: (index / 320) as i32,
                    };
                    contains(narrow, point) || pixel == 0x00ffffff
                }),
                "truncated text must stay within its clipping bounds at {dpi} DPI"
            );
        }
    }
}

#[test]
fn focus_cue_changes_repaint_without_changing_accessible_focus() {
    let f = Fixture::new();
    unsafe {
        SetWindowPos(
            f.parent,
            ptr::null_mut(),
            -10000,
            -10000,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        ShowWindow(f.parent, SW_SHOWNOACTIVATE);
    }
    set_items(f.tabs, vec![item(11, "Alpha")]);
    select(f.tabs, Some(11));
    focus(f.tabs);
    let root = f.accessible();
    for action in [UIS_SET, UIS_CLEAR] {
        unsafe {
            ValidateRect(f.tabs, ptr::null());
        }
        send(
            f.tabs,
            WM_UPDATEUISTATE,
            action as usize | ((UISF_HIDEFOCUS as usize) << 16),
            0,
        );
        assert_eq!(
            send(f.tabs, WM_QUERYUISTATE, 0, 0) & UISF_HIDEFOCUS as isize != 0,
            action == UIS_SET
        );
        assert_ne!(unsafe { GetUpdateRect(f.tabs, ptr::null_mut(), 0) }, 0);
        let page: IAccessible = IDispatch::try_from(&unsafe { root.accFocus() }.unwrap())
            .unwrap()
            .cast()
            .unwrap();
        assert_eq!(
            unsafe { page.get_accName(&0i32.into()) }
                .unwrap()
                .to_string(),
            "Alpha"
        );
        assert!(requests().is_empty());
    }
}

#[test]
fn msaa_false_results_initialize_real_abi_outputs() {
    use std::mem::MaybeUninit;
    use windows::Win32::Foundation::S_FALSE;
    let f = Fixture::new();
    let root = f.accessible();
    let dispatch: IDispatch = root.cast().unwrap();
    assert_eq!(
        root.cast::<windows::core::IUnknown>().unwrap().as_raw(),
        dispatch.cast::<windows::core::IUnknown>().unwrap().as_raw()
    );
    unsafe {
        let table = root.vtable();
        for method in [
            table.get_accValue,
            table.get_accHelp,
            table.get_accKeyboardShortcut,
            table.get_accDefaultAction,
        ] {
            let mut out = MaybeUninit::<BSTR>::uninit();
            std::ptr::write_bytes(out.as_mut_ptr(), 0x55, 1);
            assert_eq!(
                method(root.as_raw(), MaybeUninit::new(0i32.into()), &mut out),
                S_FALSE
            );
            assert!(out.as_ptr().cast::<*const u16>().read().is_null());
            assert!(out.assume_init().is_empty());
        }
        let mut file = MaybeUninit::<BSTR>::uninit();
        std::ptr::write_bytes(file.as_mut_ptr(), 0x55, 1);
        let mut topic = 123;
        assert_eq!(
            (table.get_accHelpTopic)(
                root.as_raw(),
                &mut file,
                MaybeUninit::new(0i32.into()),
                &mut topic
            ),
            S_FALSE
        );
        assert!(file.as_ptr().cast::<*const u16>().read().is_null());
        assert!(file.assume_init().is_empty());
        assert_eq!(topic, 0);
        let mut out = MaybeUninit::<VARIANT>::uninit();
        std::ptr::write_bytes(out.as_mut_ptr(), 0x55, 1);
        assert_eq!(
            (table.accNavigate)(
                root.as_raw(),
                NAVDIR_NEXT as i32,
                MaybeUninit::new(0i32.into()),
                &mut out
            ),
            S_FALSE
        );
        assert!(out.assume_init().is_empty());
    }
}

#[test]
fn msaa_focus_and_selection_are_independent_and_none_is_noop() {
    let f = Fixture::new();
    set_items(f.tabs, vec![item(11, "Alpha"), item(22, "Beta")]);
    select(f.tabs, Some(11));
    let root = f.accessible();
    let self_id = VARIANT::from(0i32);
    unsafe {
        let beta: IAccessible = root.get_accChild(&2i32.into()).unwrap().cast().unwrap();
        beta.accSelect(SELFLAG_NONE as i32, &self_id).unwrap();
        assert!(requests().is_empty());
        beta.accSelect(SELFLAG_TAKEFOCUS as i32, &self_id).unwrap();
        assert!(
            requests().is_empty(),
            "focus alone must not activate a document"
        );
        let title = |value: VARIANT| {
            let page: IAccessible = IDispatch::try_from(&value).unwrap().cast().unwrap();
            page.get_accName(&self_id).unwrap().to_string()
        };
        assert_eq!(title(root.accSelection().unwrap()), "Alpha");
        assert_eq!(title(root.accFocus().unwrap()), "Beta");
        let flags = i32::try_from(&beta.get_accState(&self_id).unwrap()).unwrap() as u32;
        assert_ne!(flags & STATE_SYSTEM_FOCUSED, 0);
        assert_eq!(flags & STATE_SYSTEM_SELECTED, 0);
        beta.accSelect(SELFLAG_TAKESELECTION as i32, &self_id)
            .unwrap();
        assert_eq!(requests(), vec![(SELECT, 22, 0)]);
    }
}

#[test]
fn cancel_mode_releases_capture_without_reordering() {
    let f = Fixture::new();
    set_items(f.tabs, vec![item(11, "Alpha"), item(22, "Beta")]);
    send(f.tabs, WM_LBUTTONDOWN, 0, 10 | (10 << 16));
    assert_eq!(unsafe { GetCapture() }, f.tabs);
    requests();
    send(f.tabs, WM_MOUSEMOVE, 0, 180 | (10 << 16));
    send(f.tabs, WM_CANCELMODE, 0, 0);
    assert!(unsafe { GetCapture() }.is_null());
    send(f.tabs, WM_LBUTTONUP, 0, 180 | (10 << 16));
    assert!(requests().is_empty());
}

#[test]
fn msaa_tree_identity_requests_and_disconnection() {
    let f = Fixture::new();
    let a = item(u64::MAX, "Alpha");
    let b = item(88, "Beta");
    set_items(f.tabs, vec![a.clone(), b.clone()]);
    select(f.tabs, Some(a.id));
    assert!(
        requests().is_empty(),
        "programmatic updates must not request host actions"
    );
    let root = f.accessible();
    let self_id = VARIANT::from(0i32);
    unsafe {
        assert_eq!(root.get_accName(&self_id).unwrap().to_string(), "Pages");
        let dispatch: IDispatch = root.cast().unwrap();
        let name = wide("accName");
        let mut member = 0;
        dispatch
            .GetIDsOfNames(&GUID::zeroed(), &PCWSTR(name.as_ptr()), 1, 0, &mut member)
            .unwrap();
        let mut argument = VARIANT::from(0i32);
        let params = DISPPARAMS {
            rgvarg: &mut argument,
            cArgs: 1,
            ..std::mem::zeroed()
        };
        let mut value = VARIANT::new();
        dispatch
            .Invoke(
                member,
                &GUID::zeroed(),
                0,
                DISPATCH_PROPERTYGET,
                &params,
                Some(&mut value),
                None,
                None,
            )
            .unwrap();
        assert_eq!(BSTR::try_from(&value).unwrap().to_string(), "Pages");
        assert_eq!(
            i32::try_from(&root.get_accRole(&self_id).unwrap()).unwrap(),
            ROLE_SYSTEM_PAGETABLIST as i32
        );
        let original = children(&root);
        let alpha = &original[0];
        assert_eq!(alpha.accChildCount().unwrap(), 0);
        assert!(alpha.get_accChild(&VARIANT::from(1i32)).is_err());
        assert_eq!(alpha.get_accName(&self_id).unwrap().to_string(), "Alpha");
        assert_eq!(
            alpha.get_accDescription(&self_id).unwrap().to_string(),
            format!("source {}", a.id)
        );
        assert!(same(&alpha.accParent().unwrap().cast().unwrap(), &root));
        assert_ne!(
            i32::try_from(&alpha.get_accState(&self_id).unwrap()).unwrap()
                & STATE_SYSTEM_SELECTED as i32,
            0
        );
        let selected: IAccessible = IDispatch::try_from(&root.accSelection().unwrap())
            .unwrap()
            .cast()
            .unwrap();
        assert!(same(&selected, alpha));
        original[1].accDoDefaultAction(&self_id).unwrap();
        send(f.tabs, WM_MBUTTONUP, 0, (10 << 16) | 10);
        assert_eq!(requests(), vec![(SELECT, b.id, 0), (CLOSE, a.id, 0)]);
        assert_eq!(
            root.accChildCount().unwrap(),
            2,
            "host can reject close requests"
        );
        assert_ne!(
            i32::try_from(&alpha.get_accState(&self_id).unwrap()).unwrap()
                & STATE_SYSTEM_SELECTED as i32,
            0,
            "selection is committed by the host"
        );
        set_items(f.tabs, vec![b.clone(), a.clone()]);
        let reordered = children(&root);
        assert!(same(&reordered[1], alpha));
        let next: IAccessible = IDispatch::try_from(
            &reordered[0]
                .accNavigate(NAVDIR_NEXT as i32, &self_id)
                .unwrap(),
        )
        .unwrap()
        .cast()
        .unwrap();
        assert!(same(&next, alpha));
        let mut renamed = a.clone();
        renamed.title = "Renamed".into();
        set_items(f.tabs, vec![b.clone(), renamed]);
        assert_eq!(alpha.get_accName(&self_id).unwrap().to_string(), "Renamed");
        set_items(f.tabs, vec![b]);
        assert!(alpha.get_accName(&self_id).is_err());
        assert!(alpha.accDoDefaultAction(&self_id).is_err());
        assert!(requests().is_empty());
        assert!(root.get_accName(&VARIANT::from("invalid child")).is_err());
        assert!(root.get_accName(&VARIANT::from("0")).is_err());
        drop(f);
        assert!(root.accChildCount().is_err());
        assert!(original[1].accDoDefaultAction(&self_id).is_err());
    }
}

#[test]
fn tab_drag_threshold_uses_horizontal_motion_and_capture_cancellation() {
    let f = Fixture::new();
    set_items(f.tabs, vec![item(1, "First"), item(2, "Second")]);
    let root = f.accessible();
    let pages = children(&root);
    let point = |page: &IAccessible| unsafe {
        let (mut x, mut y, mut width, mut height) = (0, 0, 0, 0);
        page.accLocation(
            &mut x,
            &mut y,
            &mut width,
            &mut height,
            &VARIANT::from(0i32),
        )
        .unwrap();
        let mut p = POINT {
            x: x + width / 2,
            y: y + height / 2,
        };
        ScreenToClient(f.tabs, &mut p);
        p
    };
    let first = point(&pages[0]);
    let second = point(&pages[1]);
    let packed = |x: i32, y: i32| ((x as u16 as u32) | ((y as u16 as u32) << 16)) as isize;
    let down = packed(second.x, second.y);
    send(f.tabs, WM_LBUTTONDOWN, 0, down);
    send(f.tabs, WM_MOUSEMOVE, 0, packed(second.x, second.y + 100));
    send(f.tabs, WM_LBUTTONUP, 0, down);
    assert_eq!(requests(), [(SELECT, 2, 0)]);
    assert!((second.x - first.x).abs() > unsafe { GetSystemMetrics(SM_CXDRAG) });
    for cancel in [false, true] {
        send(f.tabs, WM_LBUTTONDOWN, 0, down);
        send(f.tabs, WM_MOUSEMOVE, 0, packed(first.x, first.y));
        if cancel {
            send(f.tabs, WM_CANCELMODE, 0, 0);
        }
        send(f.tabs, WM_LBUTTONUP, 0, packed(first.x, first.y));
        let expected = if cancel {
            vec![(SELECT, 2, 0)]
        } else {
            vec![(SELECT, 2, 0), (REORDER, 2, 0)]
        };
        assert_eq!(requests(), expected);
    }
}

#[test]
fn keyboard_mouse_and_accessible_geometry_share_the_same_tabs() {
    let f = Fixture::new();
    let items = (1..=8)
        .map(|id| item(id, &format!("Long page title {id}")))
        .collect::<Vec<_>>();
    set_items(f.tabs, items);
    select(f.tabs, Some(1));
    let root = f.accessible();
    let pages = children(&root);
    let self_id = VARIANT::from(0i32);
    unsafe {
        send(f.tabs, WM_KEYDOWN, VK_END as usize, 0);
        assert_eq!(requests(), vec![(SELECT, 8, 0)]);
        select(f.tabs, Some(8));
        let state = i32::try_from(&pages[7].get_accState(&self_id).unwrap()).unwrap();
        assert_eq!(state & STATE_SYSTEM_OFFSCREEN as i32, 0);
        assert_ne!(
            i32::try_from(&pages[0].get_accState(&self_id).unwrap()).unwrap()
                & STATE_SYSTEM_OFFSCREEN as i32,
            0
        );
        assert_eq!(pages[7].accChildCount().unwrap(), 0);
        let (mut x, mut y, mut width, mut height) = (0, 0, 0, 0);
        pages[7]
            .accLocation(&mut x, &mut y, &mut width, &mut height, &self_id)
            .unwrap();
        let x = x + width - 4;
        let y = y + height / 2;
        let hit: IAccessible = IDispatch::try_from(&root.accHitTest(x, y).unwrap())
            .unwrap()
            .cast()
            .unwrap();
        assert!(same(&hit, &pages[7]));
        assert_eq!(i32::try_from(&hit.accHitTest(x, y).unwrap()).unwrap(), 0);
        let mut p = POINT { x, y };
        ScreenToClient(f.tabs, &mut p);
        let position = ((p.x as u16 as u32) | ((p.y as u16 as u32) << 16)) as isize;
        send(f.tabs, WM_LBUTTONDOWN, 0, position);
        send(f.tabs, WM_LBUTTONUP, 0, position);
        assert_eq!(requests(), vec![(SELECT, 8, 0)]);
        send(f.tabs, WM_MBUTTONUP, 0, position);
        assert_eq!(requests(), vec![(CLOSE, 8, 0)]);
    }
}

#[test]
fn msaa_events_describe_committed_selection_with_stable_child_ids() {
    use windows_sys::Win32::{
        System::Threading::GetCurrentThreadId,
        UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
    };
    unsafe extern "system" fn receive(
        _hook: HWINEVENTHOOK,
        event: u32,
        _hwnd: HWND,
        object: i32,
        child: i32,
        _thread: u32,
        _time: u32,
    ) {
        if object == OBJID_CLIENT {
            EVENTS.with(|e| e.borrow_mut().push((event, child)));
        }
    }
    struct Hook(HWINEVENTHOOK);
    impl Drop for Hook {
        fn drop(&mut self) {
            unsafe {
                UnhookWinEvent(self.0);
            }
        }
    }
    fn drain() -> Vec<(u32, i32)> {
        unsafe {
            let mut message = std::mem::zeroed();
            while PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                DispatchMessageW(&message);
            }
        }
        EVENTS.with(|e| std::mem::take(&mut *e.borrow_mut()))
    }
    let f = Fixture::new();
    let hook = Hook(unsafe {
        SetWinEventHook(
            EVENT_OBJECT_CREATE,
            EVENT_OBJECT_DESCRIPTIONCHANGE,
            ptr::null_mut(),
            Some(receive),
            0,
            GetCurrentThreadId(),
            WINEVENT_OUTOFCONTEXT,
        )
    });
    assert!(!hook.0.is_null());
    let a = item(100, "One");
    let b = item(200, "Two");
    set_items(f.tabs, vec![a.clone(), b.clone()]);
    let root = f.accessible();
    drain();
    select(f.tabs, Some(a.id));
    let events = drain();
    let child = events
        .iter()
        .find_map(|&(event, id)| (event == EVENT_OBJECT_SELECTION).then_some(id))
        .expect("selection event");
    unsafe {
        assert_eq!(
            root.get_accName(&VARIANT::from(child)).unwrap().to_string(),
            "One"
        );
    }
    select(f.tabs, Some(a.id));
    assert!(
        !drain()
            .iter()
            .any(|&(event, _)| event == EVENT_OBJECT_SELECTION),
        "unchanged selection is silent"
    );
    set_items(f.tabs, vec![b, a]);
    assert!(drain().contains(&(EVENT_OBJECT_REORDER, 0)));
    unsafe {
        assert_eq!(
            root.get_accName(&VARIANT::from(child)).unwrap().to_string(),
            "One"
        );
    }
    let mut styled = item(100, "One");
    styled.italic = true;
    styled.description = "Temporary item".into();
    set_items(f.tabs, vec![item(200, "Two"), styled]);
    let events = drain();
    assert!(events.contains(&(EVENT_OBJECT_DESCRIPTIONCHANGE, child)));
    assert!(
        !events
            .iter()
            .any(|&(event, _)| event == EVENT_OBJECT_NAMECHANGE)
    );
    unsafe {
        assert_eq!(
            root.get_accName(&VARIANT::from(child)).unwrap().to_string(),
            "One"
        );
        assert_eq!(
            root.get_accDescription(&VARIANT::from(child))
                .unwrap()
                .to_string(),
            "Temporary item"
        );
    }
    set_items(f.tabs, vec![]);
    assert!(drain().contains(&(EVENT_OBJECT_DESTROY, child)));
}
