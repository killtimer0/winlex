//! WinRT parameterized identity and a deliberately bounded scalar ABI view.
use crate::{Architecture, Catalog, Declaration, DeclarationTarget, EntityId, Result, guid};
use anyhow::{bail, ensure};
use sha1::{Digest, Sha1};
use windows_metadata::{reader::*, *};

#[derive(Debug, serde::Serialize)]
pub struct InterfaceIdentity {
    pub iid: String,
    /// The canonical WinRT type signature, not a metadata token or C declaration.
    pub signature: String,
}

#[derive(Debug, serde::Serialize)]
pub struct AbiParameter {
    pub name: String,
    pub ty: String,
    pub direction: &'static str,
}
#[derive(Debug, serde::Serialize)]
pub struct AbiMethod {
    pub calling_convention: &'static str,
    pub return_type: &'static str,
    pub parameters: Vec<AbiParameter>,
}
impl DeclarationTarget {
    /// Closed WinRT interfaces and delegates are the only targets with an instance IID.
    pub fn has_instance_identity(&self, ty: TypeDef<'_>) -> bool {
        !self.arguments.is_empty()
            && ty.flags().contains(TypeAttributes::WindowsRuntime)
            && matches!(
                ty.category(),
                TypeCategory::Interface | TypeCategory::Delegate
            )
    }
}
impl Catalog {
    /// Validate one argument when its declaring type belongs to WinRT.
    /// Ordinary .NET generics do not use the WinRT signature type system.
    pub fn validate_winrt_argument(
        &self,
        owner: EntityId,
        argument: &Type,
        arch: Architecture,
    ) -> Result<()> {
        if self
            .declaration(owner, Some(arch))?
            .owner()
            .is_some_and(|owner| owner.flags().contains(TypeAttributes::WindowsRuntime))
        {
            self.signature_inner(owner, argument, arch, 0)?;
        }
        Ok(())
    }

    pub fn interface_identity(
        &self,
        target: &DeclarationTarget,
        arch: Architecture,
    ) -> Result<InterfaceIdentity> {
        let target = self.bind_target(target, arch)?;
        self.bound_interface_identity(&target, arch)
    }

    fn bound_interface_identity(
        &self,
        target: &DeclarationTarget,
        arch: Architecture,
    ) -> Result<InterfaceIdentity> {
        let Declaration::Type(row) = self.declaration(target.id, Some(arch))? else {
            bail!("select an interface or delegate type")
        };
        ensure!(
            matches!(
                row.category(),
                TypeCategory::Interface | TypeCategory::Delegate
            ),
            "select an interface or delegate type"
        );
        let signature = self.named_winrt_signature(target, arch, 0)?;
        let iid = if target.arguments.is_empty() {
            guid(row).ok_or_else(|| anyhow::anyhow!("interface has no GUID"))?
        } else {
            parameterized_iid(&signature)
        };
        Ok(InterfaceIdentity { iid, signature })
    }

    fn signature_inner(
        &self,
        owner: EntityId,
        ty: &Type,
        arch: Architecture,
        depth: usize,
    ) -> Result<String> {
        ensure!(depth < 64, "WinRT signature depth exceeds limit");
        Ok(match ty {
            Type::Bool => "b1".into(),
            Type::Char => "c2".into(),
            Type::I8 => bail!("Int8 is not a WinRT type argument"),
            Type::U8 => "u1".into(),
            Type::I16 => "i2".into(),
            Type::U16 => "u2".into(),
            Type::I32 => "i4".into(),
            Type::U32 => "u4".into(),
            Type::I64 => "i8".into(),
            Type::U64 => "u8".into(),
            Type::ISize => bail!(
                "nint is not a WinRT type argument; choose int32 or int64 for the required width"
            ),
            Type::USize => bail!(
                "nuint is not a WinRT type argument; choose uint32 or uint64 for the required width"
            ),
            Type::F32 => "f4".into(),
            Type::F64 => "f8".into(),
            Type::String => "string".into(),
            Type::Object => "cinterface(IInspectable)".into(),
            Type::ValueName(name) if self.is_guid_type(name) => "g16".into(),
            Type::ClassName(name) | Type::ValueName(name) => {
                let target = self.resolve_target(owner, name, arch)?;
                self.named_winrt_signature(&target, arch, depth + 1)?
            }
            _ => bail!("type is not valid in a closed WinRT signature: {ty:?}"),
        })
    }

