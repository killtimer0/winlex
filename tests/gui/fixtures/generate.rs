//! Rebuild the small, checked-in inputs without requiring a compiler on GUI targets.
use std::{error::Error, path::Path};
use windows_metadata::{FieldAttributes, Type, TypeAttributes, writer};

fn main() -> Result<(), Box<dyn Error>> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/gui/fixtures");
    for (version, field) in [("before", "OriginalField"), ("after", "UpdatedField")] {
        let mut file = writer::File::new("ReloadFixture");
        let base = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
        let flags = TypeAttributes::Public
            | TypeAttributes::SequentialLayout
            | TypeAttributes::WindowsRuntime;
        file.TypeDef("GuiFixture", "ReloadSnapshot", base, flags);
        file.Field(field, &Type::I32, FieldAttributes::Public);
        if version == "after" {
            file.TypeDef("GuiFixture", "ReloadVersionTwo", base, flags);
            file.Field("value", &Type::I32, FieldAttributes::Public);
        }
        std::fs::write(
            directory.join(format!("{version}.winmd")),
            file.into_stream(),
        )?;
    }
    Ok(())
}
