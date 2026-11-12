//! Read-only expansion of registered type libraries through `ITypeLib`.
use super::Guid;
use serde::{Deserialize, Serialize};
use std::{ffi::c_void, fmt, ptr};
use windows_sys::Win32::Foundation::{SysFreeString, SysStringLen, TYPE_E_LIBNOTREGISTERED};
use windows_sys::Win32::System::Com::{
    CC_CDECL, CC_FASTCALL, CC_FPFASTCALL, CC_MACPASCAL, CC_PASCAL, CC_STDCALL, CC_SYSCALL,
    FUNCDESC, IMPLTYPEFLAG_FDEFAULT, IMPLTYPEFLAG_FDEFAULTVTABLE, IMPLTYPEFLAG_FRESTRICTED,
    IMPLTYPEFLAG_FSOURCE, INVOKE_FUNC, INVOKE_PROPERTYGET, INVOKE_PROPERTYPUT,
    INVOKE_PROPERTYPUTREF, TKIND_ALIAS, TKIND_COCLASS, TKIND_DISPATCH, TKIND_ENUM, TKIND_INTERFACE,
    TKIND_MODULE, TKIND_RECORD, TKIND_UNION, TYPEATTR, VARDESC,
};
use windows_sys::Win32::System::Ole::{
    LoadRegTypeLib, MEMBERID_NIL, PARAMFLAG_FIN, PARAMFLAG_FOPT, PARAMFLAG_FOUT, PARAMFLAG_FRETVAL,
};
use windows_sys::Win32::System::Variant::{
    VARENUM, VT_ARRAY, VT_BLOB, VT_BOOL, VT_BSTR, VT_BYREF, VT_CARRAY, VT_CF, VT_CLSID, VT_CY,
    VT_DATE, VT_DECIMAL, VT_DISPATCH, VT_EMPTY, VT_ERROR, VT_FILETIME, VT_HRESULT, VT_I1, VT_I2,
    VT_I4, VT_I8, VT_INT, VT_INT_PTR, VT_LPSTR, VT_LPWSTR, VT_NULL, VT_PTR, VT_R4, VT_R8,
    VT_RECORD, VT_SAFEARRAY, VT_STORAGE, VT_STORED_OBJECT, VT_STREAM, VT_STREAMED_OBJECT, VT_UI1,
    VT_UI2, VT_UI4, VT_UI8, VT_UINT, VT_UINT_PTR, VT_UNKNOWN, VT_USERDEFINED, VT_VARIANT, VT_VOID,
};
use windows_sys::core::{BSTR, GUID, HRESULT};

