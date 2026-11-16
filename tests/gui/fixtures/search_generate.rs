//! Rebuild search_*.winmd with the workspace's patched windows-metadata writer.
//!
//! cargo run -p winlex-core --example gui-search-fixtures --locked -- tests/gui/fixtures
//!
//! No compiler or generator is required on GUI targets.
use std::{error::Error, path::Path};
use windows_metadata::{FieldAttributes, Type, TypeAttributes, reader, writer};

fn structure(file: &mut writer::File, namespace: &str, name: &str) {
    let base = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
    file.TypeDef(
        namespace,
        name,
        base,
        TypeAttributes::Public | TypeAttributes::SequentialLayout | TypeAttributes::WindowsRuntime,
    );
}

fn field(file: &mut writer::File, name: &str, ty: &Type) {
    file.Field(name, ty, FieldAttributes::Public);
}

pub fn generate(directory: &Path) -> Result<(), Box<dyn Error>> {
    let mut left = writer::File::new("GuiSearchLeft");
    structure(&mut left, "GuiSearch", "GuiSearchSharedEntry");
    field(&mut left, "LeftOnlyValue", &Type::I32);
    for (suffix, member) in [
        ("Amber", "AmberValue"),
        ("Cobalt", "CobaltValue"),
        ("Quartz", "QuartzValue"),
    ] {
        structure(&mut left, "GuiSearch", &format!("GuiSearchFinal{suffix}"));
        field(&mut left, member, &Type::I32);
    }
    structure(&mut left, "GuiSearch", "GuiSearchDiscard");
    field(&mut left, "DiscardValue", &Type::I32);
    structure(&mut left, "GuiSearch", "GuiSearchLeftReady");
    field(&mut left, "ReadyValue", &Type::I32);
    std::fs::write(directory.join("search_left.winmd"), left.into_stream())?;

    let mut right = writer::File::new("GuiSearchRight");
    structure(&mut right, "GuiSearch", "GuiSearchSharedEntry");
    field(&mut right, "RightOnlyValue", &Type::I64);
    structure(&mut right, "GuiSearch", "GuiSearchRightReady");
    field(&mut right, "ReadyValue", &Type::I32);
    std::fs::write(directory.join("search_right.winmd"), right.into_stream())?;

    let mut dependency = writer::File::new("GuiSearchDependency");
    structure(&mut dependency, "GuiSearchDependency", "GuiSearchPair");
    field(&mut dependency, "FirstValue", &Type::I32);
    field(&mut dependency, "SecondValue", &Type::I32);
    structure(
        &mut dependency,
        "GuiSearchDependency",
        "GuiSearchDependencyReady",
    );
    field(&mut dependency, "ReadyValue", &Type::I32);
    let bytes = dependency.into_stream();

    let mut consumer = writer::File::new("GuiSearchConsumer");
    consumer.set_reference(reader::Index::new(vec![
        reader::File::new(bytes.clone()).ok_or("invalid generated dependency")?,
    ]));
    structure(&mut consumer, "GuiSearch", "GuiSearchHolder");
    field(&mut consumer, "HeaderValue", &Type::U32);
    field(
        &mut consumer,
        "Payload",
        &Type::value_named("GuiSearchDependency", "GuiSearchPair"),
    );
    field(&mut consumer, "TailValue", &Type::U32);
    structure(&mut consumer, "GuiSearch", "GuiSearchConsumerReady");
    field(&mut consumer, "ReadyValue", &Type::I32);
    std::fs::write(directory.join("search_dependency.winmd"), bytes)?;
    std::fs::write(
        directory.join("search_consumer.winmd"),
        consumer.into_stream(),
    )?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let directory = std::env::args_os()
        .nth(1)
        .ok_or("expected fixture directory")?;
    generate(Path::new(&directory))
}
