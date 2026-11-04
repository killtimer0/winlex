# Third-party components

Retain these notices and bundled license texts when distributing source copies.

| Component | Baseline | License texts |
| --- | --- | --- |
| bnd-clang / bnd | 0.0.8, bnd commit `c88a975fb4778fd253d193926e25ec12e08f4ef9` | `vendor/bnd-clang/LICENSE` (MIT, youyuanwu) |
| bnd's vendored windows-clang | See `vendor/bnd-clang/VENDORED.md` | `vendor/bnd-clang/vendored/windows-clang/license-mit`, `license-apache-2.0` |
| windows-rdl | 0.100.0 | `vendor/windows-rdl/license-mit`, `license-apache-2.0` |
| windows-metadata | 0.100.0 | `vendor/windows-metadata/license-mit`, `license-apache-2.0` |

Local changes are described in the component directories. Additional Cargo
dependencies and exact resolved versions are recorded in `Cargo.lock`; they
are downloaded through Cargo and are not included as source in this archive.

Windows SDK and MSVC headers must be obtained separately from Microsoft under
their applicable terms. LLVM/Clang, Rust, PowerShell, Python and optional Winlex
are external prerequisites, not redistributed in the source archive.

These dependency licenses do not by themselves establish the redistribution
terms for metadata derived from Microsoft headers. Review those terms separately
before distributing generated WinMD files. The generator's original source is
licensed under GPL-3.0-only; see `LICENSE`.
