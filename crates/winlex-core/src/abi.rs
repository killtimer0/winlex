//! Native parameter locations for the declaration view.
//!
//! Positions are derived from metadata, the layout model, and the documented
//! Microsoft x64, ARM64 and x86 calling conventions. Anything that cannot be
//! derived is rejected as a whole instead of annotating part of a signature.
use crate::metadata::supports;
use crate::{Architecture, Catalog, Declaration, DeclarationTarget, EntityId, Result};
use anyhow::{bail, ensure};
use windows_metadata::{reader::*, *};

/// Locations for one native function, COM method or delegate `Invoke`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeAbi {
    pub this: Option<String>,
    pub return_buffer: Option<String>,
    /// One entry per signature parameter, in declaration order. The variadic
    /// marker has no location and is represented by `None`.
    pub parameters: Vec<Option<String>>,
    pub return_location: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FloatKind {
    F32,
    F64,
}

impl FloatKind {
    fn bytes(self) -> u64 {
        match self {
            Self::F32 => 4,
            Self::F64 => 8,
        }
    }
    fn register(self, index: u32) -> String {
        match self {
            Self::F32 => format!("s{index}"),
            Self::F64 => format!("d{index}"),
        }
    }
}

/// How a value crosses the call boundary.
enum Class {
    Int {
        bytes: u64,
    },
    Float(FloatKind),
    Aggregate {
        bytes: u64,
        alignment: u64,
        /// ARM64 homogeneous floating aggregate, element kind and count.
        hfa: Option<(FloatKind, u32)>,
        /// Largest `Winlex.Metadata.NativeAlignmentAttribute` in bytes.
        explicit_alignment: Option<u64>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Convention {
    Platform,
    Cdecl,
    Stdcall,
    Thiscall,
    Fastcall,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Function,
    ComMethod,
    Delegate,
}

impl Catalog {
    /// Compute the native locations of one callable. Callers use the result only
    /// in the entity's own document; a rejection means the whole signature stays
    /// unannotated.
    pub fn native_abi(
        &self,
        target: &DeclarationTarget,
        method: MethodDef<'_>,
        arch: Architecture,
    ) -> Result<NativeAbi> {
        let owner = method.parent_row::<TypeDef>(5);
        ensure!(
            !owner.flags().contains(TypeAttributes::WindowsRuntime),
            "WinRT signatures do not use native ABI annotations"
        );
        ensure!(
            method.generic_params().len() == 0,
            "generic methods are unsupported"
        );
        let signature = target.signature(method.signature(&[]))?;
        let variadic = signature.flags.0 & 0x0f == MethodCallAttributes::VARARG.0;
        let role = self.abi_role(method, owner)?;
        let convention = match role {
            Role::Function => self.function_convention(method)?,
            Role::ComMethod => {
                ensure!(
                    !method.flags().contains(MethodAttributes::Static),
                    "static interface methods are unsupported"
                );
                if arch == Architecture::X86 {
                    Convention::Stdcall
                } else {
                    Convention::Platform
                }
            }
            Role::Delegate => self.delegate_convention(owner)?,
        };
        ensure!(
            !matches!(convention, Convention::Thiscall | Convention::Fastcall),
            "unsupported calling convention"
        );

        let parameters = signature
            .types
            .iter()
            .map(|ty| self.classify(target.id, ty, arch, 0))
            .collect::<Result<Vec<_>>>()?;
        let result = if signature.return_type == Type::Void {
            None
        } else {
            Some(self.classify(target.id, &signature.return_type, arch, 0)?)
        };
        let mut abi = match arch {
            Architecture::X64 => x64(&parameters, result.as_ref(), role, variadic),
            Architecture::Arm64 => arm64(&parameters, result.as_ref(), role, variadic),
            Architecture::X86 => x86(&parameters, result.as_ref(), role),
        };
        if variadic {
            // The rendered signature always appends an ellipsis after the named parameters.
            abi.parameters.push(None);
        }
        Ok(abi)
    }

    fn abi_role(&self, method: MethodDef<'_>, owner: TypeDef<'_>) -> Result<Role> {
        if owner.category() == TypeCategory::Delegate {
            ensure!(
                method.name() == "Invoke",
                "only delegate Invoke has a native ABI"
            );
            return Ok(Role::Delegate);
        }
        if owner.category() == TypeCategory::Interface {
            return Ok(Role::ComMethod);
        }
        let native = method.flags().contains(MethodAttributes::Static)
            && (method.impl_map().is_some()
                || method
                    .impl_flags()
                    .contains(MethodImplAttributes::Native | MethodImplAttributes::Unmanaged));
        ensure!(native, "declaration is not a native callable");
        Ok(Role::Function)
    }

    fn function_convention(&self, method: MethodDef<'_>) -> Result<Convention> {
        if let Some(import) = method.impl_map() {
            return match import.flags().0 & 0x0700 {
                0 | 0x0100 => Ok(Convention::Platform),
                0x0200 => Ok(Convention::Cdecl),
                0x0300 => Ok(Convention::Stdcall),
                0x0400 => Ok(Convention::Thiscall),
                0x0500 => Ok(Convention::Fastcall),
                _ => bail!("unknown ImplMap calling convention"),
            };
        }
        for attribute in method.attributes() {
            if attribute.namespace() == "Winlex.Metadata"
                && attribute.name() == "NativeCallingConventionAttribute"
            {
                return match attribute.value().as_slice() {
                    [(name, Value::Utf8(value))] if name.is_empty() => {
                        match value.to_ascii_lowercase().as_str() {
                            "c" => Ok(Convention::Cdecl),
                            "stdcall" => Ok(Convention::Stdcall),
                            "fastcall" => Ok(Convention::Fastcall),
                            _ => bail!("unknown native calling convention"),
                        }
                    }
                    _ => bail!("invalid native calling convention attribute"),
                };
            }
        }
        Ok(Convention::Platform)
    }

    fn delegate_convention(&self, owner: TypeDef<'_>) -> Result<Convention> {
        for attribute in owner.attributes() {
            if attribute.name() == "UnmanagedFunctionPointerAttribute" {
                let values = attribute.value();
                let value = match values.as_slice() {
                    [(name, value)] if name.is_empty() => value,
                    _ => bail!("invalid unmanaged function pointer attribute"),
                };
                let value = match value {
                    Value::I32(value) => *value,
                    Value::EnumValue(_, value) => match value.as_ref() {
                        Value::I32(value) => *value,
                        _ => bail!("invalid unmanaged function pointer attribute"),
                    },
                    _ => bail!("invalid unmanaged function pointer attribute"),
                };
                return match value {
                    1 => Ok(Convention::Platform),
                    2 => Ok(Convention::Cdecl),
                    3 => Ok(Convention::Stdcall),
                    4 => Ok(Convention::Thiscall),
                    5 => Ok(Convention::Fastcall),
                    _ => bail!("unknown delegate calling convention"),
                };
            }
        }
        bail!("delegate has no UnmanagedFunctionPointerAttribute")
    }

    fn classify(
        &self,
        owner: EntityId,
        ty: &Type,
        arch: Architecture,
        depth: usize,
    ) -> Result<Class> {
        ensure!(depth < 32, "type nesting exceeds limit");
        let pointer = arch.pointer_bytes() as u64;
        Ok(match ty {
            Type::Bool | Type::I8 | Type::U8 => Class::Int { bytes: 1 },
            Type::Char | Type::I16 | Type::U16 => Class::Int { bytes: 2 },
            Type::I32 | Type::U32 => Class::Int { bytes: 4 },
            Type::I64 | Type::U64 => Class::Int { bytes: 8 },
            Type::ISize | Type::USize => Class::Int { bytes: pointer },
            Type::F32 => Class::Float(FloatKind::F32),
            Type::F64 => Class::Float(FloatKind::F64),
            Type::PtrMut(..) | Type::PtrConst(..) | Type::RefMut(..) | Type::RefConst(..) => {
                Class::Int { bytes: pointer }
            }
            // Every metadata class in a native signature denotes a COM interface
            // pointer. WinRT owners are rejected before classification.
            Type::ClassName(_) => Class::Int { bytes: pointer },
            // The GUID foundation type has a documented native ABI but no collection TypeDef.
            Type::ValueName(name) if self.is_guid_type(name) => Class::Aggregate {
                bytes: 16,
                alignment: 4,
                hfa: None,
                explicit_alignment: None,
            },
            Type::ValueName(name) => {
                let target = self.resolve_target(owner, name, arch)?;
                let declaration = self.declaration(target.id, Some(arch))?;
                let Declaration::Type(row) = declaration else {
                    bail!("expected a value type")
                };
                ensure!(
                    !row.flags().contains(TypeAttributes::WindowsRuntime),
                    "WinRT value types are unsupported"
                );
                if let Some(field) = self.native_typedef_field(row, arch)? {
                    self.classify(owner, &field.ty(), arch, depth + 1)?
                } else if row.category() == TypeCategory::Enum {
                    let underlying = row
                        .underlying_type()
                        .ok_or_else(|| anyhow::anyhow!("enum has no underlying type"))?;
                    self.classify(owner, &underlying, arch, depth + 1)?
                } else if row.category() == TypeCategory::Struct {
                    let layout = self.layout(target.id, arch)?;
                    ensure!(layout.size_bits > 0, "empty value type");
                    ensure!(
                        layout.size_bits.is_multiple_of(8)
                            && layout.alignment_bits.is_multiple_of(8),
                        "layout is not byte-granular"
                    );
                    let bytes = layout.size_bits / 8;
                    let alignment = layout.alignment_bits / 8;
                    ensure!(alignment > 0, "value type has no alignment");
                    let hfa = if arch == Architecture::Arm64 {
                        self.hfa(target.id, layout.size_bits, arch, 0)?
                    } else {
                        None
                    };
                    let explicit_alignment = if arch == Architecture::X86 {
                        self.explicit_alignment(owner, ty, arch, 0)?
                            .filter(|a| *a > 4)
                    } else {
                        None
                    };
                    Class::Aggregate {
                        bytes,
                        alignment,
                        hfa,
                        explicit_alignment,
                    }
                } else {
                    bail!("reference types are not passed by value")
                }
            }
            Type::Void => bail!("void is not a parameter type"),
            Type::Array(_) | Type::ArrayFixed(..) => bail!("array parameters are unsupported"),
            Type::String | Type::Object => bail!("managed parameters are unsupported"),
            Type::Generic(..) | Type::MethodGeneric(..) => {
                bail!("generic parameters are unsupported")
            }
        })
    }

    /// Native typedefs such as `BOOL`, `HRESULT` or `HWND` carry one backing
    /// field and must be classified as that field, never as an aggregate.
    fn native_typedef_field<'a>(
        &self,
        t: TypeDef<'a>,
        arch: Architecture,
    ) -> Result<Option<Field<'a>>> {
        let typedef = t.attributes().any(|attribute| {
            attribute.namespace() == "Windows.Win32.Foundation.Metadata"
                && attribute.name() == "NativeTypedefAttribute"
        });
        if !typedef {
            return Ok(None);
        }
        let mut fields = t.fields().filter(|field| {
            !field.flags().contains(FieldAttributes::Static) && supports(*field, arch.bit())
        });
        let field = fields.next();
        ensure!(
            fields.next().is_none(),
            "native typedef must have one field"
        );
        field
            .map(Some)
            .ok_or_else(|| anyhow::anyhow!("native typedef has no fields"))
    }

