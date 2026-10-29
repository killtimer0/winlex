//! Immutable Windows version information captured at application startup.
use crate::native::error;
use windows_sys::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WindowsVersion {
    major: u32,
    minor: u32,
    build: u32,
}
impl WindowsVersion {
    pub const VISTA: Self = Self {
        major: 6,
        minor: 0,
        build: 0,
    };

    pub fn query() -> anyhow::Result<Self> {
        let mut version: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
        version.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
        if unsafe { GetVersionExW(&mut version) } == 0 {
            return Err(error("Read Windows version"));
        }
        Ok(Self {
            major: version.dwMajorVersion,
            minor: version.dwMinorVersion,
            build: version.dwBuildNumber,
        })
    }

    pub fn at_least(self, minimum: Self) -> bool {
        self >= minimum
    }
}
