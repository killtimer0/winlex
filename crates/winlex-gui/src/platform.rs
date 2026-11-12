//! Immutable Windows version information captured at application startup.
use windows_sys::{
    Wdk::System::SystemServices::RtlGetVersion, Win32::System::SystemInformation::OSVERSIONINFOW,
};

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
    pub const DARK_MODE: Self = Self {
        major: 10,
        minor: 0,
        build: 17763,
    };
    pub const WINDOWS_11: Self = Self {
        major: 10,
        minor: 0,
        build: 22000,
    };

    pub fn query() -> anyhow::Result<Self> {
        let mut version: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
        version.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
        // GetVersionEx is manifest-dependent; capability thresholds need the
        // actual OS build, including when running with our XP-compatible manifest.
        let status = unsafe { RtlGetVersion(&mut version) };
        anyhow::ensure!(status >= 0, "Read Windows version failed: {status:#x}");
        Ok(Self {
            major: version.dwMajorVersion,
            minor: version.dwMinorVersion,
            build: version.dwBuildNumber,
        })
    }

    pub fn build(self) -> u32 {
        self.build
    }
}