    /// ARM64 homogeneous floating aggregates: the whole value is covered by one
    /// kind of float or double, one to four elements, with no padding.
    fn hfa(
        &self,
        id: EntityId,
        size_bits: u64,
        arch: Architecture,
        depth: usize,
    ) -> Result<Option<(FloatKind, u32)>> {
        ensure!(depth < 32, "type nesting exceeds limit");
        if size_bits == 0 || size_bits > 4 * 64 || !size_bits.is_multiple_of(8) {
            return Ok(None);
        }
        let bytes = (size_bits / 8) as usize;
        let mut marks = vec![Mark::Unset; bytes];
        if !self.cover(id, arch, 0, &mut marks, depth)? {
            return Ok(None);
        }
        let mut kind = None;
        for mark in marks {
            let Mark::Float(current) = mark else {
                return Ok(None);
            };
            match kind {
                None => kind = Some(current),
                Some(expected) if expected == current => {}
                Some(_) => return Ok(None),
            }
        }
        let kind = kind.expect("non-empty value");
        let count = (size_bits / 8 / kind.bytes()) as u32;
        ensure!(
            (1..=4).contains(&count),
            "invalid homogeneous aggregate size"
        );
        Ok(Some((kind, count)))
    }

    fn cover(
        &self,
        id: EntityId,
        arch: Architecture,
        offset_bits: u64,
        marks: &mut [Mark],
        depth: usize,
    ) -> Result<bool> {
        ensure!(depth < 32, "type nesting exceeds limit");
        let Declaration::Type(row) = self.declaration(id, Some(arch))? else {
            bail!("expected a value type")
        };
        if row.category() == TypeCategory::Enum {
            return Ok(false);
        }
        ensure!(
            row.category() == TypeCategory::Struct,
            "aggregate is not a struct"
        );
        let layout = self.layout(id, arch)?;
        ensure!(
            layout.fields.len()
                == row
                    .fields()
                    .filter(|f| {
                        !f.flags().contains(FieldAttributes::Static) && supports(*f, arch.bit())
                    })
                    .count(),
            "layout field count mismatch"
        );
        let mut fields =
            layout.fields.into_iter().zip(row.fields().filter(|f| {
                !f.flags().contains(FieldAttributes::Static) && supports(*f, arch.bit())
            }));
        let mut ok = true;
        for (position, field) in &mut fields {
            let span = Span {
                start: offset_bits + position.offset_bits,
                len: position.size_bits,
            };
            if !self.cover_type(id, &field.ty(), arch, span, marks, depth + 1)? {
                ok = false;
            }
        }
        Ok(ok)
    }

