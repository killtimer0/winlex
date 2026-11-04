use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static SIMPLE_WINMD: LazyLock<Vec<u8>> = LazyLock::new(|| {
    let temp = tempfile::tempdir().expect("create temporary output directory");
    let rdl = temp.path().join("simple.rdl");
    let winmd = temp.path().join("simple.winmd");

    windows_clang::clang()
        .inputs([
            fixture("simple/simple.h"),
            fixture("simple/partial_bitfield.h"),
        ])
        .args(["-x", "c", "-std=c11", "-DCUSTOM_DEPTH=42"])
        .namespace("SimpleTest")
        .library("simple")
        .include_macros(["_IOFBF", "_IOLBF", "_IONBF"])
        .output(&rdl)
        .write()
        .expect("generate RDL from simple.h");

    windows_rdl::reader()
        .input(&rdl)
        .output(&winmd)
        .write()
        .expect("compile generated simple RDL");

    std::fs::read(winmd).expect("read generated simple WinMD")
});

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(path)
}

fn open_index() -> windows_metadata::reader::Index {
    let file =
        windows_metadata::reader::File::new(SIMPLE_WINMD.clone()).expect("parse generated WinMD");
    windows_metadata::reader::Index::new(vec![file])
}

#[test]
fn generates_simple_types() {
    let index = open_index();
    let types: Vec<String> = index
        .types()
        .filter(|ty| ty.namespace() == "SimpleTest")
        .map(|ty| ty.name().to_string())
        .collect();

    for expected in [
        "AlignedInner",
        "BitfieldKind",
        "CacheAligned",
        "Color",
        "CompareFunc",
        "EmbeddingAligned",
        "FunctionTable",
        "HasAnonUnion",
        "NetAddr",
        "Rect",
        "Value",
        "Widget",
        "WithAnon2DArrayField",
        "WithAnonArrayField",
        "WithBitfield",
    ] {
        assert!(
            types.iter().any(|name| name == expected),
            "{expected} missing. Found: {types:?}"
        );
    }
}

#[test]
fn collapses_pointer_to_function_type_typedef() {
    let index = open_index();
    let function_table = index.expect("SimpleTest", "FunctionTable");
    let fields: Vec<_> = function_table.fields().collect();

    assert_eq!(fields.len(), 1);
    assert_eq!(
        fields[0].ty(),
        windows_metadata::Type::class_named("SimpleTest", "FunctionType")
    );
}

#[test]
fn preserves_callback_parameters() {
    let index = open_index();
    let invoke = index
        .expect("SimpleTest", "CompareFunc")
        .methods()
        .find(|method| method.name() == "Invoke")
        .expect("CompareFunc Invoke method");

    assert_eq!(
        invoke.signature(&[]).types,
        [
            windows_metadata::Type::PtrConst(Box::new(windows_metadata::Type::Void), 1),
            windows_metadata::Type::PtrConst(Box::new(windows_metadata::Type::Void), 1),
        ]
    );
}

#[test]
fn function_returning_callback_pointer_has_only_its_own_parameters() {
    let index = open_index();
    let method = index
        .expect("SimpleTest", "Apis")
        .methods()
        .find(|method| method.name() == "get_function_type")
        .expect("get_function_type method");
    let signature = method.signature(&[]);

    assert_eq!(signature.types, [windows_metadata::Type::I32]);
    assert_eq!(
        signature.return_type,
        windows_metadata::Type::class_named("SimpleTest", "FunctionType")
    );
}

#[test]
fn generates_union_and_anonymous_records() {
    let index = open_index();

    let value = index.expect("SimpleTest", "Value");
    assert!(
        value
            .flags()
            .contains(windows_metadata::TypeAttributes::ExplicitLayout)
    );
    assert_eq!(
        value.fields().map(|field| field.name()).collect::<Vec<_>>(),
        ["i", "f", "bytes"]
    );

    let net_addr = index.expect("SimpleTest", "NetAddr");
    assert_eq!(
        net_addr
            .fields()
            .map(|field| field.name())
            .collect::<Vec<_>>(),
        ["addr", "scope_id"]
    );

    let macro_nested = index.expect("SimpleTest", "MacroNestedUnion");
    let dummy = macro_nested.fields().next().expect("dummy field");
    assert_eq!(dummy.name(), "dummy");
    assert_eq!(
        dummy.ty(),
        windows_metadata::Type::value_named("SimpleTest", "MacroNestedUnion_0")
    );

    let macro_union = index.expect("SimpleTest", "MacroNestedUnion_0");
    assert!(
        macro_union
            .flags()
            .contains(windows_metadata::TypeAttributes::ExplicitLayout)
    );
    assert_eq!(
        macro_union
            .fields()
            .map(|field| field.name())
            .collect::<Vec<_>>(),
        ["d1", "d2", "d3"]
    );

    let anonymous = index.expect("SimpleTest", "HasAnonUnion");
    assert_eq!(
        anonymous
            .fields()
            .map(|field| field.name())
            .collect::<Vec<_>>(),
        ["before", "Anonymous", "after"]
    );
}

