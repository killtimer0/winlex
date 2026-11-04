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

The vendored directory started as an exact copy of the upstream crate at the
recorded revision, including its original manifest, README, and license
files. The upstream manifest is stored as `Cargo.toml.upstream` so Cargo does
not exclude the vendored source as a nested package. The bnd package manifest
lives one level above it and points its library target at the vendored source.

Local experiments should modify files under `vendored/windows-clang` so the
fork remains easy to compare with upstream.

## Local changes

- Crate-level documentation uses the bnd package README while the upstream
  README remains preserved in the vendored directory.
- Runtime libclang loading is optional behind the bnd package's `runtime`
  feature. Workspace builds use the linked libclang selected by `clang-sys`,
  avoiding feature unification with the existing `clang` wrapper and keeping
  binding regeneration deterministic.
- Plain functions and callbacks parsed from C translation units are emitted
  with the C calling convention. This preserves their source ABI instead of
  falling back to the Windows platform-default convention in RDL.
- C `long` and `unsigned long` are mapped from the widths reported by Clang
  for the active host ABI.
- Typedefs that shadow Rust primitive names are omitted to prevent recursive
  generated aliases such as `pub type bool = bool`.
- Direct and chained 128-bit integer, 128-bit floating-point, and C complex
  typedefs are omitted because WinMD cannot represent them.
- Compiler `va_list` records and typedef chains are projected as opaque
  pointers, matching their decayed C parameter ABI without exposing
  compiler-private record layouts.
- Macro-expanded records without a source-file cursor retain an assigned
  generated name, allowing named nested records to remain concrete while
  unnamed compiler builtins stay opaque.
- Deferred macro probes use C `__auto_type` or C++ `constexpr auto` so
  expression macros retain native signedness and pointer-sized typedef
  identity in either language mode.
- Macros cast to function-pointer typedefs are omitted because WinMD cannot
  represent function-pointer constants.
- Pointers to bare function-type typedefs collapse to the generated callback
  alias, avoiding an extra pointer indirection in callback tables.
- Typedefs whose native size is not a multiple of their required alignment,
  and functions depending on those types, are omitted because Rust cannot
  represent those layouts as concrete value types.
- Leading-underscore macros remain excluded by default; callers can
  explicitly include public names through `include_macro(s)`.
- Callers can exclude declarations for unavailable native symbols through
  `exclude_symbol(s)`.
- Function redeclaration selection preserves Clang assembly labels, ensuring
  glibc redirects such as `scanf` to `__isoc99_scanf` reach the correct
  exported symbol.
- Partial bitfield allocation units use Clang field offsets to preserve their
  exact occupied byte span and the enclosing record's native alignment.
- Explicit `scope_headers` activate the per-header reachability sweep without
  requiring an unrelated directory scope.
- Flat per-header output preserves namespaces from referenced WinMD types and
  accepts generic defining-header library overrides.
- `remap_by_header` structurally maps flat per-header metadata into canonical
  header namespaces and repairs external TypeRef scopes through an RDL
  round trip. Header stems that are Rust keywords receive a trailing
  underscore.
- Function declarations enumerate their direct Clang arguments so parameter
  declarations nested under function-pointer return types are not duplicated.
