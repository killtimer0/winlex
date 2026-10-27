use crate::native::*;
use std::{ptr, str::FromStr};
use windows_sys::Win32::UI::HiDpi::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Pmv2,
    Pmv1,
    System,
    Unaware,
}
impl FromStr for Mode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s {
            "auto" => Self::Auto,
            "pmv2" => Self::Pmv2,
            "pmv1" => Self::Pmv1,
            "system" => Self::System,
            "unaware" => Self::Unaware,
            _ => return Err(format!("Unknown DPI mode: {s}")),
        })
    }
}

#[derive(Clone, Copy)]
pub struct Dpi {
    pub mode: Mode,
}
impl Dpi {
    pub fn initialize(request: Mode) -> Self {
        unsafe {
            SetProcessDpiAwarenessContext(match request {
                Mode::Auto | Mode::Pmv2 => DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                Mode::Pmv1 => DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
                Mode::System => DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
                Mode::Unaware => DPI_AWARENESS_CONTEXT_UNAWARE,
            });
            // The thunk reports the effective process awareness on downlevel systems,
            // never PMv2 merely because a PMv2 request was successfully downgraded.
            let context = GetThreadDpiAwarenessContext();
            let mode = if AreDpiAwarenessContextsEqual(
                context,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            ) != 0
            {
                Mode::Pmv2
            } else {
                match GetAwarenessFromDpiAwarenessContext(context) {
                    DPI_AWARENESS_PER_MONITOR_AWARE => Mode::Pmv1,
                    DPI_AWARENESS_SYSTEM_AWARE => Mode::System,
                    _ => Mode::Unaware,
                }
            };
            Self { mode }
        }
    }
    pub fn per_monitor(self) -> bool {
        matches!(self.mode, Mode::Pmv1 | Mode::Pmv2)
    }
    pub fn system(self) -> u32 {
        unsafe { GetDpiForSystem() }
    }
    pub fn window(self, hwnd: HWND) -> u32 {
        unsafe { GetDpiForWindow(hwnd) }
    }
    pub fn enable_nonclient(self, hwnd: HWND) {
        if self.mode == Mode::Pmv1 {
            unsafe {
                EnableNonClientDpiScaling(hwnd);
            }
        }
    }
    pub fn child_outer(self, hwnd: HWND, mut r: RECT) -> RECT {
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            // PMv1 cannot enable per-monitor nonclient scaling on child windows.
            if self.mode == Mode::Pmv2 {
                AdjustWindowRectExForDpi(&mut r, style, 0, ex, self.window(hwnd));
            } else {
                AdjustWindowRectEx(&mut r, style, 0, ex);
            }
        }
        r
    }
}

pub struct Font {
    pub dpi: u32,
    pub handle: HFONT,
    pub height: i32,
}
impl Font {
    pub fn new(dpi: u32) -> Result<Self, String> {
        unsafe {
            let mut ncm: NONCLIENTMETRICSW = std::mem::zeroed();
            ncm.cbSize = std::mem::size_of::<NONCLIENTMETRICSW>() as u32;
            if SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS,
                ncm.cbSize,
                (&mut ncm as *mut NONCLIENTMETRICSW).cast(),
                0,
                dpi,
            ) == 0
            {
                return Err(error("Read system font"));
            }
            let handle = CreateFontIndirectW(&ncm.lfMessageFont);
            if handle.is_null() {
                return Err(error("Create system font"));
            }
            let mut font = Self {
                dpi,
                handle,
                height: 0,
            };
            let dc = CreateCompatibleDC(ptr::null_mut());
            if dc.is_null() {
                return Err(error("Create font measurement DC"));
            }
            let previous = SelectObject(dc, handle);
            let mut metrics: TEXTMETRICW = std::mem::zeroed();
            let success = GetTextMetricsW(dc, &mut metrics);
            let failure = (success == 0).then(|| error("Measure system font"));
            SelectObject(dc, previous);
            DeleteDC(dc);
            if let Some(failure) = failure {
                return Err(failure);
            }
            font.height = metrics.tmHeight;
            Ok(font)
        }
    }
}
impl Drop for Font {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.handle);
        }
    }
}
