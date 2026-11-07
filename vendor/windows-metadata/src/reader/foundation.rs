use super::*;

/// Consumer-known foundation semantics, not a resolved CLR definition.
/// Recognition requires a qualified name and a supported core assembly identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoundationType {
    Object,
    Guid,
    ValueType,
    Enum,
    MulticastDelegate,
    Attribute,
    Type,
    IsConst,
    FlagsAttribute,
    ObsoleteAttribute,
    AttributeUsageAttribute,
    AttributeTargets,
    DoesNotReturnAttribute,
    UnmanagedFunctionPointerAttribute,
    CallingConvention,
    ComVisibleAttribute,
}

impl FoundationType {
    fn named(namespace: &str, name: &str) -> Option<Self> {
        Some(match (namespace, name) {
            ("System", "Object") => Self::Object,
            ("System", "Guid") => Self::Guid,
            ("System", "ValueType") => Self::ValueType,
            ("System", "Enum") => Self::Enum,
            ("System", "MulticastDelegate") => Self::MulticastDelegate,
            ("System", "Attribute") => Self::Attribute,
            ("System", "Type") => Self::Type,
            ("System", "FlagsAttribute") => Self::FlagsAttribute,
            ("System", "ObsoleteAttribute") => Self::ObsoleteAttribute,
            ("System", "AttributeUsageAttribute") => Self::AttributeUsageAttribute,
            ("System", "AttributeTargets") => Self::AttributeTargets,
            ("System.Runtime.CompilerServices", "IsConst") => Self::IsConst,
            ("System.Diagnostics.CodeAnalysis", "DoesNotReturnAttribute") => {
                Self::DoesNotReturnAttribute
            }
            ("System.Runtime.InteropServices", "UnmanagedFunctionPointerAttribute") => {
                Self::UnmanagedFunctionPointerAttribute
            }
            ("System.Runtime.InteropServices", "CallingConvention") => Self::CallingConvention,
            ("System.Runtime.InteropServices", "ComVisibleAttribute") => Self::ComVisibleAttribute,
            _ => return None,
        })
    }

    /// Context in which the consumer supports this reference; not a general ABI.
    pub fn semantics(self) -> &'static str {
        match self {
            Self::Guid => "native GUID: 16 bytes, alignment 4 bytes",
            Self::Object
            | Self::ValueType
            | Self::Enum
            | Self::MulticastDelegate
            | Self::Attribute => "base type marker; no native value layout",
            Self::Type => "custom attribute type-name serialization; no native value layout",
            Self::IsConst => "signature const modifier; no native value layout",
            Self::CallingConvention | Self::AttributeTargets => {
                "Int32 custom attribute enum; no general CLR layout"
            }
            _ => "custom attribute constructor and payload; no native value layout",
        }
    }
}

fn core_assembly(name: &str, version: [u16; 4], culture: &str, key: &[u8], flags: u32) -> bool {
    // Only token identities are accepted here. Do not treat a user assembly's
    // System namespace, an arbitrary version, or a WinRT assembly as corelib.
    if !culture.is_empty() || flags & (1 | 0x100 | 0xe00) != 0 {
        return false;
    }
    if name.eq_ignore_ascii_case("mscorlib") {
        matches!(version, [4, 0, 0, 0] | [255, 255, 255, 255])
            && key == [0xb7, 0x7a, 0x5c, 0x56, 0x19, 0x34, 0xe0, 0x89]
    } else if name.eq_ignore_ascii_case("netstandard") {
        matches!(version, [2, 0, 0, 0] | [2, 1, 0, 0])
            && key == [0xcc, 0x7b, 0x13, 0xff, 0xcd, 0x2d, 0xdd, 0x51]
    } else if name.eq_ignore_ascii_case("System.Runtime") {
        (matches!(version, [4, 0..=2, 0, 0] | [4, 2, 1, 0] | [5..=10, 0, 0, 0]))
            && key == [0xb0, 0x3f, 0x5f, 0x7f, 0x11, 0xd5, 0x0a, 0x3a]
    } else {
        false
    }
}

impl TypeRef<'_> {
    pub fn foundation_type(self) -> Option<FoundationType> {
        let ty = FoundationType::named(self.namespace(), self.name())?;
        if self.scope_token() >> 24 != 0x23 {
            return None;
        }
        let ResolutionScope::AssemblyRef(assembly) = self.scope() else {
            return None;
        };
        core_assembly(
            assembly.name(),
            assembly.version(),
            assembly.culture(),
            assembly.public_key_or_token(),
            assembly.flags(),
        )
        .then_some(ty)
    }
}