#[test]
fn generates_anonymous_record_arrays() {
    let index = open_index();

    let two_dimensional = index.expect("SimpleTest", "WithAnon2DArrayField");
    let fields: Vec<_> = two_dimensional.fields().collect();
    assert_eq!(fields[0].name(), "tc_rxq");
    assert!(matches!(
        fields[0].ty(),
        windows_metadata::Type::ArrayFixed(outer, 4)
            if matches!(*outer, windows_metadata::Type::ArrayFixed(_, 8))
    ));
    assert_eq!(fields[1].name(), "count");

    let one_dimensional = index.expect("SimpleTest", "WithAnonArrayField");
    let fields: Vec<_> = one_dimensional.fields().collect();
    assert_eq!(fields[0].name(), "entries");
    assert!(matches!(
        fields[0].ty(),
        windows_metadata::Type::ArrayFixed(_, 4)
    ));
    assert_eq!(fields[1].name(), "count");
}

#[test]
fn generates_bitfield_storage() {
    let index = open_index();
    let bitfield = index.expect("SimpleTest", "WithBitfield");
    let fields: Vec<_> = bitfield.fields().collect();

    assert_eq!(
        fields.iter().map(|field| field.name()).collect::<Vec<_>>(),
        ["name", "_bitfield", "data"]
    );
    assert_eq!(
        fields[1].ty(),
        windows_metadata::Type::value_named("SimpleTest", "BitfieldKind")
    );
}

#[test]
fn generates_exact_partial_bitfield_storage() {
    let index = open_index();
    let bitfield = index.expect("SimpleTest", "PartialBitfield");
    let fields: Vec<_> = bitfield.fields().collect();

    assert_eq!(
        fields.iter().map(|field| field.name()).collect::<Vec<_>>(),
        ["_bitfield", "next"]
    );
    assert_eq!(
        fields[0].ty(),
        windows_metadata::Type::ArrayFixed(Box::new(windows_metadata::Type::U8), 3)
    );
}

#[test]
fn generates_bool_function_and_conditional_constant() {
    let index = open_index();
    let apis = index.expect("SimpleTest", "Apis");

    let visible = apis
        .methods()
        .find(|method| method.name() == "widget_is_visible")
        .expect("widget_is_visible method");
    assert_eq!(
        visible.signature(&[]).return_type,
        windows_metadata::Type::Bool
    );

    let max_depth = apis
        .fields()
        .find(|field| field.name() == "MAX_DEPTH")
        .expect("MAX_DEPTH constant");
    assert_eq!(
        max_depth.constant().expect("MAX_DEPTH value").value(),
        windows_metadata::Value::I32(42)
    );
    assert!(
        apis.fields()
            .all(|field| field.name() != "TEST_SIG_DEFAULT")
    );
    assert!(apis.fields().all(|field| field.name() != "TEST_SIG_ALIAS"));
}

#[test]
fn projects_builtin_va_list_as_opaque_pointer() {
    let index = open_index();
    let method = index
        .expect("SimpleTest", "Apis")
        .methods()
        .find(|method| method.name() == "consume_va_list")
        .expect("consume_va_list method");

    assert_eq!(
        method.signature(&[]).types,
        [windows_metadata::Type::PtrMut(
            Box::new(windows_metadata::Type::Void),
            1
        )]
    );

    let va_list = index.expect("SimpleTest", "test_va_list");
    let fields: Vec<_> = va_list.fields().collect();
    assert_eq!(fields.len(), 1);
    assert_eq!(
        fields[0].ty(),
        windows_metadata::Type::ArrayFixed(Box::new(windows_metadata::Type::U64), 3)
    );

    let saved = index.expect("SimpleTest", "SavedVaList");
    assert_eq!(
        saved.fields().map(|field| field.name()).collect::<Vec<_>>(),
        ["args", "tail"]
    );
}

