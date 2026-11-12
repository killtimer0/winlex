//! Explicit activation of a registered class through `CoCreateInstance`.
//!
//! This runs arbitrary local code, so callers are expected to isolate it from
//! the application process. Only interfaces that can be queried without
//! generating proxies are read: `IDispatch` and `IInspectable`.
use super::{
    Guid,
    typelib::{self, TypeInfo},
    wide,
};
use serde::{Deserialize, Serialize};
use std::{ffi::c_void, fmt, ptr};
use windows_sys::{
    Win32::System::{
        Com::{
            CLSCTX_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
            CoTaskMemFree, CoUninitialize,
        },
        LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW},
    },
    core::{GUID, HRESULT},
};

const IID_UNKNOWN: GUID = GUID {
    data1: 0x0000_0000,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

const IID_DISPATCH: GUID = GUID {
    data1: 0x0002_0400,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

const IID_INSPECTABLE: GUID = GUID {
    data1: 0xAF86_E2E0,
    data2: 0xB12D,
    data3: 0x4C6A,
    data4: [0x9C, 0x5A, 0xD7, 0xAA, 0x65, 0x10, 0x1E, 0x90],
};

/// Everything read from one activated class.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub clsid: String,
    pub dispatch: Option<DispatchInfo>,
    pub inspectable: Option<InspectableInfo>,
}

/// Type information exposed by `IDispatch::GetTypeInfo`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchInfo {
    pub type_info: Option<TypeInfo>,
}

/// WinRT interface IDs and optional runtime class name from `IInspectable`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectableInfo {
    pub runtime_class: Option<String>,
    pub interfaces: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Initialize(HRESULT),
    Create(HRESULT),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Initialize(status) => {
                write!(f, "COM initialization failed (0x{:08X})", *status as u32)
            }
            Self::Create(status) => {
                write!(f, "Creating the instance failed (0x{:08X})", *status as u32)
            }
        }
    }
}

impl std::error::Error for Error {}

/// Creates `clsid` in the current apartment and reads its public interfaces.
pub fn inspect(clsid: Guid) -> Result<Instance, Error> {
    let _com = Com::initialize()?;
    let native = typelib::native_guid(clsid);
    let mut raw: *mut c_void = ptr::null_mut();
    let status = unsafe {
        CoCreateInstance(
            &native,
            ptr::null_mut(),
            CLSCTX_SERVER,
            &IID_UNKNOWN,
            &mut raw,
        )
    };
    if status < 0 {
        if !raw.is_null() {
            drop(Unknown(raw));
        }
        return Err(Error::Create(status));
    }
    if raw.is_null() {
        return Err(Error::Create(status));
    }
    let unknown = Unknown(raw);
    Ok(Instance {
        clsid: clsid.to_string(),
        dispatch: unknown
            .query(&IID_DISPATCH)
            .map(|raw| DispatchInfo::read(Dispatch(raw))),
        inspectable: unknown
            .query(&IID_INSPECTABLE)
            .map(|raw| InspectableInfo::read(Inspectable(raw))),
    })
}

struct Com;

