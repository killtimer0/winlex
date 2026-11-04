use winlex_core::windows_metadata::Value;
use winlex_core::windows_metadata::reader::*;
use winlex_core::{Architecture, Catalog, Declaration, EntityId, guid};

pub(super) struct ResultValue {
    pub text: String,
    pub hexadecimal: Option<String>,
    pub integer: Option<i128>,
}

pub(super) fn read(
    catalog: &Catalog,
    id: EntityId,
    architecture: Architecture,
) -> anyhow::Result<Option<ResultValue>> {
    let declaration = catalog.declaration(id, Some(architecture))?;
    let (guid, field) = match declaration {
        Declaration::Type(t) => (guid(t), None),
        Declaration::Field(f) => (guid(f), Some(f)),
        _ => return Ok(None),
    };
    if let Some(guid) = guid {
        return Ok(Some(ResultValue {
            text: format!("{{{}}}", guid.to_ascii_uppercase()),
            hexadecimal: None,
            integer: None,
        }));
    }
    let Some(field) = field else { return Ok(None) };
    let Some(constant) = field.constant() else {
        return Ok(None);
    };
    if constant.usize(0) == 0x12 {
        return Ok(Some(ResultValue {
            text: "null".into(),
            hexadecimal: None,
            integer: None,
        }));
    }
    let value = constant.value();
    let parent: TypeDef<'_> = field.parent_row(4);
    let ty = if parent.category() == TypeCategory::Enum {
        parent.underlying_type().unwrap_or_else(|| field.ty())
    } else {
        field.ty()
    };
    if let Some((number, bits)) = winlex_format::normalized_integer(&value, &ty, architecture) {
        let raw = number & ((1i128 << bits) - 1);
        Ok(Some(ResultValue {
            text: number.to_string(),
            hexadecimal: Some(format!("0x{raw:0width$X}", width = (bits / 4) as usize)),
            integer: Some(number),
        }))
    } else {
        Ok(Some(ResultValue {
            text: match &value {
                Value::Utf8(text) | Value::Utf16(text) => text.clone(),
                _ => winlex_format::constant(&value),
            },
            hexadecimal: None,
            integer: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_values_copy_without_declarations_and_preserve_integer_width() {
        let catalog = Catalog::open(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../inputs/Windows.Win32.winmd"),
        )
        .unwrap();
        for (name, decimal, hexadecimal) in [
            ("ERROR_ACCESS_DENIED", "5", Some("0x00000005")),
            ("E_FAIL", "-2147467259", Some("0x80004005")),
            ("IStream", "{0000000C-0000-0000-C000-000000000046}", None),
        ] {
            let entity = catalog
                .search(name, Default::default())
                .unwrap()
                .map(Result::unwrap)
                .find(|e| e.name == name && (name != "IStream" || e.id.token >> 24 == 2))
                .unwrap();
            let value = read(&catalog, entity.id, Architecture::X64)
                .unwrap()
                .unwrap_or_else(|| panic!("missing value for {name}: {:?}", entity.id));
            assert_eq!(value.text, decimal, "{name}");
            assert_eq!(value.hexadecimal.as_deref(), hexadecimal, "{name}");
            assert_eq!(value.integer, decimal.parse().ok(), "{name}");
        }
        let entity = catalog
            .search("CreateFileW", Default::default())
            .unwrap()
            .map(Result::unwrap)
            .find(|e| e.name == "CreateFileW")
            .unwrap();
        assert!(
            read(&catalog, entity.id, Architecture::X64)
                .unwrap()
                .is_none()
        );
    }
}
