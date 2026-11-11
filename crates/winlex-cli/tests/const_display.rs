use std::ffi::OsString;
use winlex_core::windows_metadata::{writer, *};
use winlex_core::{Architecture, Catalog, SearchMode, SearchOptions};

include!("../../../tests/rust_search.rs");
include!("support/invoke.rs");

#[test]
fn official_const_owners_remain_visible_without_replacing_native_string_types() {
    let mut file = writer::File::new("ConstDisplay");
    let constructors: Vec<_> = ["Windows.Win32.Foundation.Metadata", "Foreign"]
        .into_iter()
        .map(|namespace| {
            let attribute = file.TypeRef(namespace, "ConstAttribute");
            file.MemberRef(
                ".ctor",
                &Signature {
                    flags: MethodCallAttributes::HASTHIS,
                    return_type: Type::Void,
                    types: vec![],
                },
                writer::MemberRefParent::TypeRef(attribute),
            )
        })
        .collect();
    file.TypeDef(
        "Fixture",
        "Apis",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let pstr = Type::value_named("Windows.Win32.Foundation", "PSTR");
    for (index, name) in ["Official", "Foreign"].into_iter().enumerate() {
        let field = file.Field(&format!("{name}Field"), &pstr, FieldAttributes::Public);
        file.Attribute(
            writer::HasAttribute::Field(field),
            writer::AttributeType::MemberRef(constructors[index]),
            &[],
        );
        file.MethodDef(
            name,
            &Signature {
                flags: MethodCallAttributes::default(),
                return_type: pstr.clone(),
                types: vec![pstr.clone()],
            },
            MethodAttributes::Public | MethodAttributes::Static,
            MethodImplAttributes::default(),
        );
        for (sequence, name) in [(0, ""), (1, "input")] {
            let param = file.Param(name, sequence, ParamAttributes::In);
            file.Attribute(
                writer::HasAttribute::Param(param),
                writer::AttributeType::MemberRef(constructors[index]),
                &[],
            );
        }
    }
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("const-display-{}.winmd", std::process::id()));
    std::fs::write(&path, file.into_stream()).unwrap();
    let catalog = Catalog::open(&path).unwrap();
    for (name, annotated) in [("Official", true), ("Foreign", false)] {
        for selector in [name.to_owned(), format!("{name}Field")] {
            let (exit, text, err) = invoke([
                path.as_os_str().to_owned(),
                OsString::from("show"),
                OsString::from(&selector),
            ]);
            assert_eq!(exit, 0, "{err}");
            let prefix = if annotated { "[const] " } else { "" };
            if selector.ends_with("Field") {
                assert!(text.contains(&format!("type: {prefix}PSTR ->")), "{text}");
            } else {
                assert!(
                    text.contains(&format!("returns: {prefix}PSTR ->")),
                    "{text}"
                );
                assert!(text.contains(&format!("input: {prefix}PSTR ->")), "{text}");
            }
            let entity = first_match(catalog.search(
                &selector,
                SearchOptions {
                    mode: SearchMode::Name,
                    ..Default::default()
                },
            ));
            let rendered =
                winlex_format::declaration::render(&catalog, entity.id, Architecture::X64).unwrap();
            let expected_start = if selector.ends_with("Field") {
                format!("{prefix}PSTR ")
            } else {
                format!("static {prefix}PSTR Apis::{name}(")
            };
            assert!(
                rendered.text.starts_with(&expected_start),
                "{}",
                rendered.text
            );
            if !selector.ends_with("Field") {
                let parameter_prefix = if annotated { "[in, const] " } else { "[in] " };
                assert!(
                    rendered
                        .text
                        .contains(&format!("    {parameter_prefix}PSTR input")),
                    "{}",
                    rendered.text
                );
            }
            let units: Vec<_> = rendered.text.encode_utf16().collect();
            let reference_text = |region: &winlex_format::declaration::Region| {
                String::from_utf16(&units[region.range.clone()]).unwrap()
            };
            let references: Vec<_> = rendered
                .regions
                .iter()
                .filter(|region| region.reference().is_some())
                .map(reference_text)
                .collect();
            if selector.ends_with("Field") {
                assert!(references.is_empty(), "{references:?}");
            } else {
                assert_eq!(references, ["Apis"]);
            }
            for region in rendered.regions.iter().filter(|region| {
                region.reference().is_none()
                    && region.role == Some(winlex_format::declaration::SymbolRole::Type)
            }) {
                assert_eq!(reference_text(region), "PSTR");
            }
        }
    }
    std::fs::remove_file(path).unwrap();
}
