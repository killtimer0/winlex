//! COM and WinRT registration lookup independent of metadata and presentation.
use serde::Serialize;
use std::{cell::RefCell, fmt};

pub mod activation;
pub mod registry;
pub mod typelib;

pub(super) fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

/// Where a registration was read from. `Machine` is HKLM, `User` is HKCU.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Root {
    Machine,
    User,
}

/// Where a registration was read from; the same partition as [`Root`].
pub type Origin = Root;

/// A registry view. `Native` follows the process bitness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    X86,
    X64,
    Native,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Views {
    #[default]
    Both,
    X86,
    X64,
    Auto,
}

impl Views {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "both" => Some(Self::Both),
            "32" | "x86" | "32-bit" => Some(Self::X86),
            "64" | "x64" | "64-bit" => Some(Self::X64),
            "auto" | "native" => Some(Self::Auto),
            _ => None,
        }
    }
    fn expand(self) -> Vec<View> {
        match self {
            Self::Both => vec![View::X86, View::X64],
            Self::X86 => vec![View::X86],
            Self::X64 => vec![View::X64],
            Self::Auto => vec![View::Native],
        }
    }
}

/// How the input text is interpreted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TargetKind {
    #[default]
    Auto,
    ProgId,
    Guid,
}

impl TargetKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "progid" | "prog_id" | "prog-id" => Some(Self::ProgId),
            "guid" | "clsid" | "iid" | "libid" | "appid" => Some(Self::Guid),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub text: String,
    pub kind: TargetKind,
    pub views: Views,
}