    fn named_winrt_signature(
        &self,
        target: &DeclarationTarget,
        arch: Architecture,
        depth: usize,
    ) -> Result<String> {
        ensure!(depth < 64, "WinRT signature depth exceeds limit");
        let Declaration::Type(row) = self.declaration(target.id, Some(arch))? else {
            bail!("expected a type")
        };
        ensure!(
            row.flags().contains(TypeAttributes::WindowsRuntime),
            "type is not a WinRT type"
        );
        ensure!(
            row.generic_params().len() == target.arguments.len(),
            "WinRT signature requires closed type arguments"
        );
        let full_name = self.entity(target.id)?.full_name;
        Ok(match row.category() {
            TypeCategory::Interface | TypeCategory::Delegate => {
                let guid = guid(row)
                    .ok_or_else(|| anyhow::anyhow!("WinRT interface/delegate has no GUID"))?;
                if !target.arguments.is_empty() {
                    let mut parts = vec![format!("{{{guid}}}")];
                    for ty in &target.arguments {
                        parts.push(self.signature_inner(target.id, ty, arch, depth + 1)?);
                    }
                    format!("pinterface({})", parts.join(";"))
                } else if row.category() == TypeCategory::Delegate {
                    format!("delegate({{{guid}}})")
                } else {
                    format!("{{{guid}}}")
                }
            }
            TypeCategory::Enum => {
                let underlying = enum_underlying(row)?;
                format!(
                    "enum({full_name};{})",
                    self.signature_inner(target.id, &underlying, arch, depth + 1)?
                )
            }
            TypeCategory::Struct => {
                ensure!(
                    target.arguments.is_empty(),
                    "generic WinRT structs are unsupported"
                );
                let mut fields = vec![full_name];
                for field in row
                    .fields()
                    .filter(|f| !f.flags().contains(FieldAttributes::Static))
                {
                    fields.push(self.signature_inner(target.id, &field.ty(), arch, depth + 1)?);
                }
                ensure!(fields.len() > 1, "empty WinRT struct is unsupported");
                format!("struct({})", fields.join(";"))
            }
            TypeCategory::Class => {
                let defaults: Vec<_> = row
                    .interface_impls()
                    .filter(|i| i.has_attribute("DefaultAttribute"))
                    .collect();
                ensure!(
                    defaults.len() == 1,
                    "runtime class must have one default interface"
                );
                let signature =
                    self.signature_inner(target.id, &defaults[0].interface(&[]), arch, depth + 1)?;
                format!("rc({full_name};{signature})")
            }
            TypeCategory::Attribute => bail!("attributes are not WinRT type arguments"),
        })
    }

    /// WinRT slots are independent of InterfaceImpl requirements. Unsupported ABI
    /// methods retain their slot and an explicit reason, never a guessed signature.
    pub fn vtable_target(
        &self,
        target: &DeclarationTarget,
        arch: Architecture,
    ) -> Result<crate::Vtable> {
        let target = self.bind_target(target, arch)?;
        if target.arguments.is_empty() {
            return self.vtable(target.id, arch);
        }
        let identity = self.bound_interface_identity(&target, arch)?;
        let mut vtable = self.bound_vtable(&target, arch)?;
        for entry in &mut vtable.slots {
            let Some(id) = entry.id else { continue };
            let result = (|| {
                let Declaration::Method(method) = self.declaration(id, Some(arch))? else {
                    bail!("select a method")
                };
                self.scalar_method_abi(&target.with_id(id), method, arch, &identity.iid)
            })();
            entry.abi = Some(result.map_err(|e| format!("{e:#}")));
        }
        vtable.instance = Some(identity);
        Ok(vtable)
    }

