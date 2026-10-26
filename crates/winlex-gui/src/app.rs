//! UI-thread state, resource lifetime and the native message loop.
//!
//! Frame owns shared controls and root DPI orchestration; workspace owns MDI
//! lifecycle/arrangement; document owns its controls and content. Native MDI is
//! the sole authority for activation. State borrows must end before Win32 calls
//! that can synchronously reenter a window procedure.
use crate::{
    dpi::{Dpi, Font},
    layout::{self, Arrangement, Placement},
    native::*,
};
use std::{cell::RefCell, ptr};

mod document;
mod frame;
mod workspace;

use crate::resources::{self, *};
use document::{document_proc, remember_focus, restore_focus};
use frame::{command, frame_proc, open_search, splitter_proc};
use workspace::{active, new_document};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Architecture {
    X86,
    X64,
}
const ARCHITECTURES: [(Architecture, u16); 2] =
    [(Architecture::X86, IDS_X86), (Architecture::X64, IDS_X64)];
impl Architecture {
    fn from_index(index: usize) -> Option<Self> {
        ARCHITECTURES.get(index).map(|o| o.0)
    }
    fn index(self) -> usize {
        ARCHITECTURES.iter().position(|o| o.0 == self).unwrap()
    }
    fn label(self) -> String {
        resources::text(ARCHITECTURES[self.index()].1)
    }
}

#[derive(Clone)]
struct Document {
    hwnd: HWND,
    tabs: HWND,
    arch: HWND,
    declaration: HWND,
    table: HWND,
    last_focus: HWND,
    placement: Placement,
    title: String,
    architecture: Architecture,
    column_widths: [i32; 2],
}
impl Document {
    fn architecture_name(&self) -> String {
        self.architecture.label()
    }
}
struct App {
    dpi: Dpi,
    frame: HWND,
    mdi: HWND,
    search: HWND,
    open: HWND,
    results: HWND,
    splitter: HWND,
    document_tabs: HWND,
    sidebar: i32,
    arrangement: Arrangement,
    documents: Vec<Document>,
    fonts: Vec<Font>,
}
thread_local! {static APP:RefCell<Option<App>>=const {RefCell::new(None)};}
// These closures only inspect/update owned data; never send messages inside them.
fn read<T>(f: impl FnOnce(&App) -> T) -> T {
    APP.with(|a| f(a.borrow().as_ref().unwrap()))
}
fn write<T>(f: impl FnOnce(&mut App) -> T) -> T {
    APP.with(|a| f(a.borrow_mut().as_mut().unwrap()))
}
fn dpi() -> Dpi {
    read(|a| a.dpi)
}
fn document(hwnd: HWND) -> Option<Document> {
    read(|a| a.documents.iter().find(|d| d.hwnd == hwnd).cloned())
}
fn edit_document(hwnd: HWND, edit: impl FnOnce(&mut Document)) {
    write(|a| {
        if let Some(doc) = a.documents.iter_mut().find(|d| d.hwnd == hwnd) {
            edit(doc);
        }
    });
}
fn font_for(value: u32) -> (HFONT, i32) {
    if let Some(font) = read(|a| {
        a.fonts
            .iter()
            .find(|f| f.dpi == value)
            .map(|f| (f.handle, f.height))
    }) {
        return font;
    }
    let font = Font::new(value);
    let result = (font.handle, font.height);
    write(|a| a.fonts.push(font));
    result
}
fn bind(hwnd: HWND, font: HFONT) {
    if send(hwnd, WM_GETFONT, 0, 0) != font as isize {
        send(hwnd, WM_SETFONT, font as usize, 1);
    }
}

