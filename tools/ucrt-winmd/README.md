# UCRT declaration metadata generator (experimental)

Generate one query-oriented WinMD for x86, x64 and ARM64 from Microsoft UCRT
headers, with required VCRuntime support definitions. No import or static library
is required.

This is an independent Cargo workspace with its own lockfile and local patched
dependencies. It is not part of the Winlex default build and can be used without
a Winlex checkout. The modern build-host requirements below are separate from
Winlex's Windows XP runtime compatibility.

## Prerequisites

- A 64-bit Windows host with PowerShell 7.2+ (`pwsh`).
- Rust 1.95+ and Cargo, with the MSVC host toolchain and its linker available.
  Cargo resolves the Rust dependencies using the included `Cargo.lock`.
- Visual Studio or Build Tools with MSVC C++ tools and a Windows 10/11 SDK.
- Matching 64-bit Clang and libclang 18+; `clang.exe` on PATH and `libclang.dll`
  beside it, or explicit paths as shown below. LLVM and MSYS2 Clang distributions
  can be used; parsing always targets MSVC, not the MinGW ABI.
- Python 3.10+ only for `-Check` or optional Winlex integration checks.

Clang and libclang must have matching versions and loadable runtime dependencies.
Dependency sources and licenses are listed in `THIRD-PARTY-NOTICES.md`.

## Generate

Run in PowerShell, from an extracted source directory:

```powershell
./build.ps1
```

The script discovers installed SDK/MSVC headers and Clang, then generates all
three architecture baselines and the combined artifact.

To select particular versions or nonstandard installations:

```powershell
./build.ps1 -SdkIncludePath 'D:/SDK/Include/10.0.x.y' `
  -MsvcIncludePath 'D:/VS/VC/Tools/MSVC/14.x.y/include' `
  -ClangPath 'D:/LLVM/bin/clang.exe' -LibClangPath 'D:/LLVM/bin'
```

`SdkIncludePath` is the versioned directory containing `ucrt/` and `shared/`.
`MsvcIncludePath` contains `vcruntime.h` and `vadefs.h`. Without overrides, SDK
discovery uses `WindowsSdkDir` or the standard Windows Kits directory; MSVC
discovery uses `VCToolsInstallDir` or Visual Studio's `vswhere`. The newest
complete version is selected. Pin explicit paths when comparing different runs.

Outputs:

- `out/UCRT.winmd`: combined x86/x64/ARM64 query metadata.
- `out/{x86,x64,arm64}/UCRT.winmd`: explicitly architecture-tagged baselines.
- `out/**/UCRT.roundtrip.winmd`: metadata regenerated through RDL.
- `out/manifest.json`: tool versions, input and artifact hashes, and check results.
- `out/*.log`: build and optional check logs.

Manifests include local header paths; review them before sharing. Generated
declarations depend on the selected SDK and compiler versions.

## Optional checks

Maintainer checks do not use Winlex:

```powershell
./build.ps1 -Check
```

This runs Rust and toolchain-script contracts, an independent Clang AST audit
and Clippy. Use `-PythonPath` when Python is not on PATH. The contracts compare
metadata across architecture baselines, combined output and RDL round trips.
The audit reports declaration coverage rather than proving complete C type or ABI
equivalence. Python scripts must run without `-O` so their assertions execute.

To check queries and layouts with Winlex, supply a compatible CLI:

```powershell
./build.ps1 -Check -WinlexPath 'D:/Tools/winlex.exe'
```

Standalone commands are
`python verify.py --winlex PATH` and
`python verify_multiarch.py --winlex PATH`. The latter accepts `--previous DIR`
to compare against archived architecture baselines.

## Metadata conventions and coverage

- Free functions are native, unmanaged forward declarations without PInvokeImpl,
  ImplMap, ModuleRef or method bodies. This is query metadata, not an executable
  assembly or a directly linkable binding package.
- `Winlex.Metadata.NativeCallingConventionAttribute(String)` preserves C/system/
  fastcall independently from varargs. This is a local convention, not an upstream
  WinMD standard understood by every consumer.
- Named semantic typedefs are preserved. `size_t`/`uintptr_t` use native unsigned
  integers and `intptr_t`/`ptrdiff_t` native signed integers. `rsize_t` references
  `size_t`; ordinary fixed-width integers are not normalized this way.
- Identical definitions are shared; actual differences retain
  `Windows.Win32.Metadata.SupportedArchitectureAttribute` masks (1=X86, 2=X64,
  4=Arm64). No mask means all three. Consumers must select the requested
  architecture before resolving same-named variants or calculating layout.
- Header inline functions are retained only as declarations. Unannotated
  declaration/callback parameters retain unspecified direction, including after
  round trips; complete SAL recovery is not claimed.
- The baseline is public C headers plus Microsoft extensions and necessary
  VCRuntime support, not C++ STL or managed CRT. External variables are not emitted,
  macro values are not comprehensively evaluated, and alternate time/debug/Unicode
  macro configurations are not combined.

## Source packaging

```powershell
./package-source.ps1 -Destination 'D:/Packages/ucrt-winmd-source.zip'
```

The destination must be new. The archive includes source, lockfile, tests, local
dependencies and notices, but not generated outputs, local baselines, executables
or Microsoft headers. See `RELEASE.md` for licensing and clean-environment release
requirements.

## License and dependency maintenance

Original generator code is licensed under [GPL-3.0-only](LICENSE). Third-party
components retain their own licenses; see `THIRD-PARTY-NOTICES.md`. This does not
determine redistribution rights for metadata derived from Microsoft headers.

The local `vendor/windows-metadata` copy carries generation/merge patches and is
independent of Winlex's reader-oriented copy. Changes shared between them must
preserve and validate both patch sets.
