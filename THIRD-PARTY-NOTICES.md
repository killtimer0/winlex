# Third-party notices

Third-party components retain their own licenses. Cargo dependency versions are
recorded in `Cargo.lock`; source packages and their original copyright and license
texts are available from the linked crates.io pages. The list below includes
direct workspace dependencies for Windows x86/x64/ARM64.
Binary releases collect license materials for direct and transitive dependencies
in the accompanying `licenses.zip`.
Extract that archive alongside the program files to retain the relative paths
used below.

## Rust dependencies

| Components | License |
| --- | --- |
| [anyhow](https://crates.io/crates/anyhow), [cpp_demangle](https://crates.io/crates/cpp_demangle), [repc](https://crates.io/crates/repc) | MIT OR Apache-2.0 |
| [serde](https://crates.io/crates/serde), [serde_json](https://crates.io/crates/serde_json), [sha1](https://crates.io/crates/sha1) | MIT OR Apache-2.0 |
| [windows](https://crates.io/crates/windows), [windows-core](https://crates.io/crates/windows-core), [windows-metadata](https://crates.io/crates/windows-metadata), [windows-sys](https://crates.io/crates/windows-sys) | MIT OR Apache-2.0 |
| [winnow](https://crates.io/crates/winnow) | MIT |
| [msvc-demangler](https://crates.io/crates/msvc-demangler) | MIT OR NCSA |
| [nucleo-matcher](https://crates.io/crates/nucleo-matcher) | MPL-2.0 |

The patched `windows-metadata` 0.100.0 copy comes from Microsoft's
[windows-rs](https://github.com/microsoft/windows-rs). Its original
[MIT](vendor/windows-metadata/license-mit) and
[Apache-2.0](vendor/windows-metadata/license-apache-2.0) texts are retained;
provenance and local changes are described in [vendor/PATCHES.md](vendor/PATCHES.md).

`nucleo-matcher` 0.3.1 is unmodified. Its source and MPL-2.0 text are available in
the [source package](https://crates.io/crates/nucleo-matcher/0.3.1) and
[upstream repository](https://github.com/helix-editor/nucleo).

Rust's standard library is primarily MIT OR Apache-2.0, with additional notices
for incorporated components. The matching Rust installation provides these in
`share/doc/rust/COPYRIGHT-library.html` and `share/doc/rust/licenses/`.

## Native dark-mode implementation

The Rust implementation in `crates/winlex-gui/src/theme.rs` and
`crates/winlex-gui/src/theme/`
is adapted from [win32-darkmodelib](https://github.com/ozone10/win32-darkmodelib/tree/e8ec95ba631462f01e238e63f7d06c1e6829a7a1),
revision `e8ec95ba631462f01e238e63f7d06c1e6829a7a1`.
Copyright (c) 2025-2026 ozone10. The port replaces the C++ infrastructure with
Rust ownership and Winlex's Windows bindings, preference policy and MDI integration.
Its source is distributed in this repository under MPL-2.0; the complete
[license](licenses/win32-darkmodelib-MPL-2.0.md) is included.
The underlying win32-darkmode and UAH menu work retains its
[win32-darkmode MIT notice](licenses/win32-darkmode-MIT.md) and
[UAHMenuBar MIT notice](licenses/win32-darkmodelib-UAHMenuBar.md).

## Windows compatibility libraries

All architectures use VC-LTL: x86/x64 share the system MSVCRT, while ARM64 shares
the system UCRT. Only x86/x64 link YY-Thunks compatibility objects.

- [YY-Thunks 1.2.3](https://github.com/Chuyu-Team/YY-Thunks/tree/v1.2.3):
  Copyright (c) 2018 Chuyu-Team, MIT. The linked compatibility objects come from
  the official release archive; the [license text](licenses/YY-Thunks-LICENSE)
  is retained in this repository.
- [VC-LTL 5.3.1](https://github.com/Chuyu-Team/VC-LTL5/tree/v5.3.1):
  the upstream [LICENSE](https://github.com/Chuyu-Team/VC-LTL5/blob/v5.3.1/LICENSE)
  is EPL-2.0. The source also includes Microsoft SDK terms under
  `Sources/ucrt/Licenses/`. The libraries are linked into Winlex.
  The upstream [EPL text](licenses/VC-LTL-LICENSE),
  [SDK terms](licenses/VC-LTL-sdk-license.rtf) and
  [SDK notices](licenses/VC-LTL-sdk-third-party-notices.rtf) are retained unchanged.

## External metadata

`Microsoft.Windows.SDK.Win32Metadata` 71.0.37-preview is downloaded separately
under its [Windows SDK terms](https://www.nuget.org/packages/Microsoft.Windows.SDK.Win32Metadata/71.0.37-preview/License).

## Codicons

Winlex uses selected [Microsoft Codicons](https://github.com/microsoft/vscode-codicons)
from revision `5a6ad49557af022f97987cb3cfc3412b5d03f554`.
Copyright Microsoft Corporation and contributors. The icons are licensed under
[Creative Commons Attribution 4.0 International](https://creativecommons.org/licenses/by/4.0/).
The selected SVG paths are unchanged; their fills are converted to white in the
EMF+ resources for runtime tinting. The license text is included in
`crates/winlex-gui/res/codicons/LICENSE`.

## Development tools

[resvg-js 2.6.2](https://github.com/yisibl/resvg-js/tree/v2.6.2) is used to render
the application icon under MPL-2.0. It is a development dependency.

GUI tests use AutoItX under its separate AutoIt EULA and PyAutoIt under MIT.
Their versions, binary provenance and complete license texts are retained in
[tests/gui/licenses](tests/gui/licenses/THIRD-PARTY-NOTICES.md).
