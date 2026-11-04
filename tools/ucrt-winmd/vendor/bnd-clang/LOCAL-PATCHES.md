# Local query-oriented changes

Based on bnd-clang 0.0.8 from bnd commit
`c88a975fb4778fd253d193926e25ec12e08f4ef9`, retaining its licenses and the vendored
frontend changes described in `VENDORED.md`. Cargo workspace metadata is replaced
with standalone package values.

Changes in `vendored/windows-clang/src/`:

- `canon.rs`, `typedef.rs`: preserve selected CRT semantic aliases at definition
  and reference sites instead of collapsing them to ABI-equivalent primitives.
  The backing definitions of `size_t`, `uintptr_t`, `intptr_t` and `ptrdiff_t`
  use native-sized integers on all architectures, retaining named references and
  the `rsize_t` to `size_t` alias chain.
- `collector.rs`: store free functions separately from same-named record tags;
  filtering continues to use public symbol names. `scope.rs` builds reachability
  edges from public names rather than internal collector keys.
- `lib.rs`: retain free function definitions as declarations without their bodies.
- `callback.rs`: mark unannotated parameter directions as unspecified for the
  patched RDL encoder instead of inferring output from mutable pointer shape.

These are local query-metadata policies, not upstream binding behavior.
