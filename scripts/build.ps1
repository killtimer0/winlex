[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$YyThunksObject,
    [Parameter(Mandatory=$true)][string]$LinkExe,
    [Parameter(Mandatory=$true)][string]$VcLtlRoot,
    [ValidateSet('x86','x64')][string]$Architecture = 'x86'
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$object = (Resolve-Path $YyThunksObject).Path
$linker = (Resolve-Path $LinkExe).Path
$ltlRoot = (Resolve-Path $VcLtlRoot).Path
$minimum = if ($Architecture -eq 'x86') { '5.1.2600.0' } else { '5.2.3790.0' }
$ltlArch = if ($Architecture -eq 'x86') { 'Win32' } else { 'x64' }
$ltlLib = Join-Path $ltlRoot "TargetPlatform/$minimum/lib/$ltlArch"
foreach ($name in @('libucrt.lib', 'libvcruntime.lib')) {
    if (-not (Test-Path "$ltlLib/$name")) { throw "Missing VC-LTL library: $ltlLib/$name" }
}
if ([IO.Path]::GetFileName($linker) -ine 'link.exe') {
    throw 'Use Microsoft MSVC link.exe. YY-Thunks uses a different integration for lld-link.'
}
if ((Get-Item $linker).VersionInfo.CompanyName -notmatch 'Microsoft') {
    throw 'The supplied link.exe is not identified as a Microsoft linker.'
}
$target = if ($Architecture -eq 'x86') { 'i686-pc-windows-msvc' } else { 'x86_64-pc-windows-msvc' }
$libdir = & rustc --print target-libdir --target $target
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $libdir)) {
    throw "Rust standard library for $target is missing. Install a Rust MSVC toolchain and this target first."
}
$oldFlags = $env:CARGO_ENCODED_RUSTFLAGS
$oldRustFlags = $env:RUSTFLAGS
$oldObject = $env:WINLEX_YY_THUNKS_OBJ
$oldLtl = $env:WINLEX_VC_LTL_ROOT
try {
    if ($oldFlags -or $oldRustFlags) { throw 'Clear existing RUSTFLAGS/CARGO_ENCODED_RUSTFLAGS before this reproducible build.' }
    $env:CARGO_ENCODED_RUSTFLAGS = @('-C','target-feature=+crt-static','-C',"linker=$linker") -join [char]31
    $env:WINLEX_YY_THUNKS_OBJ = $object
    $env:WINLEX_VC_LTL_ROOT = $ltlRoot
    & cargo build --manifest-path "$root/Cargo.toml" --package winlex --target-dir "$root/target" --target $target --release --locked
    if ($LASTEXITCODE -ne 0) { throw "Build failed ($LASTEXITCODE)" }
    $exe = "$root/target/$target/release/winlex.exe"
    $report = @{
        target = $target
        rustc = (& rustc -Vv) -join "`n"
        linker = $linker
        linker_version = (Get-Item $linker).VersionInfo.FileVersion
        thunks_object = $object
        thunks_sha256 = (Get-FileHash $object -Algorithm SHA256).Hash.ToLowerInvariant()
        minimum_windows = $minimum
        crt = 'VC-LTL5 system msvcrt'
        vc_ltl_root = $ltlRoot
        vc_ltl_libraries = @(Get-ChildItem "$ltlLib/*.lib" | ForEach-Object {
            @{ name = $_.Name; sha256 = (Get-FileHash $_.FullName).Hash.ToLowerInvariant() }
        })
        executable = $exe
        executable_sha256 = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    $report | ConvertTo-Json -Depth 5 | Set-Content "$exe.build.json" -Encoding utf8
    Write-Host "Built $exe"
} finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $oldFlags
    $env:RUSTFLAGS = $oldRustFlags
    $env:WINLEX_YY_THUNKS_OBJ = $oldObject
    $env:WINLEX_VC_LTL_ROOT = $oldLtl
}
