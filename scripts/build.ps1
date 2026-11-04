#requires -Version 7.2
[CmdletBinding()]
param(
    [string]$YyThunksObject,
    [string]$LinkExe,
    [string]$VcLtlRoot,
    [ValidateSet('x86','x64','arm64')][string]$Architecture = 'x86',
    [ValidateSet('Debug','Release','Dist')][string]$Configuration = 'Debug',
    [string]$TargetDir,
    [string]$RcExe
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$policy = (Get-Content -LiteralPath "$root/inputs/targets.json" -Raw | ConvertFrom-Json).$Architecture
$ltl = Get-Content -LiteralPath "$root/inputs/vc-ltl-source.json" -Raw | ConvertFrom-Json
if (-not $VcLtlRoot) { $VcLtlRoot = "$root/tmp/deps/vc-ltl-$($ltl.version)/sdk" }
if ($policy.thunks_arch -and -not $YyThunksObject) {
    $thunks = Get-Content -LiteralPath "$root/inputs/yy-thunks-source.json" -Raw | ConvertFrom-Json
    $YyThunksObject = "$root/tmp/deps/yy-thunks-$($thunks.version)/$($policy.thunks_arch)/YY_Thunks_for_WinXP.obj"
}
if (-not $LinkExe) {
    . "$PSScriptRoot/build-tools.ps1"
    $tools = Initialize-WinlexToolchain $Architecture $policy
    $LinkExe = $tools.LinkExe
    if (-not $RcExe) { $RcExe = $tools.RcExe }
}
$linker = (Resolve-Path -LiteralPath $LinkExe).Path
$object = $null
$ltlRoot = (Resolve-Path -LiteralPath $VcLtlRoot).Path
if ($policy.thunks_arch) {
    if (-not $YyThunksObject) {
        throw 'Specify -YyThunksObject for x86 and x64 builds.'
    }
    $object = (Resolve-Path -LiteralPath $YyThunksObject).Path
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
if (-not $RcExe -and $Configuration -ne 'Dist') {
    $rcCommand = Get-Command rc.exe -ErrorAction SilentlyContinue
    if ($rcCommand) { $RcExe = $rcCommand.Source }
    else {
        $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/bin'
        $RcExe = Get-ChildItem "$kits/*/x64/rc.exe" -ErrorAction SilentlyContinue |
            Sort-Object FullName -Descending |
            Select-Object -First 1 -ExpandProperty FullName
    }
}
if (-not $RcExe) { throw 'Microsoft rc.exe was not found in the SDK environment. Specify -RcExe or omit -LinkExe to initialize the toolchain.' }
$rc = (Resolve-Path -LiteralPath $RcExe).Path
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
$target = $policy.target
$checkArguments = @("$PSScriptRoot/prepare-inputs.py", '--check', '--architecture', $Architecture, '--vc-ltl-root', $ltlRoot)
if ($object) { $checkArguments += @('--thunks-object', $object) }
$checkedInputs = & python @checkArguments
if ($LASTEXITCODE -ne 0) { throw 'Build dependency verification failed.' }
$checkedInputs = $checkedInputs | ConvertFrom-Json
$identity = & python "$PSScriptRoot/build-record.py"
if ($LASTEXITCODE -ne 0) { throw 'Unable to capture build source and toolchain identity.' }
$identity = $identity | ConvertFrom-Json
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
    if ($oldFlags -or $oldRustFlags) { throw 'Clear existing RUSTFLAGS/CARGO_ENCODED_RUSTFLAGS before this build.' }
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
        if ($message.reason -eq 'compiler-message' -and $message.message.rendered) {
            [Console]::Error.Write($message.message.rendered)
        }
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
    foreach ($property in $policy.PSObject.Properties) {
        if ($compatibility.($property.Name) -cne $property.Value) {
            throw "Build compatibility metadata policy mismatch: $($property.Name)"
        }
    }
    if (-not $compatibility.vc_ltl_platform) { throw 'Build compatibility metadata is missing the VC-LTL platform.' }
    $after = & python "$PSScriptRoot/build-record.py"
    if ($LASTEXITCODE -ne 0 -or ($after | ConvertFrom-Json | ConvertTo-Json -Depth 5 -Compress) -cne
        ($identity | ConvertTo-Json -Depth 5 -Compress)) {
        throw 'Source or toolchain changed during the build.'
    }
    & python @checkArguments | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Build dependencies changed during the build.' }
    $buildId = [guid]::NewGuid().ToString()
    foreach ($binary in @('winlex', 'winlex-gui')) {
        $exe = $executables[$binary]
        if (-not $exe) { throw "Cargo did not report an executable for $binary" }
        $report = @{
            schema_version = 1
            build_id = $buildId
            source = $identity.source
            target = $target
            configuration = $Configuration
            rustc = $identity.rustc
            cargo = $identity.cargo
            rust_licenses_sha256 = $identity.rust_licenses_sha256
            sdk_version = $env:WindowsSDKVersion
            policy = $policy
            linker = $linker
            linker_version = (Get-Item $linker).VersionInfo.FileVersion
            resource_compiler = $rc
            resource_compiler_version = (Get-Item $rc).VersionInfo.FileVersion
            minimum_windows = $compatibility.minimum_windows
            executable = $exe
            executable_sha256 = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLowerInvariant()
        }
        $report.crt = "VC-LTL5 system $($policy.crt_mode)"
        if ($policy.thunks_arch) {
            $report.thunks_object = $object
            $report.thunks_sha256 = $checkedInputs.thunks_sha256
        }
        $report.vc_ltl_root = $ltlRoot
        $report.vc_ltl_libraries = $checkedInputs.vc_ltl_libraries
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