impl TypeDefOrRef<'_> {
    pub fn foundation_type(&self) -> Option<FoundationType> {
        match self {
            Self::TypeRef(reference) => reference.foundation_type(),
            // A definition is not a WinRT marker reference. Its CLR instance
            // fields must not establish native ABI, even in a reference assembly.
            Self::TypeDef(definition) => {
                let ty = FoundationType::named(definition.namespace(), definition.name())?;
                let row = definition.to_row();
                let mut assemblies = row.index.rows::<Assembly>(row.file);
                let assembly = assemblies.next()?;
                if assemblies.next().is_some() {
                    return None;
                }
                // Assembly definitions may store a full public key. Match its
                // strong-name token rather than mistaking it for token bytes.
                use sha1::{Digest, Sha1};
                let token = if assembly.flags() & 1 != 0 {
                    Sha1::digest(assembly.public_key())[12..]
                        .iter()
                        .rev()
                        .copied()
                        .collect::<Vec<_>>()
                } else {
                    assembly.public_key().to_vec()
                };
                core_assembly(
                    assembly.name(),
                    assembly.version(),
                    assembly.culture(),
                    &token,
                    assembly.flags() & !1,
                )
                .then_some(ty)
            }
            Self::TypeSpec(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_identity_requires_name_version_token_and_culture() {
        let token = [0xb7, 0x7a, 0x5c, 0x56, 0x19, 0x34, 0xe0, 0x89];
        assert!(core_assembly("mscorlib", [4, 0, 0, 0], "", &token, 0));
        assert!(core_assembly("mscorlib", [255; 4], "", &token, 0));
        for (name, version, culture, key, flags) in [
            ("User", [4, 0, 0, 0], "", token.as_slice(), 0),
            ("mscorlib", [1, 0, 0, 0], "", token.as_slice(), 0),
            ("mscorlib", [4, 0, 0, 0], "en-US", token.as_slice(), 0),
            ("mscorlib", [4, 0, 0, 0], "", &[][..], 0),
            ("mscorlib", [4, 0, 0, 0], "", token.as_slice(), 0x100),
            ("mscorlib", [4, 0, 0, 0], "", token.as_slice(), 0x200),
        ] {
            assert!(!core_assembly(name, version, culture, key, flags));
        }
    }

    #[test]
    fn user_system_names_are_not_base_markers_or_const_modifiers() {
        let mut definitions = writer::File::new("UserTypes");
        for name in [
            "ValueType",
            "Enum",
            "MulticastDelegate",
            "Attribute",
            "Type",
            "Guid",
        ] {
            definitions.TypeDef(
                "System",
                name,
                writer::TypeDefOrRef::default(),
                TypeAttributes::Public,
            );
        }
        definitions.TypeDef(
            "System.Runtime.CompilerServices",
            "IsConst",
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public,
        );
        let mut file = writer::File::new("Consumer");
        file.set_reference(Index::new(vec![
            File::new(definitions.into_stream()).unwrap(),
        ]));
        for name in ["ValueType", "Enum", "MulticastDelegate", "Attribute"] {
            let base = file.TypeRef("System", name);
            file.TypeDef(
                "Fixture",
                name,
                writer::TypeDefOrRef::TypeRef(base),
                TypeAttributes::Public,
            );
        }
        let base = file.TypeRef("System", "Object");
        file.TypeDef(
            "Fixture",
            "Signatures",
            writer::TypeDefOrRef::TypeRef(base),
            TypeAttributes::Public,
        );
        file.Field(
            "pointer",
            &Type::PtrConst(Box::new(Type::I32), 1),
            FieldAttributes::Public,
        );
        file.TypeRef("System", "Guid");
        file.TypeRef("System", "Type");
        let index = Index::new(vec![File::new(file.into_stream()).unwrap()]);
        for name in ["ValueType", "Enum", "MulticastDelegate", "Attribute"] {
            let definition = index.expect("Fixture", name);
            assert_eq!(definition.category(), TypeCategory::Class);
            assert!(definition.extends().unwrap().foundation_type().is_none());
        }
        assert_eq!(
            index
                .expect("Fixture", "Signatures")
                .fields()
                .next()
                .unwrap()
                .ty(),
            Type::PtrMut(Box::new(Type::I32), 1)
        );
        for r in index.rows::<TypeRef>(0).filter(|r| r.name() != "Object") {
            assert!(r.foundation_type().is_none(), "{r:?}");
        }
        assert!(
            index
                .foundation_type(&TypeName::named("System", "Guid"))
                .is_none()
        );
    }
}
