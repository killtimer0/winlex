use crate as winlex_core;
use crate::{Catalog, EntityCategory, EntityId, SearchOptions, SourceData};
use windows_metadata::{writer, *};

include!("../../../tests/rust_search.rs");

pub fn source(name: &str, file: writer::File) -> SourceData {
    SourceData {
        path: format!("{name}.winmd").into(),
        bytes: file.into_stream().into(),
    }
}

pub fn catalog(name: &str, file: writer::File) -> Catalog {
    Catalog::from_inputs([source(name, file)]).unwrap()
}

pub fn type_id(db: &Catalog, name: &str) -> EntityId {
    first_match(db.search_exact(
        name,
        SearchOptions {
            categories: Some(EntityCategory::Type as u32),
            ..Default::default()
        },
    ))
    .id
}

pub trait FixtureFile {
    fn public_type(
        &mut self,
        namespace: &str,
        name: &str,
        base: writer::TypeRef,
    ) -> writer::TypeDef;
    fn constructor(&mut self, attribute: writer::TypeRef, types: Vec<Type>) -> writer::MemberRef;
    fn constant(&mut self, name: &str, value: &Value) -> writer::Field;
}

impl FixtureFile for writer::File {
    fn public_type(
        &mut self,
        namespace: &str,
        name: &str,
        base: writer::TypeRef,
    ) -> writer::TypeDef {
        self.TypeDef(
            namespace,
            name,
            writer::TypeDefOrRef::TypeRef(base),
            TypeAttributes::Public,
        )
    }

    fn constructor(&mut self, attribute: writer::TypeRef, types: Vec<Type>) -> writer::MemberRef {
        self.MemberRef(
            ".ctor",
            &Signature {
                flags: MethodCallAttributes::HASTHIS,
                return_type: Type::Void,
                types,
            },
            writer::MemberRefParent::TypeRef(attribute),
        )
    }

    fn constant(&mut self, name: &str, value: &Value) -> writer::Field {
        let field = self.Field(
            name,
            &value.ty(),
            FieldAttributes::Public | FieldAttributes::Static | FieldAttributes::Literal,
        );
        self.Constant(writer::HasConstant::Field(field), value);
        field
    }
}
