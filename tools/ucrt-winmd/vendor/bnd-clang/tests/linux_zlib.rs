use std::path::Path;
use std::sync::LazyLock;

static ZLIB_WINMD: LazyLock<Vec<u8>> = LazyLock::new(|| {
    let temp = tempfile::tempdir().expect("create temporary output directory");
    let types_rdl = temp.path().join("types.rdl");
    let types_winmd = temp.path().join("types.winmd");
    let zlib_rdl = temp.path().join("zlib.rdl");
    let zlib_winmd = temp.path().join("zlib.winmd");

    generate_types(&types_rdl);
    compile_rdl(&[&types_rdl], &types_winmd);

    windows_clang::clang()
        .input("/usr/include/zlib.h")
        .reference(&types_winmd)
        .args(["-x", "c", "-std=c11", "-I/usr/include"])
        .filter("zlib.h")
        .namespace("Zlib")
        .library("z")
        .output(&zlib_rdl)
        .write()
        .expect("generate RDL from zlib.h");

    compile_rdl(&[&types_rdl, &zlib_rdl], &zlib_winmd);
    std::fs::read(zlib_winmd).expect("read generated zlib WinMD")
});

fn generate_types(output: &Path) {
    windows_clang::clang()
        .input("/usr/include/zconf.h")
        .args(["-x", "c", "-std=c11", "-I/usr/include"])
        .filter("zconf.h")
        .namespace("Zlib.Types")
        .library("z")
        .output(output)
        .write()
        .expect("generate RDL from zconf.h");
}

fn compile_rdl(inputs: &[&Path], output: &Path) {
    windows_rdl::reader()
        .inputs(inputs)
        .reference_default()
        .output(output)
        .write()
        .expect("compile RDL");
}

fn open_index() -> windows_metadata::reader::Index {
    let file =
        windows_metadata::reader::File::new(ZLIB_WINMD.clone()).expect("parse generated WinMD");
    windows_metadata::reader::Index::new(vec![file])
}

#[test]
fn generates_partitioned_zlib_types() {
    let index = open_index();
    let types: Vec<_> = index
        .types()
        .map(|ty| (ty.namespace().to_string(), ty.name().to_string()))
        .collect();
    let has = |namespace: &str, name: &str| {
        types.iter().any(|(actual_namespace, actual_name)| {
            actual_namespace == namespace && actual_name == name
        })
    };

    assert!(has("Zlib.Types", "uLong"));
    assert!(has("Zlib.Types", "Bytef"));
    assert!(has("Zlib", "z_stream"));
    assert!(has("Zlib", "gz_header"));
    assert!(has("Zlib", "alloc_func"));
    assert!(has("Zlib", "free_func"));
    assert!(has("Zlib", "in_func"));
    assert!(has("Zlib", "out_func"));
    assert!(has("Zlib", "Apis"));
    assert!(!has("Zlib", "uLong"));
    assert!(!has("Zlib", "Bytef"));
}

#[test]
fn generates_zlib_constants_and_functions() {
    let index = open_index();
    let apis = index.expect("Zlib", "Apis");
    let fields: Vec<_> = apis.fields().map(|field| field.name()).collect();
    let type_apis = index.expect("Zlib.Types", "Apis");
    let type_fields: Vec<_> = type_apis.fields().map(|field| field.name()).collect();
    let methods: Vec<_> = apis.methods().collect();
    let method_names: Vec<_> = methods.iter().map(|method| method.name()).collect();

    for constant in [
        "Z_OK",
        "Z_STREAM_END",
        "Z_NEED_DICT",
        "Z_NO_FLUSH",
        "Z_FINISH",
        "Z_DEFLATED",
        "Z_NULL",
        "Z_NO_COMPRESSION",
        "Z_BEST_SPEED",
        "Z_BEST_COMPRESSION",
        "Z_DEFAULT_STRATEGY",
    ] {
        assert!(fields.contains(&constant), "missing constant {constant}");
    }
    assert!(type_fields.contains(&"MAX_WBITS"));
    for function in [
        "zlibVersion",
        "compress",
        "compress2",
        "uncompress",
        "compressBound",
        "deflateInit_",
        "inflateInit_",
        "deflate",
        "deflateEnd",
        "inflate",
        "inflateEnd",
        "crc32",
        "adler32",
    ] {
        assert!(
            method_names.contains(&function),
            "missing function {function}"
        );
    }

    let compress = methods
        .iter()
        .find(|method| method.name() == "compress")
        .expect("compress method");
    let import = compress.impl_map().expect("compress import");
    assert_eq!(import.import_scope().name(), "z");
    assert!(
        import
            .flags()
            .contains(windows_metadata::PInvokeAttributes::CallConvCdecl)
    );
}

#[test]
fn z_stream_uses_types_partition() {
    let index = open_index();
    let z_stream = index.expect("Zlib", "z_stream");
    let fields: Vec<_> = z_stream.fields().collect();
    let names: Vec<_> = fields.iter().map(|field| field.name()).collect();

    assert_eq!(
        names,
        [
            "next_in",
            "avail_in",
            "total_in",
            "next_out",
            "avail_out",
            "total_out",
            "msg",
            "state",
            "zalloc",
            "zfree",
            "opaque",
            "data_type",
            "adler",
            "reserved",
        ]
    );
    assert_eq!(
        fields[2].ty(),
        windows_metadata::Type::value_named("Zlib.Types", "uLong")
    );
    assert_eq!(
        fields[12].ty(),
        windows_metadata::Type::value_named("Zlib.Types", "uLong")
    );
}
