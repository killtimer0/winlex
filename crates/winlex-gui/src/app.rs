//! UI-thread state, resource lifetime and the native message loop.
//!
//! Frame owns shared controls and root DPI orchestration; workspace owns MDI
//! lifecycle/arrangement; document owns its controls and content. Native MDI is
//! the sole authority for activation. Context owns UI-thread state and is
//! reached from window procedures through the frame's user data. State borrows
//! must end before Win32 calls that can synchronously reenter a window
//! procedure.
use crate::{
    dpi::{Dpi, Font},
    layout,
    native::*,
};
use std::ptr;

mod catalog;
mod commands;
mod content;
mod context;
mod document;
mod frame;
mod help;
mod history;
mod keys;
mod metadata;
mod panel;
mod settings;
mod sidebar;
mod sources;
mod status;
mod tools;
mod workspace;

use crate::resources::{self, *};
use context::Context;
use document::document_proc;
use frame::frame_proc;
use workspace::active;

struct App {
    config: crate::config::Config,
    settings: settings::Settings,
    dpi: Dpi,
    system_version: crate::platform::WindowsVersion,
    frame: HWND,
    icons: Option<crate::icons::WindowIcons>,
    status: HWND,
    status_state: status::Status,
    session: catalog::Session,
    sources: sources::Sources,
    worker: crate::worker::Worker<crate::tasks::Job, crate::tasks::Completion>,
    sidebar: sidebar::Sidebar,
    workspace: workspace::Workspace,
    fonts: Vec<Font>,
    failure: Option<anyhow::Error>,
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
) -> anyhow::Result<()> {
    let system_version = crate::platform::WindowsVersion::query()?;
    let _apartment = crate::input::Apartment::initialize()?;
    let _graphics = crate::icons::Runtime::new()?;
    let settings = settings::Settings::load(&config);
    crate::theme::set_mode(settings.preferences.theme as u32);
    resources::initialize(settings.preferences.language)?;
    let session = catalog::Session::new(paths.is_empty());
    let worker = crate::worker::Worker::new(crate::tasks::run)?;
    let paths = if paths.is_empty() {
        config
            .key
            .as_ref()
            .map_or_else(Vec::new, |key| key.paths("AutoLoad"))
    } else {
        paths
    };
    let sidebar = sidebar::Sidebar::new(&config);
    let context = Box::new(Context::new(App {
        config,
        settings,
        dpi,
        system_version,
        frame: ptr::null_mut(),
        icons: None,
        status: ptr::null_mut(),
        status_state: status::Status::default(),
        session,
        sources: sources::Sources::default(),
        worker,
        sidebar,
        workspace: workspace::Workspace::default(),
        fonts: Vec::new(),
        failure: None,
    }));
    unsafe { run_windows(&context, paths) }
}
unsafe fn run_windows(ctx: &Context, paths: Vec<std::path::PathBuf>) -> anyhow::Result<()> {
    unsafe {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES | ICC_TAB_CLASSES | ICC_BAR_CLASSES | ICC_LINK_CLASS,
        };
        if InitCommonControlsEx(&controls) == 0 {
            anyhow::bail!("Initialize common controls failed");
        }
        let classes: [(*const u16, WNDPROC); 3] = [
            (w!("Winlex.Frame"), Some(frame_proc)),
            (w!("Winlex.Document"), Some(document_proc)),
            (w!("Winlex.Splitter"), Some(sidebar::splitter_proc)),
        ];
        for (name, proc) in classes {
            let class = WNDCLASSW {
                lpfnWndProc: proc,
                hInstance: instance(),
                hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
                hIcon: LoadIconW(ptr::null_mut(), IDI_APPLICATION),
                hbrBackground: (COLOR_BTNFACE + 1) as usize as HBRUSH,
                lpszClassName: name,
                ..std::mem::zeroed()
            };
            if RegisterClassW(&class) == 0 {
                return Err(error("Register window class"));
            }
        }
        let menu = resources::menu()?;
        let value = ctx.dpi().system();
        let saved = ctx.read(|a| a.settings.window);
        let frame = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            w!("Winlex.Frame"),
            resources::wide_text(IDS_APP_TITLE).as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            saved.map_or(CW_USEDEFAULT, |w| w.x),
            saved.map_or(CW_USEDEFAULT, |w| w.y),
            layout::scale(layout::DEFAULT_WIDTH, value),
            layout::scale(layout::DEFAULT_HEIGHT, value),
            ptr::null_mut(),
            menu,
            instance(),
            ctx as *const Context as *mut _,
        );
        if frame.is_null() {
            let failure = ctx
                .write(|a| a.failure.take())
                .unwrap_or_else(|| error("Create main window"));
            DestroyMenu(menu);
            return Err(failure);
        }
        if let Some(error) = ctx.write(|a| a.failure.take()) {
            DestroyWindow(frame);
            return Err(error);
        }
        settings::restore_window(ctx, frame);
        let accelerators = LoadAcceleratorsW(instance(), IDR_ACCELERATORS as usize as *const u16);
        if accelerators.is_null() {
            let failure = error("Load accelerators");
            DestroyWindow(frame);
            return Err(failure);
        }
        if !paths.is_empty() {
            catalog::reload(ctx, paths, vec![]);
        }
        if let Some(warning) = ctx.read(|a| a.config.warning()) {
            status::set_status(ctx, format!("{warning:#}"));
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
        sidebar::focus(ctx);
        let mut msg: MSG = std::mem::zeroed();
        loop {
            history::refresh(ctx);
            if let Some(error) = ctx.write(|a| a.failure.take()) {
                DestroyWindow(frame);
                return Err(error);
            }
            let event = ctx.read(|a| a.worker.event());
            let wake =
                MsgWaitForMultipleObjectsEx(1, &event, u32::MAX, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            if wake == WAIT_FAILED {
                let failure = error("Wait for UI messages");
                DestroyWindow(frame);
                return Err(failure);
            }
            if wake == WAIT_OBJECT_0 {
                poll_completions(ctx);
            }
            if PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) == 0 {
                continue;
            }
            if msg.message == WM_QUIT {
                break;
            }
            let (mdi, document_tabs) = ctx.read(|a| (a.workspace.mdi, a.workspace.tabs));
            if matches!(
                msg.message,
                WM_KEYDOWN | WM_SYSKEYDOWN | WM_LBUTTONDOWN | WM_RBUTTONDOWN
            ) {
                status::status_input(ctx, msg.hwnd);
            }
            if let Some(document) = workspace::content(ctx, active(ctx))
                && document.find_key(&msg)
            {
                continue;
            }
            if TranslateMDISysAccel(mdi, &msg) != 0
                || TranslateAcceleratorW(frame, accelerators, &msg) != 0
                || keys::translate(ctx, &msg)
                || sidebar::translate(ctx, frame, &msg)
                || msg.hwnd == document_tabs && IsDialogMessageW(frame, &msg) != 0
            {
                continue;
            }
            let doc = active(ctx);
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
fn poll_completions(ctx: &Context) {
    use crate::tasks::Completion;
    for _ in 0..8 {
        match ctx.read(|a| a.worker.rx.try_recv()) {
            Ok(Completion::Loaded { id, result }) => catalog::loaded(ctx, id, result),
            Ok(Completion::Matches { id, items }) => catalog::matches(ctx, id, items),
            Ok(Completion::SearchDone { id, result }) => catalog::search_done(ctx, id, result),
            Ok(Completion::Description {
                id,
                document,
                result,
            }) => {
                if let Some(view) = workspace::by_id(ctx, document) {
                    view.complete(id, result);
                }
            }
            Ok(Completion::ErrorCodeQuery {
                document,
                revision,
                result,
            }) => {
                if let Some(tool) = workspace::content_by_id(ctx, document).and_then(|c| c.tool()) {
                    tool.query_complete(revision, result);
                }
            }
            Ok(Completion::ErrorCodeLanguages {
                document,
                path,
                result,
            }) => {
                if let Some(tool) = workspace::content_by_id(ctx, document).and_then(|c| c.tool()) {
                    tool.languages_complete(path, result);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                catalog::flush_results(ctx);
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                catalog::flush_results(ctx);
                catalog::worker_stopped(ctx);
                for view in workspace::views(ctx) {
                    view.worker_stopped();
                }
                panel::cancel_debounce(ctx);
                return;
            }
        }
    }
    catalog::flush_results(ctx);
    ctx.read(|a| a.worker.resignal());
}