    fn abi_type(&self, owner: EntityId, ty: &Type, arch: Architecture) -> Result<String> {
        Ok(match ty {
            Type::Bool => "boolean".into(),
            Type::Char => "WCHAR".into(),
            Type::U8 => "uint8".into(),
            Type::I16 => "int16".into(),
            Type::U16 => "uint16".into(),
            Type::I32 => "int32".into(),
            Type::U32 => "uint32".into(),
            Type::I64 => "int64".into(),
            Type::U64 => "uint64".into(),
            Type::F32 => "float32".into(),
            Type::F64 => "float64".into(),
            Type::String => "HSTRING".into(),
            Type::Object => "IInspectable*".into(),
            Type::ValueName(name) if self.is_guid_type(name) => "GUID".into(),
            Type::ClassName(name) | Type::ValueName(name) => {
                let target = self.resolve_target(owner, name, arch)?;
                let Declaration::Type(row) = self.declaration(target.id, Some(arch))? else {
                    bail!("expected a type")
                };
                ensure!(
                    row.flags().contains(TypeAttributes::WindowsRuntime),
                    "ABI requires a WinRT type"
                );
                match row.category() {
                    TypeCategory::Interface | TypeCategory::Delegate => format!(
                        "interface({})*",
                        self.interface_identity(&target, arch)?.iid
                    ),
                    TypeCategory::Enum => {
                        let underlying = enum_underlying(row)?;
                        self.abi_type(owner, &underlying, arch)?
                    }
                    _ => bail!("struct/runtime-class ABI lowering is not implemented"),
                }
            }
            _ => bail!(
                "scalar ABI does not support arrays, pointers, open parameters or native-sized types"
            ),
        })
    }

    fn scalar_method_abi(
        &self,
        target: &DeclarationTarget,
        method: MethodDef<'_>,
        arch: Architecture,
        iid: &str,
    ) -> Result<AbiMethod> {
        ensure!(
            !method.flags().contains(MethodAttributes::Static)
                && method.generic_params().len() == 0,
            "static/generic method ABI unsupported"
        );
        let signature = target.signature(method.signature(&[]))?;
        ensure!(signature.flags.0 & 0x0f == 0, "vararg ABI unsupported");
        let params = method.params_by_sequence(signature.types.len())?;
        let mut parameters = vec![AbiParameter {
            name: "This".into(),
            ty: format!("interface({iid})*"),
            direction: "in",
        }];
        for (index, (ty, param)) in signature.types.iter().zip(params.params()).enumerate() {
            let flags = param.map_or(ParamAttributes(0), |p| p.flags());
            let (ty, byref) = match ty {
                Type::RefMut(ty) | Type::RefConst(ty) => (&**ty, true),
                ty => (ty, false),
            };
            let mut abi = self.abi_type(target.id, ty, arch)?;
            let output = flags.contains(ParamAttributes::Out);
            ensure!(
                !output || byref,
                "scalar output parameter must be by-reference"
            );
            if byref {
                abi.push('*');
            }
            parameters.push(AbiParameter {
                name: param
                    .map(|p| p.name().to_owned())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| format!("arg{}", index + 1)),
                ty: abi,
                direction: if output {
                    if flags.contains(ParamAttributes::In) {
                        "in,out"
                    } else {
                        "out"
                    }
                } else {
                    "in"
                },
            });
        }
        if signature.return_type != Type::Void {
            let mut name = "__result".to_owned();
            while parameters.iter().any(|p| p.name == name) {
                name.push('_');
            }
            parameters.push(AbiParameter {
                name,
                ty: format!(
                    "{}*",
                    self.abi_type(target.id, &signature.return_type, arch)?
                ),
                direction: "out,retval",
            });
        }
        Ok(AbiMethod {
            calling_convention: if arch == Architecture::X86 {
                "stdcall"
            } else {
                "platform"
            },
            return_type: "HRESULT",
            parameters,
        })
    }
}