#[test]
fn preserves_assembly_label_import_name() {
    let index = open_index();
    let method = index
        .expect("SimpleTest", "Apis")
        .methods()
        .find(|method| method.name() == "redirected_scan")
        .expect("redirected_scan method");

    assert_eq!(
        method
            .impl_map()
            .expect("redirected_scan import")
            .import_name(),
        "actual_scan"
    );
}

#[test]
fn generates_c_expression_macros() {
    let index = open_index();
    let apis = index.expect("SimpleTest", "Apis");

    let combined = apis
        .fields()
        .find(|field| field.name() == "COMBINED_FLAGS")
        .expect("COMBINED_FLAGS constant");
    assert_eq!(
        combined.constant().expect("COMBINED_FLAGS value").value(),
        windows_metadata::Value::I32(3)
    );

    let high_bit = apis
        .fields()
        .find(|field| field.name() == "HIGH_BIT")
        .expect("HIGH_BIT constant");
    assert_eq!(
        high_bit.constant().expect("HIGH_BIT value").value(),
        windows_metadata::Value::U32(1 << 31)
    );

    let buffer_bytes = apis
        .fields()
        .find(|field| field.name() == "BUFFER_BYTES")
        .expect("BUFFER_BYTES constant");
    assert_eq!(buffer_bytes.ty(), windows_metadata::Type::USize);
    assert_eq!(
        buffer_bytes.constant().expect("BUFFER_BYTES value").value(),
        windows_metadata::Value::U32(16)
    );

    for (name, expected) in [("_IOFBF", 0), ("_IOLBF", 1), ("_IONBF", 2)] {
        let field = apis
            .fields()
            .find(|field| field.name() == name)
            .unwrap_or_else(|| panic!("{name} constant"));
        assert_eq!(
            field.constant().expect("buffering mode value").value(),
            windows_metadata::Value::I32(expected)
        );
    }
}

#[test]
fn skips_unsupported_numeric_typedefs() {
    let index = open_index();
    let types: Vec<_> = index.types().map(|ty| ty.name()).collect();

    for unsupported in [
        "__s128",
        "__u128",
        "s128",
        "u128",
        "f128",
        "chained_f128",
        "complex64",
    ] {
        assert!(
            !types.contains(&unsupported),
            "{unsupported} should not be emitted"
        );
    }
}

#[test]
fn skips_unrepresentable_typedef_alignment_and_dependents() {
    let index = open_index();
    let types: Vec<_> = index.types().map(|ty| ty.name()).collect();
    for name in [
        "UnrepresentableAligned",
        "UnrepresentableCallback",
        "UnrepresentableCallbackAlias",
        "UnrepresentableHolder",
        "UnrepresentableUnion",
        "AlignedArgs",
        "StoredAlignedArgs",
    ] {
        assert!(!types.contains(&name), "{name} should be omitted");
    }

    let methods: Vec<_> = index
        .expect("SimpleTest", "Apis")
        .methods()
        .map(|method| method.name())
        .collect();
    assert!(!methods.contains(&"consume_unrepresentable"));
    assert!(!methods.contains(&"consume_unrepresentable_callback"));
    assert!(!methods.contains(&"consume_unrepresentable_union"));
    assert!(!methods.contains(&"consume_stored_aligned_args"));
}

#[test]
fn generates_linux_long_as_64_bit() {
    use windows_metadata::reader::HasAttributes;

    let index = open_index();
    let inner = index.expect("SimpleTest", "AlignedInner");
    let fields: Vec<_> = inner.fields().collect();

    assert!(inner.has_attribute("AlignmentAttribute"));
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name(), "a");
    assert_eq!(fields[0].ty(), windows_metadata::Type::I64);
    assert_eq!(fields[1].name(), "b");
    assert_eq!(fields[1].ty(), windows_metadata::Type::I64);
}

#[test]
fn generates_embedded_aligned_record() {
    let index = open_index();
    let outer = index.expect("SimpleTest", "EmbeddingAligned");
    let fields: Vec<_> = outer.fields().collect();

    assert_eq!(
        fields.iter().map(|field| field.name()).collect::<Vec<_>>(),
        ["before_a", "before_b", "aligned_member", "after"]
    );
    assert_eq!(fields[0].ty(), windows_metadata::Type::I64);
    assert_eq!(fields[1].ty(), windows_metadata::Type::I64);
    assert_eq!(
        fields[2].ty(),
        windows_metadata::Type::value_named("SimpleTest", "AlignedInner")
    );
    assert_eq!(fields[3].ty(), windows_metadata::Type::I32);
}
