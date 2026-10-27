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

mod catalog;
mod document;
mod frame;
mod settings;
mod sidebar;
mod sources;
mod workspace;

use crate::resources::{self, *};
use document::{document_proc, restore_focus};
use frame::{frame_proc, splitter_proc};
use workspace::active;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Architecture {
    X86,
    X64,
}
impl Architecture {
    fn core(self) -> winlex_core::Architecture {
        match self {
            Self::X86 => winlex_core::Architecture::X86,
            Self::X64 => winlex_core::Architecture::X64,
        }
    }
    fn supported_by(self, mask: i32) -> bool {
        mask & (1 << self.index()) != 0
    }
    fn index(self) -> usize {
        match self {
            Self::X86 => 0,
            Self::X64 => 1,
        }
    }
    fn label(self) -> String {
        resources::text(match self {
            Self::X86 => IDS_X86,
            Self::X64 => IDS_X64,
        })
    }
    fn other(self) -> Self {
        match self {
            Self::X86 => Self::X64,
            Self::X64 => Self::X86,
        }
    }
}

#[derive(Clone)]
struct Document {
    id: u64,
    hwnd: HWND,
    declaration: HWND,
    actions: Vec<(u16, HWND)>,
    old_version: bool,
    last_focus: HWND,
    placement: Placement,
    title: String,
    architecture: Architecture,
    query: catalog::DocumentQuery,
}
impl Document {
    fn architecture_name(&self) -> String {
        self.architecture.label()
    }
}
struct App {
    config: crate::config::Config,
    dpi: Dpi,
    frame: HWND,
    mdi: HWND,
    search: HWND,
    source_list: HWND,
    toolbar: [HWND; 2],
    source_tools: [HWND; 4],
    composing: bool,
    results: HWND,
    splitter: HWND,
    document_tabs: HWND,
    status: HWND,
    session: catalog::Session,
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

pub fn run(
    dpi: Dpi,
    config: crate::config::Config,
    paths: Vec<std::path::PathBuf>,
) -> Result<(), String> {
    let mut session = catalog::Session::new()?;
    if config.panel == 1 {
        session.panel = catalog::Panel::Sources;
    }
    let sidebar = config.sidebar;
    APP.with(|a| {
        *a.borrow_mut() = Some(App {
            config,
            dpi,
            frame: ptr::null_mut(),
            mdi: ptr::null_mut(),
            search: ptr::null_mut(),
            source_list: ptr::null_mut(),
            toolbar: [ptr::null_mut(); 2],
            source_tools: [ptr::null_mut(); 4],
            composing: false,
            results: ptr::null_mut(),
            splitter: ptr::null_mut(),
            document_tabs: ptr::null_mut(),
            status: ptr::null_mut(),
            session,
            sidebar,
            arrangement: Arrangement::Free,
            documents: Vec::new(),
            fonts: Vec::new(),
        })
    });
    let result = unsafe { run_windows(paths) };
    APP.with(|a| a.borrow_mut().take());
    result
}
unsafe fn run_windows(paths: Vec<std::path::PathBuf>) -> Result<(), String> {
    unsafe {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES | ICC_TAB_CLASSES | ICC_BAR_CLASSES,
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
        let saved = read(|a| a.config.window);
        let frame = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            wide("Winlex.Frame").as_ptr(),
            wide(&resources::text(IDS_APP_TITLE)).as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            saved.map_or(CW_USEDEFAULT, |w| w.x),
            saved.map_or(CW_USEDEFAULT, |w| w.y),
            layout::scale(1000, value),
            layout::scale(680, value),
            ptr::null_mut(),
            menu,
            instance(),
            ptr::null(),
        );
        if frame.is_null() {
            let failure = error("Create main window");
            DestroyMenu(menu);
            return Err(failure);
        }
        settings::restore_window(frame);
        settings::update_language_menu();
        let accelerators = LoadAcceleratorsW(instance(), IDR_ACCELERATORS as usize as *const u16);
        if accelerators.is_null() {
            let failure = error("Load accelerators");
            DestroyWindow(frame);
            return Err(failure);
        }
        write(|a| a.session.sources.extra = paths.clone());
        sources::refresh();
        if !paths.is_empty() {
            catalog::load(paths);
        }
        if let Some(warning) = read(|a| a.config.warning.clone()) {
            catalog::set_status(warning);
        }
        ShowWindow(
            frame,
            if saved.is_some_and(|w| w.maximized) {
                SW_SHOWMAXIMIZED
            } else {
                SW_SHOW
            },
        );
        UpdateWindow(frame);
        focus(sidebar::focus_target());
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let event = catalog::event();
            let wake =
                MsgWaitForMultipleObjectsEx(1, &event, u32::MAX, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            if wake == WAIT_FAILED {
                let failure = error("Wait for UI messages");
                DestroyWindow(frame);
                return Err(failure);
            }
            if wake == WAIT_OBJECT_0 {
                catalog::poll();
            }
            if PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) == 0 {
                continue;
            }
            if msg.message == WM_QUIT {
                break;
            }
            let (mdi, search, results, document_tabs, source_list) =
                read(|a| (a.mdi, a.search, a.results, a.document_tabs, a.source_list));
            if TranslateMDISysAccel(mdi, &msg) != 0 {
                continue;
            }
            if TranslateAcceleratorW(frame, accelerators, &msg) != 0 {
                continue;
            }
            if msg.message == WM_KEYDOWN {
                match msg.wParam as u16 {
                    VK_F6 => {
                        let h = GetFocus();
                        let d = active();
                        if !d.is_null() && (h == d || IsChild(d, h) != 0) {
                            focus(sidebar::focus_target());
                        } else {
                            restore_focus(d);
                        }
                        continue;
                    }
                    VK_RETURN if msg.hwnd == search && !frame::composing() => {
                        catalog::search();
                        continue;
                    }
                    VK_RETURN if msg.hwnd == results => {
                        catalog::open_selected();
                        continue;
                    }
                    VK_RETURN if msg.hwnd == source_list => {
                        sources::open_row(send(
                            source_list,
                            LVM_GETNEXTITEM,
                            usize::MAX,
                            LVNI_SELECTED as isize,
                        ));
                        continue;
                    }
                    VK_RETURN if msg.hwnd == document_tabs => {
                        restore_focus(active());
                        continue;
                    }
                    _ => {}
                }
            }
            if ([search, results, document_tabs, source_list].contains(&msg.hwnd)
                || read(|a| a.toolbar.contains(&msg.hwnd) || a.source_tools.contains(&msg.hwnd)))
                && IsDialogMessageW(frame, &msg) != 0
            {
                continue;
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