    fn cover_type(
        &self,
        owner: EntityId,
        ty: &Type,
        arch: Architecture,
        span: Span,
        marks: &mut [Mark],
        depth: usize,
    ) -> Result<bool> {
        ensure!(depth < 32, "type nesting exceeds limit");
        match ty {
            Type::F32 => mark(marks, span, Mark::Float(FloatKind::F32)),
            Type::F64 => mark(marks, span, Mark::Float(FloatKind::F64)),
            Type::Bool
            | Type::Char
            | Type::I8
            | Type::U8
            | Type::I16
            | Type::U16
            | Type::I32
            | Type::U32
            | Type::I64
            | Type::U64
            | Type::ISize
            | Type::USize
            | Type::PtrMut(..)
            | Type::PtrConst(..)
            | Type::RefMut(..)
            | Type::RefConst(..)
            | Type::ClassName(_)
            | Type::String
            | Type::Object
            | Type::Generic(..)
            | Type::MethodGeneric(..)
            | Type::Array(_)
            | Type::Void => Ok(false),
            Type::ArrayFixed(element, count) => {
                ensure!(*count > 0, "empty fixed array");
                ensure!(
                    span.len.is_multiple_of(*count as u64),
                    "fixed array has no integral stride"
                );
                let stride = span.len / *count as u64;
                let mut ok = true;
                for index in 0..*count {
                    let span = Span {
                        start: span.start + index as u64 * stride,
                        len: stride,
                    };
                    if !self.cover_type(owner, element, arch, span, marks, depth + 1)? {
                        ok = false;
                    }
                }
                Ok(ok)
            }
            Type::ValueName(name) if self.is_guid_type(name) => Ok(false),
            Type::ValueName(name) => {
                let target = self.resolve_target(owner, name, arch)?;
                self.cover(target.id, arch, span.start, marks, depth + 1)
            }
        }
    }

    fn explicit_alignment(
        &self,
        owner: EntityId,
        ty: &Type,
        arch: Architecture,
        depth: usize,
    ) -> Result<Option<u64>> {
        ensure!(depth < 32, "type nesting exceeds limit");
        Ok(match ty {
            Type::ValueName(name) if self.is_guid_type(name) => None,
            Type::ValueName(name) => {
                let target = self.resolve_target(owner, name, arch)?;
                let Declaration::Type(row) = self.declaration(target.id, Some(arch))? else {
                    bail!("expected a value type")
                };
                let own = row
                    .attributes()
                    .filter(|attribute| {
                        attribute.namespace() == "Winlex.Metadata"
                            && attribute.name() == "NativeAlignmentAttribute"
                    })
                    .find_map(|attribute| {
                        let values = attribute.value();
                        match values.as_slice() {
                            [(name, Value::I32(value))] if name.is_empty() && *value > 0 => {
                                Some(*value as u64)
                            }
                            _ => None,
                        }
                    });
                let mut alignment = own;
                if row.category() == TypeCategory::Struct {
                    for field in row.fields().filter(|field| {
                        !field.flags().contains(FieldAttributes::Static)
                            && supports(*field, arch.bit())
                    }) {
                        if let Some(nested) =
                            self.explicit_alignment(target.id, &field.ty(), arch, depth + 1)?
                        {
                            alignment =
                                Some(alignment.map_or(nested, |current| current.max(nested)));
                        }
                    }
                }
                alignment
            }
            Type::ArrayFixed(element, _) => {
                self.explicit_alignment(owner, element, arch, depth + 1)?
            }
            _ => None,
        })
    }
}

