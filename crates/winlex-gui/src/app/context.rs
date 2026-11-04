//! Owns UI-thread state and exposes it to native window procedures.
use super::{catalog, settings, sidebar, sources, status, workspace};
use crate::{
    dpi::{Dpi, Font},
    layout,
    native::*,
    tasks::{Completion, Job},
    worker::Ticket,
};
use std::{cell::RefCell, ptr};

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

pub(super) struct Context {
    state: RefCell<App>,
}
impl Context {
    pub fn new(
        config: crate::config::Config,
        settings: settings::Settings,
        dpi: Dpi,
        system_version: crate::platform::WindowsVersion,
        session: catalog::Session,
        worker: crate::worker::Worker<crate::tasks::Job, crate::tasks::Completion>,
        sidebar: sidebar::Sidebar,
    ) -> Self {
        Self {
            state: RefCell::new(App {
                config,
                settings,
                dpi,
                system_version,
                frame: std::ptr::null_mut(),
                icons: None,
                status: std::ptr::null_mut(),
                status_state: status::Status::default(),
                session,
                sources: sources::Sources::default(),
                worker,
                sidebar,
                workspace: workspace::Workspace::default(),
                fonts: Vec::new(),
                failure: None,
            }),
        }
    }
    pub fn with_config<T>(&self, f: impl FnOnce(&crate::config::Config) -> T) -> T {
        self.read(|a| f(&a.config))
    }
    pub fn with_session<T>(&self, f: impl FnOnce(&mut catalog::Session) -> T) -> T {
        self.write(|a| f(&mut a.session))
    }
    pub fn with_workspace<T>(&self, f: impl FnOnce(&mut workspace::Workspace) -> T) -> T {
        self.write(|a| f(&mut a.workspace))
    }
    pub fn with_sidebar<T>(&self, f: impl FnOnce(&mut sidebar::Sidebar) -> T) -> T {
        self.write(|a| f(&mut a.sidebar))
    }
    pub fn with_sources<T>(&self, f: impl FnOnce(&mut sources::Sources) -> T) -> T {
        self.write(|a| f(&mut a.sources))
    }
    pub fn with_status<T>(&self, f: impl FnOnce(&mut status::Status) -> T) -> T {
        self.write(|a| f(&mut a.status_state))
    }
    pub fn with_session_and_config<T>(
        &self,
        f: impl FnOnce(&mut catalog::Session, &crate::config::Config) -> T,
    ) -> T {
        self.write(|a| f(&mut a.session, &a.config))
    }
    pub fn with_settings<T>(&self, f: impl FnOnce(&mut settings::Settings) -> T) -> T {
        self.write(|a| f(&mut a.settings))
    }
    pub fn with_settings_and_config<T>(
        &self,
        f: impl FnOnce(&mut settings::Settings, &crate::config::Config) -> T,
    ) -> T {
        self.write(|a| f(&mut a.settings, &a.config))
    }
    pub fn frame(&self) -> HWND {
        self.read(|a| a.frame)
    }
    pub fn set_frame(&self, hwnd: HWND) {
        self.write(|a| a.frame = hwnd);
    }
    pub fn status_bar(&self) -> HWND {
        self.read(|a| a.status)
    }
    pub fn set_status_bar(&self, hwnd: HWND) {
        self.write(|a| a.status = hwnd);
    }
    pub fn set_icons(&self, icons: crate::icons::WindowIcons) {
        self.write(|a| a.icons = Some(icons));
    }
    pub fn take_fonts(&self) -> Vec<Font> {
        self.write(|a| std::mem::take(&mut a.fonts))
    }
    pub fn take_failure(&self) -> Option<anyhow::Error> {
        self.write(|a| a.failure.take())
    }
    // Closures only inspect/update owned data; never send messages inside them.
    fn read<T>(&self, f: impl FnOnce(&App) -> T) -> T {
        f(&self.state.borrow())
    }
    fn write<T>(&self, f: impl FnOnce(&mut App) -> T) -> T {
        f(&mut self.state.borrow_mut())
    }
    pub fn checked<T>(&self, result: anyhow::Result<T>) -> Option<T> {
        result
            .map_err(|error| self.write(|a| a.failure = Some(error)))
            .ok()
    }
    pub fn dpi(&self) -> Dpi {
        self.read(|a| a.dpi)
    }
    pub fn system_version(&self) -> crate::platform::WindowsVersion {
        self.read(|a| a.system_version)
    }
    pub fn font_for(&self, value: u32) -> (HFONT, i32) {
        if let Some(font) = self.read(|a| {
            a.fonts
                .iter()
                .find(|f| f.dpi == value)
                .map(|f| (f.handle.0, f.height))
        }) {
            return font;
        }
        let Some(font) = self.checked(Font::new(value)) else {
            // Window callbacks report errors at the application boundary.
            return (ptr::null_mut(), 0);
        };
        let result = (font.handle.0, font.height);
        self.write(|a| a.fonts.push(font));
        result
    }
    pub fn metrics(&self, hwnd: HWND) -> layout::Metrics {
        let dpi = self.dpi().window(hwnd);
        let (font, height) = self.font_for(dpi);
        layout::Metrics { dpi, font, height }
    }
    pub fn worker_event(&self) -> HANDLE {
        self.read(|a| a.worker.event())
    }
    pub fn replace_job(
        &self,
        previous: Option<u64>,
        ticket: Ticket,
        job: Job,
    ) -> anyhow::Result<()> {
        self.read(|a| a.worker.replace(previous, ticket, job))
    }
    pub fn cancel_job(&self, id: u64) {
        self.read(|a| a.worker.cancel(id));
    }
    pub fn try_completion(&self) -> Result<Completion, std::sync::mpsc::TryRecvError> {
        self.read(|a| a.worker.try_completion())
    }
    pub fn resignal_worker(&self) {
        self.read(|a| a.worker.resignal());
    }
    /// Resolve the context from a window created under the frame.
    ///
    /// Only the frame carries the pointer; MDI children, splitters and other
    /// controls reach it through their root window, so their own user data
    /// remains available to the control implementation.
    pub unsafe fn from_window(hwnd: HWND) -> Option<&'static Context> {
        let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
        let root = if root.is_null() { hwnd } else { root };
        let raw = unsafe { GetWindowLongPtrW(root, GWLP_USERDATA) } as *const Context;
        (!raw.is_null()).then(|| unsafe { &*raw })
    }
}