impl Com {
    fn initialize() -> Result<Self, Error> {
        let status = unsafe { CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        if status < 0 {
            return Err(Error::Initialize(status));
        }
        Ok(Self)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

type QueryInterface =
    unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;
type AddRef = unsafe extern "system" fn(*mut c_void) -> u32;
type Release = unsafe extern "system" fn(*mut c_void) -> u32;

#[repr(C)]
struct UnknownVtable {
    query_interface: QueryInterface,
    add_ref: AddRef,
    release: Release,
}

struct Unknown(*mut c_void);

impl Unknown {
    fn vtable(&self) -> &UnknownVtable {
        unsafe { &**(self.0 as *const *const UnknownVtable) }
    }
    fn query(&self, iid: &GUID) -> Option<*mut c_void> {
        let mut raw: *mut c_void = ptr::null_mut();
        let status = unsafe { (self.vtable().query_interface)(self.0, iid, &mut raw) };
        (status >= 0 && !raw.is_null()).then_some(raw)
    }
}

impl Drop for Unknown {
    fn drop(&mut self) {
        unsafe { (self.vtable().release)(self.0) };
    }
}

#[repr(C)]
struct DispatchVtable {
    query_interface: QueryInterface,
    add_ref: AddRef,
    release: Release,
    get_type_info_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    get_type_info: unsafe extern "system" fn(*mut c_void, u32, u32, *mut *mut c_void) -> HRESULT,
}

struct Dispatch(*mut c_void);

impl Dispatch {
    fn vtable(&self) -> &DispatchVtable {
        unsafe { &**(self.0 as *const *const DispatchVtable) }
    }
}

impl Drop for Dispatch {
    fn drop(&mut self) {
        unsafe { (self.vtable().release)(self.0) };
    }
}

impl DispatchInfo {
    fn read(object: Dispatch) -> Self {
        let vtable = object.vtable();
        let mut count = 0u32;
        let status = unsafe { (vtable.get_type_info_count)(object.0, &mut count) };
        let type_info = if status >= 0 && count > 0 {
            let mut raw: *mut c_void = ptr::null_mut();
            let status = unsafe { (vtable.get_type_info)(object.0, 0, 0, &mut raw) };
            if status >= 0 {
                typelib::expand_type_info(raw)
            } else {
                None
            }
        } else {
            None
        };
        Self { type_info }
    }
}

#[repr(C)]
struct InspectableVtable {
    query_interface: QueryInterface,
    add_ref: AddRef,
    release: Release,
    get_iids: unsafe extern "system" fn(*mut c_void, *mut u32, *mut *mut GUID) -> HRESULT,
    get_runtime_class_name: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    get_trust_level: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT,
}

struct Inspectable(*mut c_void);

impl Inspectable {
    fn vtable(&self) -> &InspectableVtable {
        unsafe { &**(self.0 as *const *const InspectableVtable) }
    }
}

impl Drop for Inspectable {
    fn drop(&mut self) {
        unsafe { (self.vtable().release)(self.0) };
    }
}

impl InspectableInfo {
    fn read(object: Inspectable) -> Self {
        let vtable = object.vtable();
        let mut interfaces = Vec::new();
        let mut count = 0u32;
        let mut iids: *mut GUID = ptr::null_mut();
        let status = unsafe { (vtable.get_iids)(object.0, &mut count, &mut iids) };
        if status >= 0 && !iids.is_null() {
            for index in 0..count as usize {
                let guid = unsafe { &*iids.add(index) };
                if let Some(value) = typelib::guid_text(guid) {
                    interfaces.push(value);
                }
            }
            unsafe { CoTaskMemFree(iids.cast()) };
        }
        let runtime_class = hstring().and_then(|api| unsafe {
            let mut value: *mut c_void = ptr::null_mut();
            let status = (vtable.get_runtime_class_name)(object.0, &mut value);
            if status < 0 || value.is_null() {
                return None;
            }
            let mut length = 0u32;
            let buffer = (api.get_raw)(value, &mut length);
            let text = if buffer.is_null() || length == 0 {
                None
            } else {
                Some(String::from_utf16_lossy(std::slice::from_raw_parts(
                    buffer,
                    length as usize,
                )))
            };
            (api.delete)(value);
            text
        });
        Self {
            runtime_class,
            interfaces,
        }
    }
}

type GetRaw = unsafe extern "system" fn(*const c_void, *mut u32) -> *const u16;
type Delete = unsafe extern "system" fn(*const c_void) -> HRESULT;

struct Hstring {
    get_raw: GetRaw,
    delete: Delete,
}

/// `combase.dll` is absent before Windows 8, so its exports are resolved lazily.
fn hstring() -> Option<Hstring> {
    let name = wide("combase.dll");
    unsafe {
        let mut module = GetModuleHandleW(name.as_ptr());
        if module.is_null() {
            module = LoadLibraryW(name.as_ptr());
        }
        if module.is_null() {
            return None;
        }
        let get_raw = GetProcAddress(module, c"WindowsGetStringRawBuffer".as_ptr().cast())?;
        let delete = GetProcAddress(module, c"WindowsDeleteString".as_ptr().cast())?;
        Some(Hstring {
            get_raw: std::mem::transmute::<unsafe extern "system" fn() -> isize, GetRaw>(get_raw),
            delete: std::mem::transmute::<unsafe extern "system" fn() -> isize, Delete>(delete),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_iids_match_the_documented_text() {
        assert_eq!(
            typelib::guid_text(&IID_UNKNOWN).as_deref(),
            Some("{00000000-0000-0000-C000-000000000046}")
        );
        assert_eq!(
            typelib::guid_text(&IID_DISPATCH).as_deref(),
            Some("{00020400-0000-0000-C000-000000000046}")
        );
        assert_eq!(
            typelib::guid_text(&IID_INSPECTABLE).as_deref(),
            Some("{AF86E2E0-B12D-4C6A-9C5A-D7AA65101E90}")
        );
    }
}