#[derive(Clone, Copy)]
struct Span {
    start: u64,
    len: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Unset,
    Float(FloatKind),
}

fn mark(marks: &mut [Mark], span: Span, value: Mark) -> Result<bool> {
    ensure!(
        span.start.is_multiple_of(8) && span.len.is_multiple_of(8),
        "non-byte-aligned floating member"
    );
    let start = (span.start / 8) as usize;
    let end = start + (span.len / 8) as usize;
    ensure!(end <= marks.len(), "member exceeds aggregate size");
    let Mark::Float(_) = value else {
        return Ok(false);
    };
    for slot in &mut marks[start..end] {
        match slot {
            Mark::Unset => *slot = value,
            current if *current == value => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

fn x64_gpr(slot: usize) -> String {
    match slot {
        0 => "rcx".into(),
        1 => "rdx".into(),
        2 => "r8".into(),
        3 => "r9".into(),
        _ => x64_stack(slot),
    }
}

fn x64_stack(slot: usize) -> String {
    format!("[rsp+0x{:X}]", 0x28 + (slot - 4) * 8)
}

fn x64(parameters: &[Class], result: Option<&Class>, role: Role, variadic: bool) -> NativeAbi {
    let mut slot = 0;
    let mut this = None;
    if role == Role::ComMethod {
        this = Some(x64_gpr(slot));
        slot += 1;
    }
    let hidden = matches!(
        (role, result),
        (Role::ComMethod, Some(Class::Aggregate { .. }))
    ) || matches!(
        (role, result),
        (
            Role::Function | Role::Delegate,
            Some(Class::Aggregate { bytes, .. })
        ) if !matches!(bytes, 1 | 2 | 4 | 8)
    );
    let return_buffer = hidden.then(|| {
        let location = if role == Role::ComMethod {
            "rdx"
        } else {
            "rcx"
        }
        .to_owned();
        slot += 1;
        location
    });
    let mut locations = Vec::with_capacity(parameters.len());
    for class in parameters {
        let location = match class {
            Class::Int { .. } => x64_gpr(slot),
            Class::Float(_) => {
                if slot < 4 {
                    let vector = format!("xmm{slot}");
                    if variadic {
                        format!("{}/{}", x64_gpr(slot), vector)
                    } else {
                        vector
                    }
                } else {
                    x64_stack(slot)
                }
            }
            Class::Aggregate { .. } => x64_gpr(slot),
        };
        locations.push(Some(location));
        slot += 1;
    }
    let return_location = match result {
        Some(Class::Int { .. }) => Some("rax".into()),
        Some(Class::Float(_)) => Some("xmm0".into()),
        Some(Class::Aggregate { bytes, .. }) if !hidden && matches!(bytes, 1 | 2 | 4 | 8) => {
            Some("rax".into())
        }
        _ => None,
    };
    debug_assert!(hidden || return_buffer.is_none());
    NativeAbi {
        this,
        return_buffer,
        parameters: locations,
        return_location,
    }
}

fn arm64_stack(offset: u64, alignment: u64) -> String {
    let offset = align_up(offset, alignment);
    if offset == 0 {
        "[sp+0]".into()
    } else {
        format!("[sp+0x{offset:X}]")
    }
}

fn arm64(parameters: &[Class], result: Option<&Class>, role: Role, variadic: bool) -> NativeAbi {
    let mut ngrn = 0usize;
    let mut nsrn = 0usize;
    let mut stack = 0u64;
    let mut this = None;
    if role == Role::ComMethod {
        this = Some("x0".into());
        ngrn = 1;
    }
    let hidden = match (role, result) {
        (Role::ComMethod, Some(Class::Aggregate { .. })) => true,
        (Role::Function | Role::Delegate, Some(Class::Aggregate { bytes, hfa, .. })) => {
            hfa.is_none() && *bytes > 16
        }
        _ => false,
    };
    let return_buffer = if hidden {
        if role == Role::ComMethod {
            ngrn = 2;
            Some("x1".into())
        } else {
            Some("x8".into())
        }
    } else {
        None
    };
    let mut locations = Vec::with_capacity(parameters.len());
    for class in parameters {
        let location = match class {
            Class::Int { .. } => {
                if ngrn < 8 {
                    let location = format!("x{ngrn}");
                    ngrn += 1;
                    location
                } else {
                    let location = arm64_stack(stack, 8);
                    stack += 8;
                    location
                }
            }
            Class::Float(kind) => {
                if variadic {
                    if ngrn < 8 {
                        let location = format!("x{ngrn}");
                        ngrn += 1;
                        location
                    } else {
                        let location = arm64_stack(stack, 8);
                        stack += 8;
                        location
                    }
                } else if nsrn < 8 {
                    let location = kind.register(nsrn as u32);
                    nsrn += 1;
                    location
                } else {
                    let location = arm64_stack(stack, 8);
                    stack += 8;
                    location
                }
            }
            Class::Aggregate {
                bytes,
                alignment,
                hfa,
                ..
            } => {
                let hfa = hfa.filter(|_| !variadic);
                if let Some((kind, count)) = hfa {
                    let count = count as usize;
                    if nsrn + count <= 8 {
                        let location = (0..count)
                            .map(|index| kind.register((nsrn + index) as u32))
                            .collect::<Vec<_>>()
                            .join("/");
                        nsrn += count;
                        location
                    } else {
                        let location = arm64_stack(stack, 8);
                        stack += align_up(*bytes, 8);
                        nsrn = 8;
                        location
                    }
                } else if *bytes > 16 {
                    if ngrn < 8 {
                        let location = format!("x{ngrn}");
                        ngrn += 1;
                        location
                    } else {
                        let location = arm64_stack(stack, 8);
                        stack += 8;
                        location
                    }
                } else {
                    let fit = if *alignment == 16 {
                        (ngrn + 1) & !1
                    } else {
                        ngrn
                    };
                    let count = bytes.div_ceil(8) as usize;
                    if fit + count <= 8 {
                        let location = (0..count)
                            .map(|index| format!("x{}", fit + index))
                            .collect::<Vec<_>>()
                            .join("/");
                        ngrn = fit + count;
                        location
                    } else {
                        let location = arm64_stack(stack, (*alignment).min(16));
                        stack += align_up(*bytes, 8);
                        ngrn = 8;
                        location
                    }
                }
            }
        };
        locations.push(Some(location));
    }
    let return_location = match result {
        Some(Class::Int { .. }) => Some("x0".into()),
        Some(Class::Float(kind)) => Some(kind.register(0)),
        Some(Class::Aggregate { bytes, hfa, .. }) if !hidden => {
            if let Some((kind, count)) = hfa {
                Some(
                    (0..*count)
                        .map(|index| kind.register(index))
                        .collect::<Vec<_>>()
                        .join("/"),
                )
            } else if *bytes <= 8 {
                Some("x0".into())
            } else if *bytes <= 16 {
                Some("x0/x1".into())
            } else {
                None
            }
        }
        _ => None,
    };
    debug_assert!(hidden || return_buffer.is_none());
    NativeAbi {
        this,
        return_buffer,
        parameters: locations,
        return_location,
    }
}

fn align_up(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

fn x86(parameters: &[Class], result: Option<&Class>, role: Role) -> NativeAbi {
    let mut offset = 4u64;
    let mut this = None;
    if role == Role::ComMethod {
        this = Some("[esp+4]".to_owned());
        offset += 4;
    }
    let hidden = match (role, result) {
        (Role::ComMethod, Some(Class::Aggregate { .. })) => true,
        (
            Role::Function | Role::Delegate,
            Some(Class::Aggregate {
                bytes,
                explicit_alignment,
                ..
            }),
        ) => explicit_alignment.is_some() || !matches!(bytes, 1 | 2 | 4 | 8),
        _ => false,
    };
    let return_buffer = if hidden {
        let location = format!("[esp+{offset}]");
        offset += 4;
        Some(location)
    } else {
        None
    };
    let mut locations = Vec::with_capacity(parameters.len());
    for class in parameters {
        let location = format!("[esp+{offset}]");
        offset += match class {
            Class::Int { bytes } => (*bytes).max(4),
            Class::Float(FloatKind::F32) => 4,
            Class::Float(FloatKind::F64) => 8,
            Class::Aggregate {
                bytes,
                explicit_alignment,
                ..
            } => {
                if explicit_alignment.is_some_and(|alignment| alignment > 4) {
                    4
                } else {
                    align_up(*bytes, 4)
                }
            }
        };
        locations.push(Some(location));
    }
    let return_location = match result {
        Some(Class::Int { bytes: 8 }) => Some("edx:eax".into()),
        Some(Class::Int { .. }) => Some("eax".into()),
        Some(Class::Float(_)) => Some("st0".into()),
        Some(Class::Aggregate { bytes, .. }) if !hidden => match bytes {
            8 => Some("edx:eax".into()),
            _ => Some("eax".into()),
        },
        _ => None,
    };
    debug_assert!(hidden || return_buffer.is_none());
    NativeAbi {
        this,
        return_buffer,
        parameters: locations,
        return_location,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FixtureFile, catalog};
    use crate::windows_metadata::writer;
    use crate::{Declaration, SearchOptions};

    fn public_struct() -> TypeAttributes {
        TypeAttributes::Public | TypeAttributes::SequentialLayout
    }

    fn public_interface() -> TypeAttributes {
        TypeAttributes::Public | TypeAttributes::Interface | TypeAttributes::Abstract
    }

    fn public_delegate() -> TypeAttributes {
        TypeAttributes::Public | TypeAttributes::Sealed
    }

    fn named(name: &str) -> Type {
        Type::value_named("Fixture", name)
    }

    fn add_struct(file: &mut writer::File, name: &str, fields: &[Type]) -> writer::TypeDef {
        let base = file.TypeRef("System", "ValueType");
        let definition = file.TypeDef(
            "Fixture",
            name,
            writer::TypeDefOrRef::TypeRef(base),
            public_struct(),
        );
        for (index, ty) in fields.iter().enumerate() {
            file.Field(&format!("f{index}"), ty, FieldAttributes::Public);
        }
        definition
    }

    fn add_method(
        file: &mut writer::File,
        name: &str,
        returns: Type,
        parameters: &[Type],
        call: MethodCallAttributes,
        attributes: MethodAttributes,
    ) -> writer::MethodDef {
        let method = file.MethodDef(
            name,
            &Signature {
                flags: call,
                return_type: returns,
                types: parameters.to_vec(),
            },
            attributes,
            MethodImplAttributes(0),
        );
        for index in 0..parameters.len() {
            file.Param(&format!("p{index}"), index as u16 + 1, ParamAttributes::In);
        }
        method
    }

    fn add_function(
        file: &mut writer::File,
        name: &str,
        returns: Type,
        parameters: &[Type],
        call: MethodCallAttributes,
        convention: PInvokeAttributes,
    ) {
        let method = add_method(
            file,
            name,
            returns,
            parameters,
            call,
            MethodAttributes::Public | MethodAttributes::Static,
        );
        file.ImplMap(method, convention, name, "fixture.dll");
    }

    fn add_callback(file: &mut writer::File, name: &str, returns: Type, parameters: &[Type]) {
        let base = file.TypeRef("System", "MulticastDelegate");
        let callback = file.TypeDef(
            "Fixture",
            name,
            writer::TypeDefOrRef::TypeRef(base),
            public_delegate(),
        );
        add_method(
            file,
            "Invoke",
            returns,
            parameters,
            MethodCallAttributes::HASTHIS,
            MethodAttributes::Public,
        );
        let attribute = file.TypeRef(
            "System.Runtime.InteropServices",
            "UnmanagedFunctionPointerAttribute",
        );
        let constructor = file.constructor(
            attribute,
            vec![Type::value_named(
                "System.Runtime.InteropServices",
                "CallingConvention",
            )],
        );
        file.Attribute(
            writer::HasAttribute::TypeDef(callback),
            writer::AttributeType::MemberRef(constructor),
            &[(
                String::new(),
                Value::EnumValue(
                    TypeName::named("System.Runtime.InteropServices", "CallingConvention"),
                    Box::new(Value::I32(1)),
                ),
            )],
        );
    }

    fn fixture() -> Catalog {
        let mut file = writer::File::new("NativeAbi");

        add_struct(&mut file, "S8", &[Type::I32, Type::I32]);
        add_struct(
            &mut file,
            "S16",
            &[Type::I32, Type::I32, Type::I32, Type::I32],
        );
        add_struct(
            &mut file,
            "S24",
            &[
                Type::I32,
                Type::I32,
                Type::I32,
                Type::I32,
                Type::I32,
                Type::I32,
            ],
        );
        add_struct(
            &mut file,
            "F4",
            &[Type::F32, Type::F32, Type::F32, Type::F32],
        );

        let aligned = add_struct(&mut file, "A8", &[Type::I32, Type::I32]);
        let alignment = file.TypeRef("Winlex.Metadata", "NativeAlignmentAttribute");
        let constructor = file.constructor(alignment, vec![Type::I32]);
        file.Attribute(
            writer::HasAttribute::TypeDef(aligned),
            writer::AttributeType::MemberRef(constructor),
            &[(String::new(), Value::I32(8))],
        );

        let boolean = add_struct(&mut file, "BOOL", &[Type::I32]);
        let typedef = file.TypeRef(
            "Windows.Win32.Foundation.Metadata",
            "NativeTypedefAttribute",
        );
        let constructor = file.constructor(typedef, vec![]);
        file.Attribute(
            writer::HasAttribute::TypeDef(boolean),
            writer::AttributeType::MemberRef(constructor),
            &[],
        );

        file.TypeDef(
            "Fixture",
            "IThing",
            writer::TypeDefOrRef::default(),
            public_interface(),
        );
        for (name, returns, parameters) in [
            ("GetS16", named("S16"), &[Type::I32][..]),
            ("GetS8", named("S8"), &[Type::I32][..]),
            ("GetBool", named("BOOL"), &[Type::I32][..]),
            ("TakeS16", Type::Void, &[named("S16"), Type::I32][..]),
            (
                "TakeMany",
                Type::Void,
                &[
                    Type::I32,
                    Type::I32,
                    Type::I32,
                    Type::I32,
                    Type::I32,
                    Type::I32,
                ][..],
            ),
        ] {
            add_method(
                &mut file,
                name,
                returns,
                parameters,
                MethodCallAttributes::HASTHIS,
                MethodAttributes::Public | MethodAttributes::Abstract | MethodAttributes::Virtual,
            );
        }

        add_callback(&mut file, "Callback", named("S16"), &[Type::F32, Type::I32]);
        add_callback(
            &mut file,
            "HfaCallback",
            named("F4"),
            &[named("F4"), Type::F32],
        );

        let base = file.TypeRef("System", "MulticastDelegate");
        file.TypeDef(
            "Fixture",
            "NoAttribute",
            writer::TypeDefOrRef::TypeRef(base),
            public_delegate(),
        );
        add_method(
            &mut file,
            "Invoke",
            Type::Void,
            &[Type::I32],
            MethodCallAttributes::HASTHIS,
            MethodAttributes::Public,
        );

        file.TypeDef(
            "Fixture",
            "IWinrt",
            writer::TypeDefOrRef::default(),
            public_interface() | TypeAttributes::WindowsRuntime,
        );
        add_method(
            &mut file,
            "Run",
            Type::Void,
            &[],
            MethodCallAttributes::HASTHIS,
            MethodAttributes::Public | MethodAttributes::Abstract | MethodAttributes::Virtual,
        );

        let object = file.TypeRef("System", "Object");
        file.TypeDef(
            "Fixture",
            "Apis",
            writer::TypeDefOrRef::TypeRef(object),
            TypeAttributes::Public,
        );
        add_function(
            &mut file,
            "Ret24",
            named("S24"),
            &[Type::F32, Type::F64, Type::I32],
            MethodCallAttributes::default(),
            PInvokeAttributes::CallConvPlatformapi,
        );
        add_function(
            &mut file,
            "Var",
            Type::Void,
            &[Type::F32, Type::F64],
            MethodCallAttributes::VARARG,
            PInvokeAttributes::CallConvCdecl,
        );
        add_function(
            &mut file,
            "TakeA8",
            Type::Void,
            &[named("A8"), Type::I32],
            MethodCallAttributes::default(),
            PInvokeAttributes::CallConvPlatformapi,
        );
        add_function(
            &mut file,
            "RetA8",
            named("A8"),
            &[],
            MethodCallAttributes::default(),
            PInvokeAttributes::CallConvPlatformapi,
        );
        add_function(
            &mut file,
            "RetBool",
            named("BOOL"),
            &[],
            MethodCallAttributes::default(),
            PInvokeAttributes::CallConvPlatformapi,
        );
        add_function(
            &mut file,
            "RoundTripGuid",
            Type::value_named("System", "Guid"),
            &[Type::value_named("System", "Guid")],
            MethodCallAttributes::default(),
            PInvokeAttributes::CallConvPlatformapi,
        );
        add_function(
            &mut file,
            "BadArray",
            Type::Void,
            &[Type::Array(Box::new(Type::I32))],
            MethodCallAttributes::default(),
            PInvokeAttributes::CallConvPlatformapi,
        );
        add_method(
            &mut file,
            "Instance",
            Type::Void,
            &[Type::I32],
            MethodCallAttributes::HASTHIS,
            MethodAttributes::Public,
        );

        catalog("native-abi", file)
    }

    fn id(catalog: &Catalog, full_name: &str, arch: Architecture) -> EntityId {
        catalog
            .search_exact(
                full_name,
                SearchOptions {
                    architecture: Some(arch),
                    ..Default::default()
                },
            )
            .unwrap()
            .find_map(|item| item.ok())
            .unwrap_or_else(|| panic!("missing {full_name}"))
            .id
    }

    fn compute(catalog: &Catalog, full_name: &str, arch: Architecture) -> NativeAbi {
        let id = id(catalog, full_name, arch);
        let Declaration::Method(row) = catalog.declaration(id, Some(arch)).unwrap() else {
            panic!("{full_name} is not a method")
        };
        catalog
            .native_abi(&DeclarationTarget::from(id), row, arch)
            .unwrap()
    }

    fn locations(values: &[&str]) -> Vec<Option<String>> {
        values.iter().map(|value| Some((*value).into())).collect()
    }

    fn rejected(catalog: &Catalog, full_name: &str, arch: Architecture) {
        let id = id(catalog, full_name, arch);
        let Declaration::Method(row) = catalog.declaration(id, Some(arch)).unwrap() else {
            panic!("{full_name} is not a method")
        };
        assert!(
            catalog
                .native_abi(&DeclarationTarget::from(id), row, arch)
                .is_err(),
            "{full_name} should be rejected"
        );
    }

    #[test]
    fn com_methods_place_this_buffer_and_aggregate_returns() {
        let catalog = fixture();
        for (arch, this, buffer, parameter) in [
            (Architecture::X64, "rcx", "rdx", "r8"),
            (Architecture::Arm64, "x0", "x1", "x2"),
            (Architecture::X86, "[esp+4]", "[esp+8]", "[esp+12]"),
        ] {
            for name in ["Fixture.IThing.GetS16", "Fixture.IThing.GetS8"] {
                let abi = compute(&catalog, name, arch);
                assert_eq!(abi.this.as_deref(), Some(this), "{name} {arch:?}");
                assert_eq!(
                    abi.return_buffer.as_deref(),
                    Some(buffer),
                    "{name} {arch:?}"
                );
                assert_eq!(abi.parameters, locations(&[parameter]), "{name} {arch:?}");
                assert_eq!(abi.return_location, None, "{name} {arch:?}");
            }

            let abi = compute(&catalog, "Fixture.IThing.GetBool", arch);
            assert_eq!(abi.this.as_deref(), Some(this), "{arch:?}");
            assert_eq!(abi.return_buffer, None, "{arch:?}");
            let parameter = match arch {
                Architecture::X64 => "rdx",
                Architecture::Arm64 => "x1",
                Architecture::X86 => "[esp+8]",
            };
            assert_eq!(abi.parameters, locations(&[parameter]), "{arch:?}");
            let result = match arch {
                Architecture::X64 => "rax",
                Architecture::Arm64 => "x0",
                Architecture::X86 => "eax",
            };
            assert_eq!(abi.return_location.as_deref(), Some(result), "{arch:?}");
        }

        let abi = compute(&catalog, "Fixture.IThing.TakeMany", Architecture::X64);
        assert_eq!(
            abi.parameters,
            locations(&["rdx", "r8", "r9", "[rsp+0x28]", "[rsp+0x30]", "[rsp+0x38]"])
        );
        let abi = compute(&catalog, "Fixture.IThing.TakeMany", Architecture::X86);
        assert_eq!(
            abi.parameters,
            locations(&[
                "[esp+8]", "[esp+12]", "[esp+16]", "[esp+20]", "[esp+24]", "[esp+28]",
            ])
        );
        let abi = compute(&catalog, "Fixture.IThing.TakeMany", Architecture::Arm64);
        assert_eq!(
            abi.parameters,
            locations(&["x1", "x2", "x3", "x4", "x5", "x6"])
        );
    }

    #[test]
    fn functions_use_hidden_returns_by_size_and_architecture() {
        let catalog = fixture();
        let abi = compute(&catalog, "Fixture.Apis.Ret24", Architecture::X64);
        assert_eq!(abi.this, None);
        assert_eq!(abi.return_buffer.as_deref(), Some("rcx"));
        assert_eq!(abi.parameters, locations(&["xmm1", "xmm2", "r9"]));
        assert_eq!(abi.return_location, None);

        let abi = compute(&catalog, "Fixture.Apis.Ret24", Architecture::Arm64);
        assert_eq!(abi.return_buffer.as_deref(), Some("x8"));
        assert_eq!(abi.parameters, locations(&["s0", "d1", "x0"]));
        assert_eq!(abi.return_location, None);

        let abi = compute(&catalog, "Fixture.Apis.Ret24", Architecture::X86);
        assert_eq!(abi.return_buffer.as_deref(), Some("[esp+4]"));
        assert_eq!(
            abi.parameters,
            locations(&["[esp+8]", "[esp+12]", "[esp+20]"])
        );
        assert_eq!(abi.return_location, None);

        for arch in Architecture::ALL {
            let abi = compute(&catalog, "Fixture.Apis.RetBool", arch);
            assert_eq!(abi.return_buffer, None, "{arch:?}");
            let result = match arch {
                Architecture::X64 => "rax",
                Architecture::Arm64 => "x0",
                Architecture::X86 => "eax",
            };
            assert_eq!(abi.return_location.as_deref(), Some(result), "{arch:?}");
        }
    }

    #[test]
    fn delegates_and_homogeneous_aggregates_follow_the_platform_rules() {
        let catalog = fixture();

        let abi = compute(&catalog, "Fixture.Callback.Invoke", Architecture::X64);
        assert_eq!(abi.this, None);
        assert_eq!(abi.return_buffer.as_deref(), Some("rcx"));
        assert_eq!(abi.parameters, locations(&["xmm1", "r8"]));
        let abi = compute(&catalog, "Fixture.Callback.Invoke", Architecture::Arm64);
        assert_eq!(abi.return_buffer, None);
        assert_eq!(abi.return_location.as_deref(), Some("x0/x1"));
        assert_eq!(abi.parameters, locations(&["s0", "x0"]));
        let abi = compute(&catalog, "Fixture.Callback.Invoke", Architecture::X86);
        assert_eq!(abi.return_buffer.as_deref(), Some("[esp+4]"));
        assert_eq!(abi.parameters, locations(&["[esp+8]", "[esp+12]"]));

        let abi = compute(&catalog, "Fixture.HfaCallback.Invoke", Architecture::Arm64);
        assert_eq!(abi.return_buffer, None);
        assert_eq!(abi.return_location.as_deref(), Some("s0/s1/s2/s3"));
        assert_eq!(abi.parameters, locations(&["s0/s1/s2/s3", "s4"]));
        let abi = compute(&catalog, "Fixture.HfaCallback.Invoke", Architecture::X64);
        assert_eq!(abi.return_buffer.as_deref(), Some("rcx"));
        assert_eq!(abi.parameters, locations(&["rdx", "xmm2"]));
        let abi = compute(&catalog, "Fixture.HfaCallback.Invoke", Architecture::X86);
        assert_eq!(abi.return_buffer.as_deref(), Some("[esp+4]"));
        assert_eq!(abi.parameters, locations(&["[esp+8]", "[esp+24]"]));
    }

    #[test]
    fn explicit_alignment_and_native_typedefs_change_aggregate_handling() {
        let catalog = fixture();
        for (arch, first, second) in [
            (Architecture::X64, "rcx", "rdx"),
            (Architecture::Arm64, "x0", "x1"),
            (Architecture::X86, "[esp+4]", "[esp+8]"),
        ] {
            let abi = compute(&catalog, "Fixture.Apis.TakeA8", arch);
            assert_eq!(abi.parameters, locations(&[first, second]), "{arch:?}");

            let abi = compute(&catalog, "Fixture.Apis.RetA8", arch);
            let buffer = (arch == Architecture::X86).then_some("[esp+4]");
            assert_eq!(abi.return_buffer.as_deref(), buffer, "{arch:?}");
            let result = match arch {
                Architecture::X64 => Some("rax"),
                Architecture::Arm64 => Some("x0"),
                Architecture::X86 => None,
            };
            assert_eq!(abi.return_location.as_deref(), result, "{arch:?}");
        }
    }

    #[test]
    fn guid_is_sixteen_bytes_four_aligned() {
        let catalog = fixture();
        let abi = compute(&catalog, "Fixture.Apis.RoundTripGuid", Architecture::X64);
        assert_eq!(abi.return_buffer.as_deref(), Some("rcx"));
        assert_eq!(abi.parameters, locations(&["rdx"]));
        assert_eq!(abi.return_location, None);

        let abi = compute(&catalog, "Fixture.Apis.RoundTripGuid", Architecture::Arm64);
        assert_eq!(abi.return_buffer, None);
        assert_eq!(abi.parameters, locations(&["x0/x1"]));
        assert_eq!(abi.return_location.as_deref(), Some("x0/x1"));

        let abi = compute(&catalog, "Fixture.Apis.RoundTripGuid", Architecture::X86);
        assert_eq!(abi.return_buffer.as_deref(), Some("[esp+4]"));
        assert_eq!(abi.parameters, locations(&["[esp+8]"]));
        assert_eq!(abi.return_location, None);
    }

    #[test]
    fn varargs_duplicate_floats_or_use_integer_registers() {
        let catalog = fixture();
        let abi = compute(&catalog, "Fixture.Apis.Var", Architecture::X64);
        assert_eq!(
            abi.parameters,
            [Some("rcx/xmm0".into()), Some("rdx/xmm1".into()), None]
        );
        let abi = compute(&catalog, "Fixture.Apis.Var", Architecture::Arm64);
        assert_eq!(abi.parameters, [Some("x0".into()), Some("x1".into()), None]);
        let abi = compute(&catalog, "Fixture.Apis.Var", Architecture::X86);
        assert_eq!(
            abi.parameters,
            [Some("[esp+4]".into()), Some("[esp+8]".into()), None]
        );
    }

    #[test]
    fn unsupported_calls_are_rejected_as_a_whole() {
        let catalog = fixture();
        for name in [
            "Fixture.Apis.BadArray",
            "Fixture.NoAttribute.Invoke",
            "Fixture.IWinrt.Run",
            "Fixture.Apis.Instance",
        ] {
            rejected(&catalog, name, Architecture::X64);
        }
    }
}
