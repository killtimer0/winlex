#requires -Version 7.2
[CmdletBinding()]
param(
    [string]$YyThunksObject,
    [Parameter(Mandatory=$true)][string]$LinkExe,
    [string]$VcLtlRoot,
    [ValidateSet('x86','x64','arm64')][string]$Architecture = 'x86',
    [ValidateSet('Debug','Release','Dist')][string]$Configuration = 'Debug',
    [string]$TargetDir,
    [string]$RcExe
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$linker = (Resolve-Path $LinkExe).Path
$object = $null
$ltlRoot = $null
if ($Architecture -ne 'arm64') {
    if (-not $YyThunksObject -or -not $VcLtlRoot) {
        throw 'Specify -YyThunksObject and -VcLtlRoot for x86 and x64 builds.'
    }
    $object = (Resolve-Path $YyThunksObject).Path
    $ltlRoot = (Resolve-Path $VcLtlRoot).Path
}
if (-not $RcExe) {
    $sdkBins = @(
        $env:WindowsSdkVerBinPath
        if ($env:WindowsSdkDir -and $env:WindowsSDKVersion) {
            Join-Path $env:WindowsSdkDir "bin/$($env:WindowsSDKVersion.TrimEnd('\'))"
        }
    )
    $RcExe = $sdkBins | Where-Object { $_ } | ForEach-Object {
        Join-Path $_ 'x64/rc.exe'
    } | Where-Object { Test-Path $_ -PathType Leaf } | Select-Object -First 1
}
if (-not $RcExe) {
    $rcCommand = Get-Command rc.exe -ErrorAction SilentlyContinue
    if ($rcCommand) { $RcExe = $rcCommand.Source }
    else {
        $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/bin'
        $RcExe = Get-ChildItem "$kits/*/x64/rc.exe" -ErrorAction SilentlyContinue |
            Sort-Object FullName -Descending |
            Select-Object -First 1 -ExpandProperty FullName
    }
}
if (-not $RcExe) { throw 'Microsoft rc.exe was not found. Specify -RcExe from a Windows SDK.' }
$rc = (Resolve-Path $RcExe).Path
if ((Get-Item $rc).VersionInfo.CompanyName -notmatch 'Microsoft') { throw 'Use Microsoft rc.exe.' }
if ([IO.Path]::GetFileName($linker) -ine 'link.exe') {
    throw 'Use Microsoft MSVC link.exe. YY-Thunks uses a different integration for lld-link.'
}
if ((Get-Item $linker).VersionInfo.CompanyName -notmatch 'Microsoft') {
    throw 'The supplied link.exe is not identified as a Microsoft linker.'
}
$linkerDirectory = Split-Path $linker -Parent
if ((Split-Path $linkerDirectory -Leaf) -ine $Architecture -or
    (Split-Path (Split-Path $linkerDirectory -Parent) -Leaf) -ine 'Hostx64') {
    throw "Use the Hostx64/$Architecture Microsoft linker for this target."
}
$target = switch ($Architecture) {
    'x86' { 'i686-pc-windows-msvc' }
    'x64' { 'x86_64-pc-windows-msvc' }
    'arm64' { 'aarch64-pc-windows-msvc' }
}
$libdir = & rustc --print target-libdir --target $target
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $libdir)) {
    throw "Rust standard library for $target is missing. Install a Rust MSVC toolchain and this target first."
}
$oldFlags = $env:CARGO_ENCODED_RUSTFLAGS
$oldRustFlags = $env:RUSTFLAGS
$oldObject = $env:WINLEX_YY_THUNKS_OBJ
$oldLtl = $env:WINLEX_VC_LTL_ROOT
$oldRc = $env:WINLEX_RC_EXE
try {
    if ($oldFlags -or $oldRustFlags) { throw 'Clear existing RUSTFLAGS/CARGO_ENCODED_RUSTFLAGS before this reproducible build.' }
    $flags = @('-C','target-feature=+crt-static','-C',"linker=$linker")
    if ($Configuration -eq 'Dist') {
        $sysroot = & rustc --print sysroot
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path $sysroot)) { throw 'Rust sysroot was not found.' }
        $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
        $remappings = @(
            @($root, '/src/winlex'),
            @($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($cargoHome), '/cargo'),
            @($sysroot, '/rust-toolchain')
        )
        foreach ($mapping in $remappings) {
            foreach ($prefix in @($mapping[0].Replace('/', '\'), $mapping[0].Replace('\', '/')) | Select-Object -Unique) {
                $flags += "--remap-path-prefix=$prefix=$($mapping[1])"
            }
        }
    }
    $env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]31
    $env:WINLEX_YY_THUNKS_OBJ = $object
    $env:WINLEX_VC_LTL_ROOT = $ltlRoot
    $env:WINLEX_RC_EXE = $rc
    $arguments = @(
        'build', '--manifest-path', "$root/Cargo.toml", '--package', 'winlex',
        '--target', $target, '--locked', '--message-format=json-render-diagnostics'
    )
    if ($Configuration -eq 'Release') { $arguments += '--release' }
    if ($Configuration -eq 'Dist') { $arguments += @('--profile', 'dist') }
    if ($TargetDir) { $arguments += @('--target-dir', $TargetDir) }
    $executables = @{}
    $buildOutputs = @{}
    $packageId = $null
    & cargo @arguments | ForEach-Object {
        $message = $_ | ConvertFrom-Json
        if ($message.reason -eq 'build-script-executed') {
            $buildOutputs[$message.package_id] = $message.out_dir
        }
        if ($message.reason -eq 'compiler-artifact' -and $message.executable -and
            $message.target.kind -contains 'bin' -and $message.target.name -in @('winlex', 'winlex-gui')) {
            $executables[$message.target.name] = $message.executable
            $packageId = $message.package_id
        }
    }
    if ($LASTEXITCODE -ne 0) { throw "Build failed ($LASTEXITCODE)" }
    if (-not $packageId -or -not $buildOutputs.ContainsKey($packageId)) {
        throw 'Cargo did not report the executable package build output.'
    }
    $compatibility = Get-Content (Join-Path $buildOutputs[$packageId] 'compatibility.json') -Raw | ConvertFrom-Json
    if ($compatibility.target -ne $target) { throw 'Build compatibility metadata target mismatch.' }
    if ($Architecture -eq 'arm64') {
        if ($compatibility.vc_ltl_platform -or $compatibility.minimum_windows -ne '10.0.17763.0') {
            throw 'Build compatibility metadata does not match the ARM64 policy.'
        }
    } else {
        if (-not $compatibility.vc_ltl_platform) { throw 'Build compatibility metadata is missing the VC-LTL platform.' }
        $ltlLib = Join-Path $ltlRoot "TargetPlatform/$($compatibility.vc_ltl_platform)"
    }
    foreach ($binary in @('winlex', 'winlex-gui')) {
        $exe = $executables[$binary]
        if (-not $exe) { throw "Cargo did not report an executable for $binary" }
        $report = @{
            target = $target
            configuration = $Configuration
            rustc = (& rustc -Vv) -join "`n"
            linker = $linker
            linker_version = (Get-Item $linker).VersionInfo.FileVersion
            resource_compiler = $rc
            resource_compiler_version = (Get-Item $rc).VersionInfo.FileVersion
            minimum_windows = $compatibility.minimum_windows
            executable = $exe
            executable_sha256 = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLowerInvariant()
        }
        if ($Architecture -eq 'arm64') {
            $report.crt = 'static MSVC CRT'
        } else {
            $report.thunks_object = $object
            $report.thunks_sha256 = (Get-FileHash $object -Algorithm SHA256).Hash.ToLowerInvariant()
            $report.crt = 'VC-LTL5 system msvcrt'
            $report.vc_ltl_root = $ltlRoot
            $report.vc_ltl_libraries = @(Get-ChildItem "$ltlLib/*.lib" | ForEach-Object {
                @{ name = $_.Name; sha256 = (Get-FileHash $_.FullName).Hash.ToLowerInvariant() }
            })
        }
        $report | ConvertTo-Json -Depth 5 | Set-Content "$exe.build.json" -Encoding utf8
        Write-Host "Built $exe"
    }
} finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $oldFlags
    $env:RUSTFLAGS = $oldRustFlags
    $env:WINLEX_YY_THUNKS_OBJ = $oldObject
    $env:WINLEX_VC_LTL_ROOT = $oldLtl
    $env:WINLEX_RC_EXE = $oldRc
}
