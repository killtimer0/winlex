# bnd-clang

`bnd-clang` is bnd's maintained fork of
[`windows-clang`](https://crates.io/crates/windows-clang). It parses C and
C++ headers with libclang and emits Rust Definition Language (RDL) source
for [`windows-rdl`](https://crates.io/crates/windows-rdl) to compile into
WinMD metadata.

The package keeps the upstream-compatible `windows_clang` Rust library name.

## Requirements

Install libclang through the host package manager. On Ubuntu:

```sh
sudo apt-get install libclang-dev
```

The default feature set links libclang at build time. Enable the `runtime`
feature to use `clang-sys` runtime loading instead.

## Usage

```toml
[build-dependencies]
windows-clang = { package = "bnd-clang", version = "0.0.8" }
windows-rdl = { version = "0.100", default-features = false }
```

```rust,no_run
windows_clang::clang()
    .input("example.h")
    .args(["-x", "c", "-std=c11"])
    .filter("example.h")
    .namespace("example")
    .library("example")
    .output("example.rdl")
    .write()
    .expect("generate RDL");

windows_rdl::reader()
    .input("example.rdl")
    .output("example.winmd")
    .write()
    .expect("compile WinMD");
```

For package generation, `write_by_header()` emits flat RDL partitions and
`remap_by_header()` converts the compiled WinMD into canonical
defining-header namespaces. The remapper round-trips through RDL to restore
external TypeRef scopes.

See [`VENDORED.md`](VENDORED.md) for the upstream revision, licensing, and
the maintained fork changes.

## License

MIT or Apache-2.0