/// A type library loaded from the registration of one LIBID and version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeLibrary {
    pub guid: String,
    pub major: u16,
    pub minor: u16,
    pub name: String,
    pub types: Vec<TypeInfo>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeInfo {
    pub name: String,
    pub kind: TypeKind,
    pub guid: Option<String>,
    pub implemented: Vec<Implemented>,
    pub functions: Vec<Function>,
    pub variables: Vec<Variable>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeKind {
    Enum,
    Record,
    Module,
    Interface,
    Dispatch,
    Coclass,
    Alias,
    Union,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Implemented {
    pub name: String,
    pub guid: Option<String>,
    pub source: bool,
    pub default: bool,
    pub restricted: bool,
    pub default_vtable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function {
    pub name: String,
    pub member: MemberKind,
    pub convention: Convention,
    pub returns: String,
    pub parameters: Vec<Parameter>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberKind {
    #[default]
    Function,
    PropertyGet,
    PropertyPut,
    PropertyPutRef,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Convention {
    #[default]
    Unknown,
    Cdecl,
    Stdcall,
    Fastcall,
    Pascal,
    Syscall,
    MacPascal,
    FpFastcall,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub ty: String,
    pub direction: Direction,
    pub optional: bool,
    pub return_value: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    In,
    Out,
    InOut,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    pub ty: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    NotRegistered,
    Failure(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRegistered => f.write_str("the type library is not registered"),
            Self::Failure(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

/// Loads and expands the type library registered for `guid` using the system default locale.
pub fn load(guid: Guid, major: u16, minor: u16) -> Result<TypeLibrary, Error> {
    let native = native_guid(guid);
    let mut raw: *mut c_void = ptr::null_mut();
    let status = unsafe { LoadRegTypeLib(&native, major, minor, 0, &mut raw) };
    if status < 0 {
        return Err(if status == TYPE_E_LIBNOTREGISTERED {
            Error::NotRegistered
        } else {
            Error::Failure(format!("LoadRegTypeLib failed (0x{:08X})", status as u32))
        });
    }
    if raw.is_null() {
        return Err(Error::Failure("LoadRegTypeLib returned no library".into()));
    }
    let library = TypeLib(raw);
    let name = library.documentation(-1).unwrap_or_default();
    let count = library.count();
    let mut types = Vec::with_capacity(count as usize);
    for index in 0..count {
        if let Some(info) = library.type_info(index) {
            types.push(info.expand());
        }
    }
    Ok(TypeLibrary {
        guid: guid.to_string(),
        major,
        minor,
        name,
        types,
    })
}

fn guid_bytes(guid: GUID) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[..4].copy_from_slice(&guid.data1.to_be_bytes());
    bytes[4..6].copy_from_slice(&guid.data2.to_be_bytes());
    bytes[6..8].copy_from_slice(&guid.data3.to_be_bytes());
    bytes[8..].copy_from_slice(&guid.data4);
    bytes
}

pub(super) fn native_guid(guid: Guid) -> GUID {
    let bytes = guid.0;
    GUID {
        data1: u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        data2: u16::from_be_bytes([bytes[4], bytes[5]]),
        data3: u16::from_be_bytes([bytes[6], bytes[7]]),
        data4: [
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ],
    }
}

pub(super) fn guid_text(guid: &GUID) -> Option<String> {
    let bytes = guid_bytes(*guid);
    (bytes != [0; 16]).then(|| Guid(bytes).to_string())
}

fn take_bstr(value: BSTR) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let text = unsafe {
        let length = SysStringLen(value) as usize;
        let units = std::slice::from_raw_parts(value, length);
        String::from_utf16_lossy(units)
    };
    unsafe { SysFreeString(value) };
    (!text.is_empty()).then_some(text)
}

#[repr(C)]
struct TypeLibVtable {
    query_interface: *const c_void,
    add_ref: *const c_void,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_type_info_count: unsafe extern "system" fn(*mut c_void) -> u32,
    get_type_info: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT,
    get_type_info_type: *const c_void,
    get_type_info_of_guid: *const c_void,
    get_lib_attr: *const c_void,
    get_type_comp: *const c_void,
    get_documentation: unsafe extern "system" fn(
        *mut c_void,
        i32,
        *mut BSTR,
        *mut BSTR,
        *mut u32,
        *mut BSTR,
    ) -> HRESULT,
    is_name: *const c_void,
    find_name: *const c_void,
    release_tlib_attr: *const c_void,
}

struct TypeLib(*mut c_void);

impl TypeLib {
    fn vtable(&self) -> &TypeLibVtable {
        unsafe { &**(self.0 as *const *const TypeLibVtable) }
    }
    fn count(&self) -> u32 {
        unsafe { (self.vtable().get_type_info_count)(self.0) }
    }
    fn type_info(&self, index: u32) -> Option<RawTypeInfo> {
        let mut raw: *mut c_void = ptr::null_mut();
        let status = unsafe { (self.vtable().get_type_info)(self.0, index, &mut raw) };
        (status >= 0 && !raw.is_null()).then_some(RawTypeInfo(raw))
    }
    fn documentation(&self, index: i32) -> Option<String> {
        documentation(self.0, self.vtable().get_documentation, index)
    }
}

impl Drop for TypeLib {
    fn drop(&mut self) {
        unsafe { (self.vtable().release)(self.0) };
    }
}

type GetDocumentation = unsafe extern "system" fn(
    *mut c_void,
    i32,
    *mut BSTR,
    *mut BSTR,
    *mut u32,
    *mut BSTR,
) -> HRESULT;

fn documentation(
    this: *mut c_void,
    get_documentation: GetDocumentation,
    member: i32,
) -> Option<String> {
    let mut name: BSTR = ptr::null();
    unsafe {
        (get_documentation)(
            this,
            member,
            &mut name,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        );
    }
    take_bstr(name)
}

#[repr(C)]
struct TypeInfoVtable {
    query_interface: *const c_void,
    add_ref: *const c_void,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_type_attr: unsafe extern "system" fn(*mut c_void, *mut *mut TYPEATTR) -> HRESULT,
    get_type_comp: *const c_void,
    get_func_desc: unsafe extern "system" fn(*mut c_void, u32, *mut *mut FUNCDESC) -> HRESULT,
    get_var_desc: unsafe extern "system" fn(*mut c_void, u32, *mut *mut VARDESC) -> HRESULT,
    get_names: unsafe extern "system" fn(*mut c_void, i32, *mut BSTR, u32, *mut u32) -> HRESULT,
    get_ref_type_of_impl_type: unsafe extern "system" fn(*mut c_void, u32, *mut u32) -> HRESULT,
    get_impl_type_flags: unsafe extern "system" fn(*mut c_void, u32, *mut i32) -> HRESULT,
    get_ids_of_names: *const c_void,
    invoke: *const c_void,
    get_documentation: GetDocumentation,
    get_dll_entry: *const c_void,
    get_ref_type_info: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT,
    address_of_member: *const c_void,
    create_instance: *const c_void,
    get_mops: *const c_void,
    get_containing_type_lib: *const c_void,
    release_type_attr: unsafe extern "system" fn(*mut c_void, *mut TYPEATTR),
    release_func_desc: unsafe extern "system" fn(*mut c_void, *mut FUNCDESC),
    release_var_desc: unsafe extern "system" fn(*mut c_void, *mut VARDESC),
}

struct RawTypeInfo(*mut c_void);

impl RawTypeInfo {
    fn vtable(&self) -> &TypeInfoVtable {
        unsafe { &**(self.0 as *const *const TypeInfoVtable) }
    }
    fn documentation(&self, member: i32) -> Option<String> {
        documentation(self.0, self.vtable().get_documentation, member)
    }
    fn attr(&self) -> Option<Attr> {
        let mut raw: *mut TYPEATTR = ptr::null_mut();
        let status = unsafe { (self.vtable().get_type_attr)(self.0, &mut raw) };
        if status < 0 || raw.is_null() {
            return None;
        }
        let attr = unsafe { &*raw };
        let snapshot = Attr {
            guid: attr.guid,
            kind: attr.typekind,
            functions: attr.cFuncs,
            variables: attr.cVars,
            implemented: attr.cImplTypes,
        };
        unsafe { (self.vtable().release_type_attr)(self.0, raw) };
        Some(snapshot)
    }
    fn names(&self, member: i32, count: usize) -> Vec<String> {
        if count == 0 {
            return Vec::new();
        }
        let mut values: Vec<BSTR> = vec![ptr::null(); count];
        let mut fetched = 0u32;
        let status = unsafe {
            (self.vtable().get_names)(
                self.0,
                member,
                values.as_mut_ptr(),
                count as u32,
                &mut fetched,
            )
        };
        if status < 0 {
            for value in values {
                if !value.is_null() {
                    unsafe { SysFreeString(value) };
                }
            }
            return Vec::new();
        }
        values
            .into_iter()
            .take(fetched as usize)
            .filter_map(take_bstr)
            .collect()
    }
    fn function(&self, index: u32) -> Option<Function> {
        let vtable = self.vtable();
        let mut raw: *mut FUNCDESC = ptr::null_mut();
        let status = unsafe { (vtable.get_func_desc)(self.0, index, &mut raw) };
        if status < 0 || raw.is_null() {
            return None;
        }
        let desc = unsafe { &*raw };
        let param_count = desc.cParams.max(0) as usize;
        let names = self.names(desc.memid, param_count + 1);
        let parameters = (0..param_count)
            .map(|index| {
                let element = unsafe { &*desc.lprgelemdescParam.add(index) };
                let flags = unsafe { element.Anonymous.paramdesc.wParamFlags };
                Parameter {
                    name: names.get(index + 1).cloned().unwrap_or_default(),
                    ty: self.type_name(&element.tdesc, 0),
                    direction: direction(flags),
                    optional: flags & PARAMFLAG_FOPT != 0,
                    return_value: flags & PARAMFLAG_FRETVAL != 0,
                }
            })
            .collect();
        let function = Function {
            name: names
                .first()
                .cloned()
                .unwrap_or_else(|| format!("member {}", desc.memid)),
            member: member_kind(desc.invkind),
            convention: convention(desc.callconv),
            returns: self.type_name(&desc.elemdescFunc.tdesc, 0),
            parameters,
        };
        unsafe { (vtable.release_func_desc)(self.0, raw) };
        Some(function)
    }
    fn variable(&self, index: u32) -> Option<Variable> {
        let vtable = self.vtable();
        let mut raw: *mut VARDESC = ptr::null_mut();
        let status = unsafe { (vtable.get_var_desc)(self.0, index, &mut raw) };
        if status < 0 || raw.is_null() {
            return None;
        }
        let desc = unsafe { &*raw };
        let names = self.names(desc.memid, 1);
        let variable = Variable {
            name: names
                .first()
                .cloned()
                .unwrap_or_else(|| format!("member {}", desc.memid)),
            ty: self.type_name(&desc.elemdescVar.tdesc, 0),
        };
        unsafe { (vtable.release_var_desc)(self.0, raw) };
        Some(variable)
    }
    fn implemented(&self, index: u32) -> Option<Implemented> {
        let vtable = self.vtable();
        let mut href = 0u32;
        let status = unsafe { (vtable.get_ref_type_of_impl_type)(self.0, index, &mut href) };
        if status < 0 {
            return None;
        }
        let mut flags = 0i32;
        unsafe { (vtable.get_impl_type_flags)(self.0, index, &mut flags) };
        let info = self.ref_type_info(href)?;
        let name = info
            .documentation(MEMBERID_NIL)
            .unwrap_or_else(|| "?".into());
        let guid = info.attr().and_then(|attr| guid_text(&attr.guid));
        Some(Implemented {
            name,
            guid,
            source: flags & IMPLTYPEFLAG_FSOURCE != 0,
            default: flags & IMPLTYPEFLAG_FDEFAULT != 0,
            restricted: flags & IMPLTYPEFLAG_FRESTRICTED != 0,
            default_vtable: flags & IMPLTYPEFLAG_FDEFAULTVTABLE != 0,
        })
    }
    fn ref_type_info(&self, href: u32) -> Option<RawTypeInfo> {
        let mut raw: *mut c_void = ptr::null_mut();
        let status = unsafe { (self.vtable().get_ref_type_info)(self.0, href, &mut raw) };
        (status >= 0 && !raw.is_null()).then_some(RawTypeInfo(raw))
    }
    fn ref_type_name(&self, href: u32) -> Option<String> {
        self.ref_type_info(href)?.documentation(MEMBERID_NIL)
    }
    fn type_name(&self, desc: &windows_sys::Win32::System::Com::TYPEDESC, depth: u32) -> String {
        if depth > 8 {
            return "...".into();
        }
        let anonymous = &desc.Anonymous;
        match desc.vt {
            VT_PTR => {
                let inner = unsafe { anonymous.lptdesc };
                if inner.is_null() {
                    "void*".into()
                } else {
                    format!("{}*", self.type_name(unsafe { &*inner }, depth + 1))
                }
            }
            VT_USERDEFINED => self
                .ref_type_name(unsafe { anonymous.hreftype })
                .unwrap_or_else(|| "typedef".into()),
            VT_SAFEARRAY => "SAFEARRAY".into(),
            VT_ARRAY | VT_CARRAY => {
                let array = unsafe { anonymous.lpadesc };
                if array.is_null() {
                    "array".into()
                } else {
                    let array = unsafe { &*array };
                    format!(
                        "{}[{}]",
                        self.type_name(&array.tdescElem, depth + 1),
                        array.cDims
                    )
                }
            }
            VT_BYREF => "reference".into(),
            vt => base_name(vt),
        }
    }
    fn expand(&self) -> TypeInfo {
        let name = self.documentation(MEMBERID_NIL);
        let Some(attr) = self.attr() else {
            return TypeInfo {
                name: name.unwrap_or_default(),
                ..TypeInfo::default()
            };
        };
        let implemented = (0..u32::from(attr.implemented))
            .filter_map(|index| self.implemented(index))
            .collect();
        let functions = (0..u32::from(attr.functions))
            .filter_map(|index| self.function(index))
            .collect();
        let variables = (0..u32::from(attr.variables))
            .filter_map(|index| self.variable(index))
            .collect();
        TypeInfo {
            name: name.unwrap_or_default(),
            kind: type_kind(attr.kind),
            guid: guid_text(&attr.guid),
            implemented,
            functions,
            variables,
        }
    }
}

impl Drop for RawTypeInfo {
    fn drop(&mut self) {
        unsafe { (self.vtable().release)(self.0) };
    }
}

/// Expands a type info pointer and takes ownership of its reference.
pub(super) fn expand_type_info(raw: *mut c_void) -> Option<TypeInfo> {
    if raw.is_null() {
        return None;
    }
    Some(RawTypeInfo(raw).expand())
}

struct Attr {
    guid: GUID,
    kind: i32,
    functions: u16,
    variables: u16,
    implemented: u16,
}

fn base_name(vt: VARENUM) -> String {
    match vt {
        VT_EMPTY => "empty".into(),
        VT_NULL => "null".into(),
        VT_I2 => "short".into(),
        VT_I4 => "long".into(),
        VT_R4 => "float".into(),
        VT_R8 => "double".into(),
        VT_CY => "CURRENCY".into(),
        VT_DATE => "DATE".into(),
        VT_BSTR => "BSTR".into(),
        VT_DISPATCH => "IDispatch*".into(),
        VT_ERROR => "SCODE".into(),
        VT_BOOL => "VARIANT_BOOL".into(),
        VT_VARIANT => "VARIANT".into(),
        VT_UNKNOWN => "IUnknown*".into(),
        VT_DECIMAL => "DECIMAL".into(),
        VT_I1 => "char".into(),
        VT_UI1 => "byte".into(),
        VT_UI2 => "unsigned short".into(),
        VT_UI4 => "unsigned long".into(),
        VT_I8 => "long long".into(),
        VT_UI8 => "unsigned long long".into(),
        VT_INT => "int".into(),
        VT_UINT => "unsigned int".into(),
        VT_VOID => "void".into(),
        VT_HRESULT => "HRESULT".into(),
        VT_LPSTR => "char*".into(),
        VT_LPWSTR => "wchar_t*".into(),
        VT_FILETIME => "FILETIME".into(),
        VT_BLOB => "BLOB".into(),
        VT_STREAM | VT_STREAMED_OBJECT => "IStream*".into(),
        VT_STORAGE | VT_STORED_OBJECT => "IStorage*".into(),
        VT_CF => "CLIPFORMAT".into(),
        VT_CLSID => "CLSID".into(),
        VT_INT_PTR => "INT_PTR".into(),
        VT_UINT_PTR => "UINT_PTR".into(),
        VT_RECORD => "record".into(),
        other => format!("VT_{other}"),
    }
}

fn member_kind(invoke: i32) -> MemberKind {
    match invoke {
        INVOKE_FUNC => MemberKind::Function,
        INVOKE_PROPERTYGET => MemberKind::PropertyGet,
        INVOKE_PROPERTYPUT => MemberKind::PropertyPut,
        INVOKE_PROPERTYPUTREF => MemberKind::PropertyPutRef,
        _ => MemberKind::Unknown,
    }
}

fn convention(value: i32) -> Convention {
    match value {
        CC_CDECL => Convention::Cdecl,
        CC_STDCALL => Convention::Stdcall,
        CC_FASTCALL => Convention::Fastcall,
        CC_PASCAL => Convention::Pascal,
        CC_SYSCALL => Convention::Syscall,
        CC_MACPASCAL => Convention::MacPascal,
        CC_FPFASTCALL => Convention::FpFastcall,
        _ => Convention::Unknown,
    }
}

fn direction(flags: u16) -> Direction {
    match (flags & PARAMFLAG_FIN != 0, flags & PARAMFLAG_FOUT != 0) {
        (true, true) => Direction::InOut,
        (true, false) => Direction::In,
        (false, true) => Direction::Out,
        (false, false) => Direction::Unknown,
    }
}

fn type_kind(value: i32) -> TypeKind {
    match value {
        TKIND_ENUM => TypeKind::Enum,
        TKIND_RECORD => TypeKind::Record,
        TKIND_MODULE => TypeKind::Module,
        TKIND_INTERFACE => TypeKind::Interface,
        TKIND_DISPATCH => TypeKind::Dispatch,
        TKIND_COCLASS => TypeKind::Coclass,
        TKIND_ALIAS => TypeKind::Alias,
        TKIND_UNION => TypeKind::Union,
        _ => TypeKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STD_OLE: &str = "{00020430-0000-0000-C000-000000000046}";

    #[test]
    fn expands_the_standard_ole_type_library() {
        let guid = Guid::parse(STD_OLE).expect("GUID");
        let library = match load(guid, 2, 0) {
            Ok(library) => library,
            Err(Error::NotRegistered) => return,
            Err(error) => panic!("expand stdole2: {error}"),
        };
        assert_eq!(library.guid, STD_OLE);
        let font = library
            .types
            .iter()
            .find(|info| info.name.eq_ignore_ascii_case("IFont"))
            .expect("IFont");
        assert!(
            font.functions
                .iter()
                .any(|function| function.name == "Name"
                    && function.member == MemberKind::PropertyGet)
        );
        assert!(
            font.functions
                .iter()
                .flat_map(|function| &function.parameters)
                .any(|parameter| parameter.ty == "BSTR")
        );
        let coclass = library
            .types
            .iter()
            .find(|info| info.kind == TypeKind::Coclass)
            .expect("coclass");
        assert!(!coclass.implemented.is_empty());
        assert!(
            coclass
                .implemented
                .iter()
                .all(|implemented| !implemented.name.is_empty())
        );
    }

    #[test]
    fn native_guid_round_trips_registry_text() {
        let guid = Guid::parse("{00112233-4455-6677-8899-AABBCCDDEEFF}").expect("GUID");
        let native = native_guid(guid);
        assert_eq!(native.data1, 0x0011_2233);
        assert_eq!(native.data2, 0x4455);
        assert_eq!(native.data3, 0x6677);
        assert_eq!(
            native.data4,
            [0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]
        );
        assert_eq!(guid_text(&native).as_deref(), Some(&guid.to_string()[..]));
    }
}
