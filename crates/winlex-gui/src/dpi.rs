use crate::native::*;
use std::{ffi::c_void, ptr, str::FromStr};

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
type GetWindow = unsafe extern "system" fn(HWND) -> u32;
type GetMonitor = unsafe extern "system" fn(HMONITOR, i32, *mut u32, *mut u32) -> i32;
type EnableNc = unsafe extern "system" fn(HWND) -> BOOL;
type Adjust = unsafe extern "system" fn(*mut RECT, u32, BOOL, u32, u32) -> BOOL;

#[derive(Clone, Copy)]
pub struct Dpi {
    pub mode: Mode,
    pub root: u32,
    get_window: Option<GetWindow>,
    get_monitor: Option<GetMonitor>,
    enable_nc: Option<EnableNc>,
    adjust: Option<Adjust>,
}
impl Dpi {
    pub fn initialize(request: Mode) -> Result<Self, String> {
        unsafe {
            let user = GetModuleHandleW(wide("user32.dll").as_ptr());
            let shcore = LoadLibraryW(wide("shcore.dll").as_ptr());
            let set_context = procedure::<unsafe extern "system" fn(*mut c_void) -> BOOL>(
                user,
                b"SetProcessDpiAwarenessContext\0",
            );
            let set_process = procedure::<unsafe extern "system" fn(i32) -> i32>(
                shcore,
                b"SetProcessDpiAwareness\0",
            );
            let set_system =
                procedure::<unsafe extern "system" fn() -> BOOL>(user, b"SetProcessDPIAware\0");
            let candidates: &[Mode] = if request == Mode::Auto {
                &[Mode::Pmv2, Mode::Pmv1, Mode::System]
            } else {
                std::slice::from_ref(&request)
            };
            let mut actual = None;
            for &mode in candidates {
                let context = match mode {
                    Mode::Pmv2 => -4isize,
                    Mode::Pmv1 => -3,
                    Mode::System => -2,
                    _ => -1,
                };
                let success = if let Some(set) = set_context {
                    set(context as *mut c_void) != 0
                } else if mode == Mode::Pmv2 {
                    false
                } else if let Some(set) = set_process {
                    set(match mode {
                        Mode::Pmv1 => 2,
                        Mode::System => 1,
                        _ => 0,
                    }) >= 0
                } else {
                    match mode {
                        Mode::Pmv1 => false,
                        Mode::System => set_system.is_none_or(|set| set() != 0),
                        Mode::Unaware => true,
                        _ => false,
                    }
                };
                if success {
                    actual = Some(mode);
                    break;
                }
                // A mode already fixed by the host/compatibility settings is not a fallback opportunity.
                if set_context.is_some() && GetLastError() == ERROR_ACCESS_DENIED {
                    break;
                }
            }
            let mode = actual
                .ok_or_else(|| format!("Cannot activate DPI mode {request:?} on this system"))?;
            let dc = GetDC(ptr::null_mut());
            let system = if dc.is_null() {
                96
            } else {
                GetDeviceCaps(dc, LOGPIXELSX as i32).max(96) as u32
            };
            if !dc.is_null() {
                ReleaseDC(ptr::null_mut(), dc);
            }
            Ok(Self {
                mode,
                root: if mode == Mode::Unaware { 96 } else { system },
                get_window: procedure(user, b"GetDpiForWindow\0"),
                get_monitor: procedure(shcore, b"GetDpiForMonitor\0"),
                enable_nc: procedure(user, b"EnableNonClientDpiScaling\0"),
                adjust: procedure(user, b"AdjustWindowRectExForDpi\0"),
            })
        }
    }
    pub fn per_monitor(self) -> bool {
        matches!(self.mode, Mode::Pmv1 | Mode::Pmv2)
    }
    pub fn window(self, hwnd: HWND) -> u32 {
        unsafe {
            if let Some(get) = self.get_window {
                let dpi = get(hwnd);
                if dpi != 0 {
                    return dpi;
                }
            }
        }
        self.root
    }
    pub fn initial(self, hwnd: HWND) -> u32 {
        if self.get_window.is_some() || !self.per_monitor() {
            return self.window(hwnd);
        }
        if let Some(get) = self.get_monitor {
            let (mut x, mut y) = (0, 0);
            if unsafe {
                get(
                    MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                    0,
                    &mut x,
                    &mut y,
                )
            } >= 0
                && x != 0
            {
                return x;
            }
        }
        self.root
    }
    pub fn enable_nonclient(self, hwnd: HWND) {
        if self.mode == Mode::Pmv1
            && let Some(enable) = self.enable_nc
        {
            unsafe {
                enable(hwnd);
            }
        }
    }
    pub fn child_outer(self, hwnd: HWND, mut r: RECT) -> RECT {
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            // PMv1 cannot enable per-monitor nonclient scaling on child windows.
            if self.mode == Mode::Pmv2
                && let Some(adjust) = self.adjust
            {
                adjust(&mut r, style, 0, ex, self.window(hwnd));
                return r;
            }
            AdjustWindowRectEx(&mut r, style, 0, ex);
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
    pub fn new(dpi: u32) -> Self {
        unsafe {
            // Message font follows the user's system font settings, including pre-Vista metrics.
            let mut ncm: NONCLIENTMETRICSW = std::mem::zeroed();
            ncm.cbSize = std::mem::size_of::<NONCLIENTMETRICSW>() as u32;
            if SystemParametersInfoW(
                SPI_GETNONCLIENTMETRICS,
                ncm.cbSize,
                (&mut ncm as *mut NONCLIENTMETRICSW).cast(),
                0,
            ) == 0
            {
                ncm.cbSize -= 4;
                SystemParametersInfoW(
                    SPI_GETNONCLIENTMETRICS,
                    ncm.cbSize,
                    (&mut ncm as *mut NONCLIENTMETRICSW).cast(),
                    0,
                );
            }
            let dc = GetDC(ptr::null_mut());
            let system = if dc.is_null() {
                96
            } else {
                GetDeviceCaps(dc, LOGPIXELSX as i32).max(1)
            };
            let mut lf = ncm.lfMessageFont;
            if lf.lfHeight == 0 {
                lf.lfHeight = -12;
            }
            lf.lfHeight = MulDiv(lf.lfHeight, dpi as i32, system);
            lf.lfWidth = MulDiv(lf.lfWidth, dpi as i32, system);
            let handle = CreateFontIndirectW(&lf);
            let mut metrics: TEXTMETRICW = std::mem::zeroed();
            let previous = if !dc.is_null() {
                SelectObject(dc, handle)
            } else {
                ptr::null_mut()
            };
            if !dc.is_null() {
                GetTextMetricsW(dc, &mut metrics);
                SelectObject(dc, previous);
                ReleaseDC(ptr::null_mut(), dc);
            }
            Self {
                dpi,
                handle,
                height: metrics.tmHeight.max(-lf.lfHeight),
            }
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