pub fn run(dpi: Dpi) -> Result<(), String> {
    APP.with(|a| {
        *a.borrow_mut() = Some(App {
            dpi,
            frame: ptr::null_mut(),
            mdi: ptr::null_mut(),
            search: ptr::null_mut(),
            open: ptr::null_mut(),
            results: ptr::null_mut(),
            splitter: ptr::null_mut(),
            document_tabs: ptr::null_mut(),
            sidebar: 240,
            arrangement: Arrangement::Free,
            documents: Vec::new(),
            fonts: Vec::new(),
        })
    });
    let result = unsafe { run_windows() };
    APP.with(|a| a.borrow_mut().take());
    result
}
unsafe fn run_windows() -> Result<(), String> {
    unsafe {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES | ICC_TAB_CLASSES,
        };
        if InitCommonControlsEx(&controls) == 0 {
            return Err(error("Initialize common controls"));
        }
        initialize_subclass()?;
        let classes: [(&str, WNDPROC); 3] = [
            ("Winlex.Frame", Some(frame_proc)),
            ("Winlex.Document", Some(document_proc)),
            ("Winlex.Splitter", Some(splitter_proc)),
        ];
        for (name, proc) in classes {
            let name = wide(name);
            let class = WNDCLASSW {
                lpfnWndProc: proc,
                hInstance: instance(),
                hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
                hIcon: LoadIconW(ptr::null_mut(), IDI_APPLICATION),
                hbrBackground: (COLOR_BTNFACE + 1) as usize as HBRUSH,
                lpszClassName: name.as_ptr(),
                ..std::mem::zeroed()
            };
            if RegisterClassW(&class) == 0 {
                return Err(error("Register window class"));
            }
        }
        let menu = resources::menu()?;
        let value = dpi().root;
        let frame = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            wide("Winlex.Frame").as_ptr(),
            wide(&resources::text(IDS_APP_TITLE)).as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            layout::scale(1000, value),
            layout::scale(680, value),
            ptr::null_mut(),
            menu,
            instance(),
            ptr::null(),
        );
        if frame.is_null() {
            DestroyMenu(menu);
            return Err(error("Create main window"));
        }
        let value = dpi().window(frame);
        let mut monitor: MONITORINFO = std::mem::zeroed();
        monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let mut width = layout::scale(1000, value);
        let mut height = layout::scale(680, value);
        if GetMonitorInfoW(
            MonitorFromWindow(frame, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        ) != 0
        {
            width = width.min(monitor.rcWork.right - monitor.rcWork.left);
            height = height.min(monitor.rcWork.bottom - monitor.rcWork.top);
        }
        SetWindowPos(
            frame,
            ptr::null_mut(),
            0,
            0,
            width,
            height,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let initialized = (|| {
            new_document(&resources::text(IDS_DOCUMENT), Architecture::X86)?;
            let doc = new_document(&resources::text(IDS_DOCUMENT), Architecture::X64)?;
            let mdi = read(|a| a.mdi);
            send(mdi, WM_MDIMAXIMIZE, doc as usize, 0);
            Ok::<_, String>(())
        })();
        if let Err(e) = initialized {
            DestroyWindow(frame);
            return Err(e);
        }
        ShowWindow(frame, SW_SHOW);
        UpdateWindow(frame);
        focus(read(|a| a.search));
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let status = GetMessageW(&mut msg, ptr::null_mut(), 0, 0);
            if status == 0 {
                break;
            }
            if status < 0 {
                DestroyWindow(frame);
                return Err(error("GetMessage"));
            }
            remember_focus();
            let (mdi, search, open, results, document_tabs) =
                read(|a| (a.mdi, a.search, a.open, a.results, a.document_tabs));
            if TranslateMDISysAccel(mdi, &msg) != 0 {
                continue;
            }
            if msg.message == WM_KEYDOWN {
                let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
                match msg.wParam as u16 {
                    VK_F6 => {
                        let h = GetFocus();
                        let d = active();
                        if !d.is_null() && (h == d || IsChild(d, h) != 0) {
                            focus(search);
                        } else {
                            restore_focus(d);
                        }
                        continue;
                    }
                    0x4e if ctrl => {
                        command(NEW_X86);
                        continue;
                    }
                    0x4c if ctrl => {
                        focus(search);
                        send(search, EM_SETSEL, 0, -1);
                        continue;
                    }
                    VK_RETURN if msg.hwnd == search => {
                        open_search();
                        continue;
                    }
                    VK_RETURN if msg.hwnd == results => {
                        let i = send(results, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
                        if i >= 0 {
                            command(if i == 0 { NEW_X86 } else { NEW_X64 });
                        }
                        continue;
                    }
                    VK_RETURN if msg.hwnd == document_tabs => {
                        restore_focus(active());
                        continue;
                    }
                    VK_TAB if [search, open, results, document_tabs].contains(&msg.hwnd) => {
                        let order: Vec<_> = [search, open, results, document_tabs]
                            .into_iter()
                            .filter(|&h| IsWindowEnabled(h) != 0)
                            .collect();
                        let i = order.iter().position(|&h| h == msg.hwnd).unwrap();
                        focus(
                            order[(i + if GetKeyState(VK_SHIFT as i32) < 0 {
                                order.len() - 1
                            } else {
                                1
                            }) % order.len()],
                        );
                        continue;
                    }
                    _ => {}
                }
            }
            let doc = active();
            if !doc.is_null()
                && (msg.hwnd == doc || IsChild(doc, msg.hwnd) != 0)
                && IsDialogMessageW(doc, &msg) != 0
            {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        Ok(())
    }
}
