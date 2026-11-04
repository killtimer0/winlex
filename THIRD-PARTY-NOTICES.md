# Third-party notices

Third-party components retain their own licenses. Cargo dependency versions are
recorded in `Cargo.lock`; source packages and their original copyright and license
texts are available from the linked crates.io pages. The list below includes
normal and build dependencies for Windows x86/x64, not just runtime libraries.
When distributing binaries, retain the applicable copyright notices and license
texts alongside this document.

## Rust dependencies

| Components | License |
| --- | --- |
| [anyhow](https://crates.io/crates/anyhow), [block-buffer](https://crates.io/crates/block-buffer), [cfg-if](https://crates.io/crates/cfg-if), [cpufeatures](https://crates.io/crates/cpufeatures), [crypto-common](https://crates.io/crates/crypto-common), [digest](https://crates.io/crates/digest) | MIT OR Apache-2.0 |
| [itoa](https://crates.io/crates/itoa), [proc-macro2](https://crates.io/crates/proc-macro2), [quote](https://crates.io/crates/quote), [repc](https://crates.io/crates/repc), [repc-impl](https://crates.io/crates/repc-impl) | MIT OR Apache-2.0 |
| [serde](https://crates.io/crates/serde), [serde_core](https://crates.io/crates/serde_core), [serde_derive](https://crates.io/crates/serde_derive), [serde_json](https://crates.io/crates/serde_json), [sha1](https://crates.io/crates/sha1), [syn](https://crates.io/crates/syn), [typenum](https://crates.io/crates/typenum), [version_check](https://crates.io/crates/version_check) | MIT OR Apache-2.0 |
| [windows](https://crates.io/crates/windows), [windows-core](https://crates.io/crates/windows-core), [windows-implement](https://crates.io/crates/windows-implement), [windows-interface](https://crates.io/crates/windows-interface), [windows-metadata](https://crates.io/crates/windows-metadata), [windows-result](https://crates.io/crates/windows-result), [windows-strings](https://crates.io/crates/windows-strings), [windows-sys](https://crates.io/crates/windows-sys), [windows-targets](https://crates.io/crates/windows-targets), [windows_i686_msvc](https://crates.io/crates/windows_i686_msvc), [windows_x86_64_msvc](https://crates.io/crates/windows_x86_64_msvc) | MIT OR Apache-2.0 |
| [generic-array](https://crates.io/crates/generic-array), [winnow](https://crates.io/crates/winnow), [zmij](https://crates.io/crates/zmij) | MIT |
| [memchr](https://crates.io/crates/memchr) | Unlicense OR MIT |
| [unicode-ident](https://crates.io/crates/unicode-ident) | (MIT OR Apache-2.0) AND Unicode-3.0 |
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

## Windows compatibility libraries

- [YY-Thunks 1.2.3](https://github.com/Chuyu-Team/YY-Thunks/tree/v1.2.3):
  Copyright (c) 2018 Chuyu-Team, MIT. The linked compatibility objects come from
  the official release archive; the [license text](licenses/YY-Thunks-LICENSE)
  is retained in this repository.
- [VC-LTL 5.3.1](https://github.com/Chuyu-Team/VC-LTL5/tree/v5.3.1):
  the upstream [LICENSE](https://github.com/Chuyu-Team/VC-LTL5/blob/v5.3.1/LICENSE)
  is EPL-2.0. The source also includes Microsoft SDK terms under
  `Sources/ucrt/Licenses/`. The libraries are linked into Winlex; their applicable
  object-level grants and GPL combination terms remain to be confirmed.

## External metadata

`Microsoft.Windows.SDK.Win32Metadata` 71.0.37-preview is downloaded separately
under its [Windows SDK terms](https://www.nuget.org/packages/Microsoft.Windows.SDK.Win32Metadata/71.0.37-preview/License).
These are distinct from the Rust `windows-metadata` library's licenses.

## UCRT metadata generator

The independently built tool in `tools/ucrt-winmd/` vendors patched bnd-clang,
windows-rdl and windows-metadata. Their baselines, licenses and local changes are
documented in [the tool's notices](tools/ucrt-winmd/THIRD-PARTY-NOTICES.md).
Microsoft headers and generated metadata are not bundled with the tool's source.

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
the application icon under MPL-2.0. It is a development dependency, not bundled
with Winlex; its license does not by itself apply to rendered images.
