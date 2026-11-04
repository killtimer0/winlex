# Vendored windows-clang

This crate contains an experimental fork of `windows-clang`.

| Field | Value |
|---|---|
| Upstream repository | <https://github.com/microsoft/windows-rs> |
| Upstream path | `crates/libs/clang` |
| Upstream revision | `ca99b307b4e3705da54be35396a33a5afad229fd` |
| Upstream version | `0.100.0` |
| Vendored directory | `vendored/windows-clang` |
| License | MIT OR Apache-2.0 |

The original manifest is retained as `Cargo.toml.upstream`; the bnd package
manifest points its library target at the vendored source. The upstream README
and license files are retained alongside it.

## Local changes

- Optional runtime libclang loading is provided by the `runtime` feature.
- C calling conventions, Clang-reported integer widths, function assembly labels,
  bitfield offsets and callback types are preserved in the emitted metadata.
- Unsupported 128-bit and complex typedefs, function-pointer constants, and
  types with unrepresentable size/alignment are omitted. Compiler `va_list` types
  are represented as opaque pointers.
- Macro probing preserves native signedness and pointer-sized typedef identity;
  callers can include otherwise excluded macros or exclude unavailable symbols.
- Per-header output and remapping preserve referenced namespaces and external
  TypeRef scopes. Explicit scope headers enable reachability filtering.

Additional generator-specific changes are described in `LOCAL-PATCHES.md`.