impl Request {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: TargetKind::Auto,
            views: Views::Both,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Provenance {
    pub view: View,
    pub origin: Origin,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Sourced {
    pub value: String,
    pub sources: Vec<Provenance>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Attribute {
    Description,
    InprocServer32,
    LocalServer32,
    InprocHandler32,
    InprocHandler,
    ThreadingModel,
    ProgId,
    VersionIndependentProgId,
    AppId,
    TypeLib,
    TreatAs,
    AutoConvertTo,
    ProxyStubClsid32,
    NumMethods,
    BaseInterface,
    AsynchronousInterface,
    Flags,
    HelpDir,
    HelpFile,
    DllSurrogate,
    LocalService,
    RunAs,
    AuthenticationLevel,
    ServiceParameters,
    Clsid,
    CurVer,
}

impl Attribute {
    /// Registry value name or a stable English label for CLI output.
    pub fn name(self) -> &'static str {
        match self {
            Self::Description => "Description",
            Self::InprocServer32 => "InprocServer32",
            Self::LocalServer32 => "LocalServer32",
            Self::InprocHandler32 => "InprocHandler32",
            Self::InprocHandler => "InprocHandler",
            Self::ThreadingModel => "ThreadingModel",
            Self::ProgId => "ProgID",
            Self::VersionIndependentProgId => "VersionIndependentProgID",
            Self::AppId => "AppID",
            Self::TypeLib => "TypeLib",
            Self::TreatAs => "TreatAs",
            Self::AutoConvertTo => "AutoConvertTo",
            Self::ProxyStubClsid32 => "ProxyStubClsid32",
            Self::NumMethods => "NumMethods",
            Self::BaseInterface => "BaseInterface",
            Self::AsynchronousInterface => "AsynchronousInterface",
            Self::Flags => "Flags",
            Self::HelpDir => "HelpDir",
            Self::HelpFile => "HelpFile",
            Self::DllSurrogate => "DllSurrogate",
            Self::LocalService => "LocalService",
            Self::RunAs => "RunAs",
            Self::AuthenticationLevel => "AuthenticationLevel",
            Self::ServiceParameters => "ServiceParameters",
            Self::Clsid => "CLSID",
            Self::CurVer => "CurVer",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Field {
    pub attribute: Attribute,
    pub values: Vec<Sourced>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    ProgId,
    Class,
    Interface,
    TypeLib,
    TypeLibVersion,
    TypeLibLocale,
    AppId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Group {
    pub kind: GroupKind,
    /// GUID text, ProgID text, version or locale; language-neutral.
    pub identity: Option<String>,
    pub fields: Vec<Field>,
    pub children: Vec<Group>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Warning {
    EmptyInput,
    InvalidGuid(String),
    InvalidProgId(String),
    Unrecognized(String),
    NotFound(String),
    UnsupportedView(View),
    Registry(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedKind {
    Guid,
    ProgId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Report {
    pub input: String,
    pub kind: Option<ResolvedKind>,
    pub normalized: Option<String>,
    pub groups: Vec<Group>,
    pub warnings: Vec<Warning>,
}

/// Read errors distinguish a missing key from a view that this system cannot
/// address; the latter is a warning, not a failed lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    UnsupportedView,
    Other(String),
}

/// Registry access seam. Tests inject a snapshot; the tool uses the live hive.
pub trait Source {
    fn exists(&self, root: Root, view: View, path: &str) -> Result<bool, ReadError>;
    fn value(
        &self,
        root: Root,
        view: View,
        path: &str,
        name: &str,
    ) -> Result<Option<String>, ReadError>;
    fn subkeys(&self, root: Root, view: View, path: &str) -> Result<Vec<String>, ReadError>;
}

/// A GUID parsed from `{...}`, dashed or bare 32-digit hexadecimal text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guid([u8; 16]);

impl Guid {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text
            .strip_prefix("urn:uuid:")
            .or_else(|| text.strip_prefix("URN:UUID:"))
            .unwrap_or(text);
        let body = match text.strip_prefix('{') {
            Some(rest) => rest.strip_suffix('}')?,
            None => {
                if text.ends_with('}') {
                    return None;
                }
                text
            }
        };
        let digits: Vec<u8> = if body.contains('-') {
            let parts: Vec<&str> = body.split('-').collect();
            let lengths: Vec<usize> = parts.iter().map(|part| part.len()).collect();
            if lengths != [8, 4, 4, 4, 12] {
                return None;
            }
            parts.concat().into_bytes()
        } else {
            if body.len() != 32 {
                return None;
            }
            body.as_bytes().to_vec()
        };
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let mut bytes = [0u8; 16];
        for (index, pair) in digits.as_chunks::<2>().0.iter().enumerate() {
            bytes[index] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        }
        Some(Self(bytes))
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = self.0;
        write!(
            f,
            "{{{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            b[0],
            b[1],
            b[2],
            b[3],
            b[4],
            b[5],
            b[6],
            b[7],
            b[8],
            b[9],
            b[10],
            b[11],
            b[12],
            b[13],
            b[14],
            b[15]
        )
    }
}

impl std::str::FromStr for Guid {
    type Err = ();
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text).ok_or(())
    }
}

/// ProgID syntax: identifiers separated by dots, optionally versioned.
pub fn is_prog_id(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 255
        && text.contains('.')
        && text.split('.').all(|part| !part.is_empty())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

enum Resolved {
    Guid(Guid),
    ProgId(String),
}

fn resolve(request: &Request) -> Result<Resolved, Warning> {
    let text = request.text.trim();
    if text.is_empty() {
        return Err(Warning::EmptyInput);
    }
    match request.kind {
        TargetKind::Guid => Guid::parse(text)
            .map(Resolved::Guid)
            .ok_or_else(|| Warning::InvalidGuid(text.to_owned())),
        TargetKind::ProgId => {
            if is_prog_id(text) {
                Ok(Resolved::ProgId(text.to_owned()))
            } else {
                Err(Warning::InvalidProgId(text.to_owned()))
            }
        }
        TargetKind::Auto => {
            if let Some(guid) = Guid::parse(text) {
                Ok(Resolved::Guid(guid))
            } else if is_prog_id(text) {
                Ok(Resolved::ProgId(text.to_owned()))
            } else {
                Err(Warning::Unrecognized(text.to_owned()))
            }
        }
    }
}

pub fn query(source: &dyn Source, request: &Request) -> Report {
    let reader = Reader::new(source, request.views.expand());
    let (kind, normalized, groups) = match resolve(request) {
        Ok(Resolved::Guid(guid)) => (
            Some(ResolvedKind::Guid),
            Some(guid.to_string()),
            reader.guid_groups(guid),
        ),
        Ok(Resolved::ProgId(text)) => (
            Some(ResolvedKind::ProgId),
            Some(text.clone()),
            reader.prog_id_groups(&text),
        ),
        Err(warning) => {
            return Report {
                input: request.text.clone(),
                kind: None,
                normalized: None,
                groups: Vec::new(),
                warnings: vec![warning],
            };
        }
    };
    let mut warnings = reader.take_warnings();
    if groups.is_empty()
        && let Some(normalized) = &normalized
    {
        warnings.push(Warning::NotFound(normalized.clone()));
    }
    Report {
        input: request.text.clone(),
        kind,
        normalized,
        groups,
        warnings,
    }
}

struct Reader<'a> {
    source: &'a dyn Source,
    views: Vec<View>,
    warnings: RefCell<Vec<Warning>>,
}

impl<'a> Reader<'a> {
    fn new(source: &'a dyn Source, views: Vec<View>) -> Self {
        Self {
            source,
            views,
            warnings: RefCell::new(Vec::new()),
        }
    }
    fn take_warnings(&self) -> Vec<Warning> {
        std::mem::take(&mut *self.warnings.borrow_mut())
    }
    fn warn(&self, warning: Warning) {
        let mut warnings = self.warnings.borrow_mut();
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
    }
    fn each_location(
        &self,
        mut read: impl FnMut(Root, View) -> std::result::Result<(), ReadError>,
    ) {
        for &view in &self.views {
            for root in [Root::Machine, Root::User] {
                match read(root, view) {
                    Ok(()) => {}
                    Err(ReadError::UnsupportedView) => self.warn(Warning::UnsupportedView(view)),
                    Err(ReadError::Other(error)) => self.warn(Warning::Registry(error)),
                }
            }
        }
    }
    fn locations(&self, path: &str) -> Vec<Provenance> {
        let mut out = Vec::new();
        self.each_location(|root, view| {
            if self.source.exists(root, view, path)? {
                out.push(Provenance { view, origin: root });
            }
            Ok(())
        });
        out
    }
    fn values(&self, path: &str, name: &str) -> Vec<Sourced> {
        let mut out: Vec<Sourced> = Vec::new();
        self.each_location(|root, view| {
            if let Some(value) = self.source.value(root, view, path, name)? {
                push_sourced(&mut out, value, Provenance { view, origin: root });
            }
            Ok(())
        });
        out
    }
    fn subkeys(&self, path: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        self.each_location(|root, view| {
            for name in self.source.subkeys(root, view, path)? {
                if !out.iter().any(|seen| seen.eq_ignore_ascii_case(&name)) {
                    out.push(name);
                }
            }
            Ok(())
        });
        out
    }
    fn field(&self, path: &str, attribute: Attribute) -> Option<Field> {
        self.named_field(path, "", attribute)
    }
    fn named_field(&self, path: &str, name: &str, attribute: Attribute) -> Option<Field> {
        let values = self.values(path, name);
        (!values.is_empty()).then_some(Field { attribute, values })
    }
    fn guid_groups(&self, guid: Guid) -> Vec<Group> {
        let mut groups = Vec::new();
        let mut type_libs = Vec::new();
        let mut app_ids = Vec::new();
        if let Some(group) = self.class_group(guid) {
            type_libs.extend(referenced_ids(&group, Attribute::TypeLib));
            app_ids.extend(referenced_ids(&group, Attribute::AppId));
            groups.push(group);
        }
        if let Some(group) = self.interface_group(guid) {
            type_libs.extend(referenced_ids(&group, Attribute::TypeLib));
            groups.push(group);
        }
        if let Some(group) = self.type_lib_group(guid) {
            groups.push(group);
        }
        if let Some(group) = self.app_id_group(guid) {
            groups.push(group);
        }
        for (ids, kind, build) in [
            (
                type_libs,
                GroupKind::TypeLib,
                Self::type_lib_group as fn(&Self, Guid) -> Option<Group>,
            ),
            (app_ids, GroupKind::AppId, Self::app_id_group),
        ] {
            for id in ids {
                if id != guid
                    && !groups.iter().any(|group| {
                        group.kind == kind && group.identity.as_deref() == Some(&id.to_string())
                    })
                    && let Some(group) = build(self, id)
                {
                    groups.push(group);
                }
            }
        }
        groups
    }
    /// The description field when the key exists, or `None` when it does not.
    fn described(&self, path: &str) -> Option<Vec<Field>> {
        if self.locations(path).is_empty() {
            return None;
        }
        Some(
            self.field(path, Attribute::Description)
                .into_iter()
                .collect(),
        )
    }
    /// Read the named subkeys of `path` as fields, preserving the requested order.
    fn subkey_fields(&self, path: &str, subkeys: &[(Attribute, &str)]) -> Vec<Field> {
        subkeys
            .iter()
            .filter_map(|&(attribute, name)| self.field(&format!("{path}\\{name}"), attribute))
            .collect()
    }
    fn class_group(&self, guid: Guid) -> Option<Group> {
        let path = format!("Software\\Classes\\CLSID\\{guid}");
        let mut fields = self.described(&path)?;
        for (attribute, name) in [
            (Attribute::InprocServer32, "InprocServer32"),
            (Attribute::LocalServer32, "LocalServer32"),
        ] {
            let subkey = format!("{path}\\{name}");
            fields.extend(self.field(&subkey, attribute));
            fields.extend(self.named_field(&subkey, "ThreadingModel", Attribute::ThreadingModel));
        }
        fields.extend(self.subkey_fields(
            &path,
            &[
                (Attribute::InprocHandler32, "InprocHandler32"),
                (Attribute::InprocHandler, "InprocHandler"),
                (Attribute::ProgId, "ProgID"),
                (
                    Attribute::VersionIndependentProgId,
                    "VersionIndependentProgID",
                ),
                (Attribute::AppId, "AppID"),
                (Attribute::TypeLib, "TypeLib"),
                (Attribute::TreatAs, "TreatAs"),
                (Attribute::AutoConvertTo, "AutoConvertTo"),
            ],
        ));
        Some(Group {
            kind: GroupKind::Class,
            identity: Some(guid.to_string()),
            fields,
            children: Vec::new(),
        })
    }
    fn interface_group(&self, guid: Guid) -> Option<Group> {
        let path = format!("Software\\Classes\\Interface\\{guid}");
        let mut fields = self.described(&path)?;
        fields.extend(self.subkey_fields(
            &path,
            &[
                (Attribute::ProxyStubClsid32, "ProxyStubClsid32"),
                (Attribute::NumMethods, "NumMethods"),
                (Attribute::BaseInterface, "BaseInterface"),
                (Attribute::AsynchronousInterface, "AsynchronousInterface"),
                (Attribute::TypeLib, "TypeLib"),
            ],
        ));
        Some(Group {
            kind: GroupKind::Interface,
            identity: Some(guid.to_string()),
            fields,
            children: Vec::new(),
        })
    }
    fn type_lib_group(&self, guid: Guid) -> Option<Group> {
        let path = format!("Software\\Classes\\TypeLib\\{guid}");
        let fields = self.described(&path)?;
        let mut children = Vec::new();
        for version in self.subkeys(&path) {
            if version.eq_ignore_ascii_case("FLAGS") || version.eq_ignore_ascii_case("HELPDIR") {
                continue;
            }
            let version_path = format!("{path}\\{version}");
            if self.locations(&version_path).is_empty() {
                continue;
            }
            let fields = self.subkey_fields(
                &version_path,
                &[(Attribute::Flags, "FLAGS"), (Attribute::HelpDir, "HELPDIR")],
            );
            let mut locales = Vec::new();
            for locale in self.subkeys(&version_path) {
                if locale.eq_ignore_ascii_case("FLAGS") || locale.eq_ignore_ascii_case("HELPDIR") {
                    continue;
                }
                let locale_path = format!("{version_path}\\{locale}");
                let flavors: Vec<String> = self
                    .subkeys(&locale_path)
                    .into_iter()
                    .filter(|name| {
                        name.eq_ignore_ascii_case("win32") || name.eq_ignore_ascii_case("win64")
                    })
                    .collect();
                let mut add_locale = |identity: String, fields: Vec<Field>| {
                    if !fields.is_empty() {
                        locales.push(Group {
                            kind: GroupKind::TypeLibLocale,
                            identity: Some(identity),
                            fields,
                            children: Vec::new(),
                        });
                    }
                };
                if flavors.is_empty() {
                    add_locale(
                        locale,
                        self.field(&locale_path, Attribute::Description)
                            .into_iter()
                            .collect(),
                    );
                    continue;
                }
                for flavor in flavors {
                    let flavor_path = format!("{locale_path}\\{flavor}");
                    let mut fields = Vec::new();
                    fields.extend(self.field(&flavor_path, Attribute::Description));
                    fields.extend(self.named_field(&flavor_path, "HELPFILE", Attribute::HelpFile));
                    add_locale(format!("{flavor} {locale}"), fields);
                }
            }
            children.push(Group {
                kind: GroupKind::TypeLibVersion,
                identity: Some(version),
                fields,
                children: locales,
            });
        }
        Some(Group {
            kind: GroupKind::TypeLib,
            identity: Some(guid.to_string()),
            fields,
            children,
        })
    }
    fn app_id_group(&self, guid: Guid) -> Option<Group> {
        let path = format!("Software\\Classes\\AppID\\{guid}");
        let mut fields = self.described(&path)?;
        for (attribute, name) in [
            (Attribute::DllSurrogate, "DllSurrogate"),
            (Attribute::LocalService, "LocalService"),
            (Attribute::RunAs, "RunAs"),
            (Attribute::AuthenticationLevel, "AuthenticationLevel"),
            (Attribute::ServiceParameters, "ServiceParameters"),
        ] {
            if let Some(field) = self.named_field(&path, name, attribute) {
                fields.push(field);
            }
        }
        Some(Group {
            kind: GroupKind::AppId,
            identity: Some(guid.to_string()),
            fields,
            children: Vec::new(),
        })
    }
    fn prog_id_groups(&self, text: &str) -> Vec<Group> {
        let path = format!("Software\\Classes\\{text}");
        let Some(mut fields) = self.described(&path) else {
            return Vec::new();
        };
        fields.extend(self.subkey_fields(
            &path,
            &[(Attribute::Clsid, "CLSID"), (Attribute::CurVer, "CurVer")],
        ));
        let classes: Vec<Guid> = fields
            .iter()
            .filter(|field| field.attribute == Attribute::Clsid)
            .flat_map(|field| field.values.iter())
            .filter_map(|value| Guid::parse(&value.value))
            .collect();
        let mut groups = vec![Group {
            kind: GroupKind::ProgId,
            identity: Some(text.to_owned()),
            fields,
            children: Vec::new(),
        }];
        for class in classes {
            groups.extend(self.guid_groups(class));
        }
        groups
    }
}

fn push_sourced(values: &mut Vec<Sourced>, value: String, provenance: Provenance) {
    if let Some(existing) = values.iter_mut().find(|entry| entry.value == value) {
        if !existing.sources.contains(&provenance) {
            existing.sources.push(provenance);
        }
    } else {
        values.push(Sourced {
            value,
            sources: vec![provenance],
        });
    }
}

fn referenced_ids(group: &Group, attribute: Attribute) -> Vec<Guid> {
    group
        .fields
        .iter()
        .filter(|field| field.attribute == attribute)
        .flat_map(|field| field.values.iter())
        .filter_map(|value| Guid::parse(&value.value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Snapshot {
        keys: BTreeMap<(Root, View, String), Key>,
        unsupported: Vec<View>,
    }

    #[derive(Default, Clone)]
    struct Key {
        values: Vec<(String, String)>,
        subkeys: Vec<String>,
    }

    impl Snapshot {
        fn key(mut self, root: Root, view: View, path: &str) -> Self {
            self.keys.entry((root, view, path.to_owned())).or_default();
            self
        }
        fn value(mut self, root: Root, view: View, path: &str, name: &str, value: &str) -> Self {
            let key = self.keys.entry((root, view, path.to_owned())).or_default();
            key.values.push((name.to_owned(), value.to_owned()));
            self
        }
        fn subkey(mut self, root: Root, view: View, path: &str, name: &str) -> Self {
            let key = self.keys.entry((root, view, path.to_owned())).or_default();
            key.subkeys.push(name.to_owned());
            self
        }
        fn unsupported(mut self, view: View) -> Self {
            self.unsupported.push(view);
            self
        }
        fn check(&self, view: View) -> Result<(), ReadError> {
            if self.unsupported.contains(&view) {
                Err(ReadError::UnsupportedView)
            } else {
                Ok(())
            }
        }
        fn entry(&self, root: Root, view: View, path: &str) -> Option<&Key> {
            self.keys.get(&(root, view, path.to_owned()))
        }
    }

    impl Source for Snapshot {
        fn exists(&self, root: Root, view: View, path: &str) -> Result<bool, ReadError> {
            self.check(view)?;
            Ok(self.entry(root, view, path).is_some())
        }
        fn value(
            &self,
            root: Root,
            view: View,
            path: &str,
            name: &str,
        ) -> Result<Option<String>, ReadError> {
            self.check(view)?;
            Ok(self.entry(root, view, path).and_then(|key| {
                key.values
                    .iter()
                    .find(|(key_name, _)| key_name == name)
                    .map(|(_, value)| value.clone())
            }))
        }
        fn subkeys(&self, root: Root, view: View, path: &str) -> Result<Vec<String>, ReadError> {
            self.check(view)?;
            Ok(self
                .entry(root, view, path)
                .map(|key| key.subkeys.clone())
                .unwrap_or_default())
        }
    }

    const CLSID: &str = "{00000000-0000-0000-0000-000000000001}";
    const IID: &str = "{00000000-0000-0000-0000-000000000002}";
    const LIBID: &str = "{00000000-0000-0000-0000-000000000003}";
    const APPID: &str = "{00000000-0000-0000-0000-000000000004}";

    fn class_path() -> String {
        format!("Software\\Classes\\CLSID\\{CLSID}")
    }

    fn field(group: &Group, attribute: Attribute) -> &Field {
        group
            .fields
            .iter()
            .find(|field| field.attribute == attribute)
            .expect("field")
    }

    #[test]
    fn guid_parsing_accepts_braced_dashed_and_bare_hex() {
        let canonical = Guid::parse(CLSID).expect("braced");
        assert_eq!(canonical.to_string(), CLSID);
        assert_eq!(Guid::parse(&CLSID[1..37]).expect("dashed"), canonical);
        assert_eq!(
            Guid::parse("00000000000000000000000000000001").expect("bare"),
            canonical
        );
        assert_eq!(
            Guid::parse("urn:uuid:00000000-0000-0000-0000-000000000001").expect("urn"),
            canonical
        );
        for text in [
            "",
            "{",
            "}",
            "{00000000-0000-0000-0000-00000000000}",
            "{00000000-0000-0000-0000-0000000000001}",
            "{0000000-0000-0000-0000-000000000001}",
            "00000000-0000-0000-0000-00000000000g",
            "{00000000-0000-0000-0000-000000000001}x",
        ] {
            assert!(Guid::parse(text).is_none(), "{text:?}");
        }
    }

    #[test]
    fn prog_ids_need_a_dot_and_identifier_characters() {
        for text in ["Shell.Application", "Vendor.Class.1", "A.B-C_D"] {
            assert!(is_prog_id(text), "{text}");
        }
        for text in ["", "Shell", ".Shell", "Shell.", "Shell Application", "A..B"] {
            assert!(!is_prog_id(text), "{text:?}");
        }
    }

    #[test]
    fn class_fields_merge_equal_values_and_split_differing_views() {
        let path = class_path();
        let inproc = format!("{path}\\InprocServer32");
        let source = Snapshot::default()
            .key(Root::Machine, View::X86, &path)
            .value(Root::Machine, View::X86, &path, "", "Example class")
            .subkey(Root::Machine, View::X86, &path, "InprocServer32")
            .value(
                Root::Machine,
                View::X86,
                &inproc,
                "",
                "C:\\x86\\example.dll",
            )
            .value(
                Root::Machine,
                View::X86,
                &inproc,
                "ThreadingModel",
                "Apartment",
            )
            .key(Root::Machine, View::X64, &path)
            .value(Root::Machine, View::X64, &path, "", "Example class")
            .subkey(Root::Machine, View::X64, &path, "InprocServer32")
            .value(
                Root::Machine,
                View::X64,
                &inproc,
                "",
                "C:\\x64\\example.dll",
            );
        let report = query(
            &source,
            &Request {
                text: CLSID.into(),
                kind: TargetKind::Guid,
                views: Views::Both,
            },
        );
        assert_eq!(report.kind, Some(ResolvedKind::Guid));
        assert_eq!(report.groups.len(), 1);
        let group = &report.groups[0];
        assert_eq!(group.kind, GroupKind::Class);
        assert_eq!(group.identity.as_deref(), Some(CLSID));
        let description = field(group, Attribute::Description);
        assert_eq!(description.values.len(), 1);
        assert_eq!(description.values[0].sources.len(), 2);
        let server = field(group, Attribute::InprocServer32);
        assert_eq!(server.values.len(), 2);
        assert_eq!(server.values[0].sources[0].view, View::X86);
        assert_eq!(server.values[1].sources[0].view, View::X64);
        let threading = field(group, Attribute::ThreadingModel);
        assert_eq!(threading.values[0].value, "Apartment");
        assert_eq!(threading.values[0].sources[0].view, View::X86);
    }

    #[test]
    fn prog_id_resolves_class_and_referenced_type_library() {
        let path = "Software\\Classes\\Example.Object";
        let class = class_path();
        let typelib = format!("Software\\Classes\\TypeLib\\{LIBID}");
        let source = Snapshot::default()
            .key(Root::Machine, View::X86, path)
            .value(Root::Machine, View::X86, path, "", "Example object")
            .subkey(Root::Machine, View::X86, path, "CLSID")
            .value(
                Root::Machine,
                View::X86,
                &format!("{path}\\CLSID"),
                "",
                CLSID,
            )
            .key(Root::Machine, View::X86, &class)
            .value(Root::Machine, View::X86, &class, "", "Example class")
            .subkey(Root::Machine, View::X86, &class, "TypeLib")
            .value(
                Root::Machine,
                View::X86,
                &format!("{class}\\TypeLib"),
                "",
                LIBID,
            )
            .key(Root::Machine, View::X86, &typelib)
            .value(
                Root::Machine,
                View::X86,
                &typelib,
                "",
                "Example type library",
            )
            .subkey(Root::Machine, View::X86, &typelib, "1.0")
            .key(Root::Machine, View::X86, &format!("{typelib}\\1.0"))
            .subkey(
                Root::Machine,
                View::X86,
                &format!("{typelib}\\1.0"),
                "FLAGS",
            )
            .value(
                Root::Machine,
                View::X86,
                &format!("{typelib}\\1.0\\FLAGS"),
                "",
                "3",
            )
            .subkey(Root::Machine, View::X86, &format!("{typelib}\\1.0"), "0")
            .key(Root::Machine, View::X86, &format!("{typelib}\\1.0\\0"))
            .subkey(
                Root::Machine,
                View::X86,
                &format!("{typelib}\\1.0\\0"),
                "win32",
            )
            .key(
                Root::Machine,
                View::X86,
                &format!("{typelib}\\1.0\\0\\win32"),
            )
            .value(
                Root::Machine,
                View::X86,
                &format!("{typelib}\\1.0\\0\\win32"),
                "",
                "Example type library",
            );
        let report = query(&source, &Request::new("Example.Object"));
        assert_eq!(report.kind, Some(ResolvedKind::ProgId));
        assert_eq!(
            report
                .groups
                .iter()
                .map(|group| group.kind)
                .collect::<Vec<_>>(),
            [GroupKind::ProgId, GroupKind::Class, GroupKind::TypeLib]
        );
        let typelib = &report.groups[2];
        assert_eq!(typelib.children.len(), 1);
        let version = &typelib.children[0];
        assert_eq!(version.identity.as_deref(), Some("1.0"));
        assert_eq!(field(version, Attribute::Flags).values[0].value, "3");
        let locale = &version.children[0];
        assert_eq!(locale.identity.as_deref(), Some("win32 0"));
        assert_eq!(
            field(locale, Attribute::Description).values[0].value,
            "Example type library"
        );
    }

    #[test]
    fn a_guid_lists_every_registration_kind_it_matches() {
        let interface = format!("Software\\Classes\\Interface\\{IID}");
        let appid = format!("Software\\Classes\\AppID\\{APPID}");
        let source = Snapshot::default()
            .key(Root::Machine, View::X86, &interface)
            .value(Root::Machine, View::X86, &interface, "", "IExample")
            .subkey(Root::Machine, View::X86, &interface, "NumMethods")
            .value(
                Root::Machine,
                View::X86,
                &format!("{interface}\\NumMethods"),
                "",
                "3",
            )
            .subkey(Root::Machine, View::X86, &interface, "ProxyStubClsid32")
            .value(
                Root::Machine,
                View::X86,
                &format!("{interface}\\ProxyStubClsid32"),
                "",
                CLSID,
            )
            .key(Root::Machine, View::X86, &appid)
            .value(Root::Machine, View::X86, &appid, "DllSurrogate", "");
        let report = query(
            &source,
            &Request {
                text: IID.into(),
                kind: TargetKind::Guid,
                views: Views::Both,
            },
        );
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].kind, GroupKind::Interface);
        assert_eq!(
            field(&report.groups[0], Attribute::NumMethods).values[0].value,
            "3"
        );

        let report = query(
            &source,
            &Request {
                text: APPID.into(),
                kind: TargetKind::Guid,
                views: Views::Both,
            },
        );
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].kind, GroupKind::AppId);
        let surrogate = field(&report.groups[0], Attribute::DllSurrogate);
        assert_eq!(surrogate.values[0].value, "");
    }

    #[test]
    fn one_guid_lists_every_registration_kind_it_matches() {
        let class = format!("Software\\Classes\\CLSID\\{CLSID}");
        let interface = format!("Software\\Classes\\Interface\\{CLSID}");
        let typelib = format!("Software\\Classes\\TypeLib\\{CLSID}");
        let version = format!("{typelib}\\1.0");
        let appid = format!("Software\\Classes\\AppID\\{CLSID}");
        let source = Snapshot::default()
            .key(Root::Machine, View::X86, &class)
            .value(Root::Machine, View::X86, &class, "", "Example class")
            .key(Root::Machine, View::X86, &interface)
            .value(Root::Machine, View::X86, &interface, "", "IExample")
            .key(Root::Machine, View::X86, &typelib)
            .subkey(Root::Machine, View::X86, &typelib, "1.0")
            .key(Root::Machine, View::X86, &version)
            .value(Root::Machine, View::X86, &version, "", "1.0")
            .key(Root::Machine, View::X86, &appid)
            .value(Root::Machine, View::X86, &appid, "DllSurrogate", "");
        let report = query(
            &source,
            &Request {
                text: CLSID.into(),
                kind: TargetKind::Guid,
                views: Views::Both,
            },
        );
        assert_eq!(
            report
                .groups
                .iter()
                .map(|group| group.kind)
                .collect::<Vec<_>>(),
            [
                GroupKind::Class,
                GroupKind::Interface,
                GroupKind::TypeLib,
                GroupKind::AppId
            ]
        );
    }

    #[test]
    fn missing_and_unavailable_registrations_become_warnings() {
        let source = Snapshot::default().unsupported(View::X64);
        let report = query(
            &source,
            &Request {
                text: CLSID.into(),
                kind: TargetKind::Guid,
                views: Views::Both,
            },
        );
        assert!(report.groups.is_empty());
        assert!(
            report
                .warnings
                .contains(&Warning::UnsupportedView(View::X64))
        );
        assert_eq!(
            report
                .warnings
                .iter()
                .filter(|warning| matches!(warning, Warning::UnsupportedView(View::X64)))
                .count(),
            1
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| matches!(warning, Warning::NotFound(text) if text == CLSID))
        );
    }

    #[test]
    fn invalid_input_names_the_expected_form() {
        let source = Snapshot::default();
        for (text, kind, expected) in [
            ("", TargetKind::Auto, Warning::EmptyInput),
            (
                "{bad",
                TargetKind::Guid,
                Warning::InvalidGuid("{bad".into()),
            ),
            (
                "Shell",
                TargetKind::ProgId,
                Warning::InvalidProgId("Shell".into()),
            ),
            (
                "not a target",
                TargetKind::Auto,
                Warning::Unrecognized("not a target".into()),
            ),
        ] {
            let report = query(
                &source,
                &Request {
                    text: text.into(),
                    kind,
                    views: Views::Both,
                },
            );
            assert_eq!(report.kind, None);
            assert_eq!(report.warnings, [expected]);
        }
    }
}
