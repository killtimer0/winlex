//! Owns UI-thread state and exposes it to native window procedures.
use super::App;
use crate::{
    dpi::{Dpi, Font},
    layout,
    native::*,
    tasks::{Completion, Job},
    worker::Ticket,
};
use std::{cell::RefCell, ptr};

pub(super) struct Context {
    state: RefCell<App>,
}
impl Context {
    pub fn new(app: App) -> Self {
        Self {
            state: RefCell::new(app),
        }
    }
    // Closures only inspect/update owned data; never send messages inside them.
    pub fn read<T>(&self, f: impl FnOnce(&App) -> T) -> T {
        f(&self.state.borrow())
    }
    pub fn write<T>(&self, f: impl FnOnce(&mut App) -> T) -> T {
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
        self.read(|a| a.worker.rx.try_recv())
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
