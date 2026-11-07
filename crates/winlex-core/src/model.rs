//! Typed query results. Metadata records borrow their owning Catalog.
use crate::{Architecture, EntityId};
use windows_metadata::reader::*;

pub enum Declaration<'a> {
    Type(TypeDef<'a>),
    Field(Field<'a>),
    Method(MethodDef<'a>),
    Property(Property<'a>),
    Event(Event<'a>),
    Reference(TypeRef<'a>),
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Bitfield {
    pub name: String,
    pub relative_offset_bits: u64,
    pub width_bits: u64,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct FieldLayout {
    pub id: EntityId,
    pub name: String,
    pub offset_bits: u64,
    pub size_bits: u64,
    pub bitfields: Vec<Bitfield>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldLayout>,
}
#[derive(Debug, serde::Serialize)]
pub struct Layout {
    pub id: EntityId,
    pub name: String,
    pub architecture: Architecture,
    pub size_bits: u64,
    pub alignment_bits: u64,
    pub fields: Vec<FieldLayout>,
    pub dependencies_loaded: usize,
}
#[derive(Debug, serde::Serialize)]
pub struct Slot {
    pub method: String,
    pub declaring_type: String,
    pub id: Option<EntityId>,
    pub declaring_type_id: Option<EntityId>,
}
#[derive(Debug, serde::Serialize)]
pub struct Vtable {
    pub id: EntityId,
    pub name: String,
    pub architecture: Architecture,
    pub windows_runtime: bool,
    pub slots: Vec<Slot>,
}