fn enum_underlying(row: TypeDef<'_>) -> Result<Type> {
    let underlying = row
        .underlying_type()
        .ok_or_else(|| anyhow::anyhow!("enum has no underlying type"))?;
    ensure!(
        matches!(underlying, Type::I32 | Type::U32),
        "WinRT enum must use Int32 or UInt32"
    );
    Ok(underlying)
}

fn parameterized_iid(signature: &str) -> String {
    // WinRT namespace UUID, in network byte order (not a Windows GUID memory layout).
    let namespace = [
        0x11, 0xf4, 0x7a, 0xd5, 0x7b, 0x73, 0x42, 0xc0, 0xab, 0xae, 0x87, 0x8b, 0x1e, 0x16, 0xad,
        0xee,
    ];
    let mut hash = Sha1::new();
    hash.update(namespace);
    hash.update(signature.as_bytes());
    let mut bytes: [u8; 20] = hash.finalize().into();
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FixtureFile, catalog};
    #[test]
    fn sdk_ivector_int32_identity() {
        assert_eq!(
            parameterized_iid("pinterface({913337e9-11a1-4345-a3a2-4e7f956e222d};i4)"),
            "b939af5b-b45d-5489-9149-61442c1905fe"
        );
    }

    fn vector_fixture() -> Catalog {
        let mut file = writer::File::new("WinRTInstances");
        let guid_type = file.TypeRef("Windows.Foundation.Metadata", "GuidAttribute");
        let ctor = file.constructor(
            guid_type,
            vec![
                Type::U32,
                Type::U16,
                Type::U16,
                Type::U8,
                Type::U8,
                Type::U8,
                Type::U8,
                Type::U8,
                Type::U8,
                Type::U8,
                Type::U8,
            ],
        );
        let vector = file.TypeDef(
            "Windows.Foundation.Collections",
            "IVector`1",
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public
                | TypeAttributes::Interface
                | TypeAttributes::Abstract
                | TypeAttributes::WindowsRuntime,
        );
        file.GenericParam(
            "T",
            writer::TypeOrMethodDef::TypeDef(vector),
            0,
            GenericParamAttributes(0),
        );
        let guid = [
            Value::U32(0x913337e9),
            Value::U16(0x11a1),
            Value::U16(0x4345),
            Value::U8(0xa3),
            Value::U8(0xa2),
            Value::U8(0x4e),
            Value::U8(0x7f),
            Value::U8(0x95),
            Value::U8(0x6e),
            Value::U8(0x22),
            Value::U8(0x2d),
        ];
        file.Attribute(
            writer::HasAttribute::TypeDef(vector),
            writer::AttributeType::MemberRef(ctor),
            &guid
                .into_iter()
                .map(|v| (String::new(), v))
                .collect::<Vec<_>>(),
        );
        file.MethodDef(
            "GetAt",
            &Signature {
                flags: MethodCallAttributes::HASTHIS,
                return_type: Type::Generic("T".into(), 0),
                types: vec![Type::U32],
            },
            MethodAttributes::Public | MethodAttributes::Abstract | MethodAttributes::Virtual,
            MethodImplAttributes(0),
        );
        file.Param("index", 1, ParamAttributes::In);
        file.MethodDef(
            "GetMany",
            &Signature {
                flags: MethodCallAttributes::HASTHIS,
                return_type: Type::U32,
                types: vec![Type::Array(Box::new(Type::Generic("T".into(), 0)))],
            },
            MethodAttributes::Public | MethodAttributes::Abstract | MethodAttributes::Virtual,
            MethodImplAttributes(0),
        );
        file.Param("items", 1, ParamAttributes::Out);
        file.TypeDef(
            "System",
            "Guid",
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public,
        );
        let regular = file.TypeDef(
            "Fixture",
            "Box`1",
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public,
        );
        file.GenericParam(
            "T",
            writer::TypeOrMethodDef::TypeDef(regular),
            0,
            GenericParamAttributes(0),
        );
        catalog("winrt-instances", file)
    }

    #[test]
    fn interactive_argument_validation_uses_winrt_signatures_only_for_winrt_owners() {
        let catalog = vector_fixture();
        for arch in Architecture::ALL {
            let winrt = catalog.parse_target("IVector<int32>", arch).unwrap();
            let regular = catalog.parse_target("Fixture.Box<nuint>", arch).unwrap();
            for expression in [
                "int32",
                "uint32",
                "int64",
                "uint64",
                "guid",
                "IVector<int32>",
            ] {
                let argument = catalog.parse_argument(winrt.id, expression, arch).unwrap();
                catalog
                    .validate_winrt_argument(winrt.id, &argument, arch)
                    .unwrap();
            }
            for expression in [
                "nint",
                "nuint",
                "IVector<nuint>",
                "IVector<IVector<nint>>",
                "int32[]",
                "int8",
                "Fixture.Box<int32>",
            ] {
                let argument = catalog.parse_argument(winrt.id, expression, arch).unwrap();
                assert!(
                    catalog
                        .validate_winrt_argument(winrt.id, &argument, arch)
                        .is_err(),
                    "{expression}"
                );
            }
            for expression in ["nint", "nuint", "Fixture.Box<nuint>"] {
                let argument = catalog
                    .parse_argument(regular.id, expression, arch)
                    .unwrap();
                catalog
                    .validate_winrt_argument(regular.id, &argument, arch)
                    .unwrap();
            }
            let vtable = catalog.vtable_target(&winrt, arch).unwrap();
            let method = vtable.slots[6].id.unwrap();
            assert!(
                catalog
                    .validate_winrt_argument(method, &Type::USize, arch)
                    .is_err()
            );
            catalog
                .validate_winrt_argument(method, &Type::I32, arch)
                .unwrap();
        }
    }

    #[test]
    fn bound_winrt_slots_and_scalar_return_lowering_match_the_sdk_contract() {
        let catalog = vector_fixture();
        for arch in Architecture::ALL {
            let target = catalog
                .parse_target("Windows.Foundation.Collections.IVector<int32>", arch)
                .unwrap();
            assert_eq!(
                catalog.interface_identity(&target, arch).unwrap().iid,
                "b939af5b-b45d-5489-9149-61442c1905fe"
            );
            assert!(
                catalog.vtable(target.id, arch).is_err(),
                "open generic definitions still reject ABI"
            );
            let vtable = catalog.vtable_target(&target, arch).unwrap();
            assert_eq!(vtable.slots[6].method, "GetAt");
            let abi = vtable.slots[6].abi.as_ref().unwrap().as_ref().unwrap();
            assert_eq!(abi.return_type, "HRESULT");
            assert_eq!(abi.parameters[1].ty, "uint32");
            assert_eq!(abi.parameters[2].ty, "int32*");
            assert_eq!(abi.parameters[2].direction, "out,retval");
            assert!(
                vtable.slots[7]
                    .abi
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap_err()
                    .contains("arrays")
            );
            let nested = catalog
                .parse_target("IVector<IVector<int32>>", arch)
                .unwrap();
            assert_eq!(
                catalog.interface_identity(&nested, arch).unwrap().signature,
                "pinterface({913337e9-11a1-4345-a3a2-4e7f956e222d};pinterface({913337e9-11a1-4345-a3a2-4e7f956e222d};i4))"
            );
            let invalid = catalog.parse_target("IVector<int32[]>", arch).unwrap();
            assert!(catalog.interface_identity(&invalid, arch).is_err());
            let intrinsic = catalog.parse_target("IVector<guid>", arch).unwrap();
            assert!(
                catalog
                    .interface_identity(&intrinsic, arch)
                    .unwrap()
                    .signature
                    .ends_with(";g16)")
            );
            let impostor = catalog
                .parse_target("IVector<0::System.Guid>", arch)
                .unwrap();
            assert!(catalog.interface_identity(&impostor, arch).is_err());
        }
    }
}
