function Resolve-CrtExecutable([string]$Name) {
    $command = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $command) { throw "Executable not found: $Name. Install it or specify an explicit path." }
    return $command.Source
}

function Resolve-CrtInclude([string]$Path, [string[]]$Headers, [string]$Option) {
    if (-not $Path -or -not (Test-Path -LiteralPath $Path -PathType Container)) {
        throw "Include directory not found. Specify -$Option."
    }
    $resolved = (Resolve-Path -LiteralPath $Path).ProviderPath
    foreach ($header in $Headers) {
        if (-not (Test-Path -LiteralPath (Join-Path $resolved $header) -PathType Leaf)) {
            throw "Missing $header under $resolved. Check -$Option."
        }
    }
    return $resolved
}

function Find-CrtVersionedInclude([string]$Root, [string]$Suffix, [string]$Header) {
    if (-not (Test-Path -LiteralPath $Root -PathType Container)) { return $null }
    $versions = Get-ChildItem -LiteralPath $Root -Directory | Where-Object { $_.Name -match '^\d+(\.\d+)+$' } | Sort-Object { [version]$_.Name } -Descending
    foreach ($version in $versions) {
        $candidate = if ($Suffix) { Join-Path $version.FullName $Suffix } else { $version.FullName }
        if (Test-Path -LiteralPath (Join-Path $candidate $Header) -PathType Leaf) { return $candidate }
    }
    return $null
}

function Get-CrtToolchain {
    param([string]$SdkIncludePath, [string]$MsvcIncludePath, [string]$ClangPath, [string]$LibClangPath)
    if (-not $IsWindows) { throw 'This generator currently requires a Windows host.' }
    $cargo = Resolve-CrtExecutable 'cargo'
    $rustc = Resolve-CrtExecutable 'rustc'
    $rustVersion = & $rustc --version
    if ($LASTEXITCODE -ne 0 -or $rustVersion -notmatch '^rustc (\d+\.\d+\.\d+)') { throw 'Cannot determine Rust version.' }
    if ([version]$Matches[1] -lt [version]'1.95.0') { throw 'Rust 1.95+ is required by the local generator dependencies.' }
    if (-not $SdkIncludePath) {
        $kits = if ($env:WindowsSdkDir) { $env:WindowsSdkDir } else { Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10' }
        $SdkIncludePath = Find-CrtVersionedInclude (Join-Path $kits 'Include') '' 'ucrt/stdio.h'
    }
    if (-not $MsvcIncludePath -and $env:VCToolsInstallDir) { $MsvcIncludePath = Join-Path $env:VCToolsInstallDir 'include' }
    if (-not $MsvcIncludePath) {
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
        if (Test-Path -LiteralPath $vswhere) {
            $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
            if ($LASTEXITCODE -ne 0) { throw 'Visual Studio discovery failed; specify -MsvcIncludePath.' }
            if ($vs) { $MsvcIncludePath = Find-CrtVersionedInclude (Join-Path $vs 'VC/Tools/MSVC') 'include' 'vcruntime.h' }
        }
    }
    $sdk = Resolve-CrtInclude $SdkIncludePath @('ucrt/stdio.h', 'shared/sdkddkver.h') 'SdkIncludePath'
    $msvc = Resolve-CrtInclude $MsvcIncludePath @('vcruntime.h', 'vadefs.h', 'vcruntime_startup.h') 'MsvcIncludePath'
    $clang = Resolve-CrtExecutable $ClangPath
    $clangVersion = & $clang --version
    if ($LASTEXITCODE -ne 0 -or $clangVersion[0] -notmatch 'clang version (\d+)') { throw 'Cannot determine Clang version.' }
    if ([int]$Matches[1] -lt 18) { throw 'Clang/libclang 18+ is required.' }
    if (-not $LibClangPath) {
        $LibClangPath = if ($env:LIBCLANG_PATH) { $env:LIBCLANG_PATH } else { Split-Path $clang }
    }
    $libclang = Resolve-CrtInclude $LibClangPath @('libclang.dll') 'LibClangPath'
    $cargoVersion = & $cargo --version
    if ($LASTEXITCODE -ne 0) { throw 'Cannot determine Cargo version.' }
    return [ordered]@{
        schema_version = 1
        powershell_version = $PSVersionTable.PSVersion.ToString()
        sdk_include = $sdk
        msvc_include = $msvc
        clang_path = $clang
        libclang_path = $libclang
        clang_version = $clangVersion[0]
        clang_sha256 = (Get-FileHash -LiteralPath $clang -Algorithm SHA256).Hash.ToLowerInvariant()
        libclang_sha256 = (Get-FileHash -LiteralPath (Join-Path $libclang 'libclang.dll') -Algorithm SHA256).Hash.ToLowerInvariant()
        rustc_version = $rustVersion
        cargo_version = $cargoVersion
        clang_args = @('-x', 'c', '-std=c17', '-fno-builtin', '-D_NO_CRT_STDIO_INLINE=1', "-I$(Join-Path $sdk 'ucrt')", "-I$(Join-Path $sdk 'shared')", "-I$msvc")
    }
}

function Invoke-CrtStep([string]$Name, [string]$Executable, [string[]]$Arguments) {
    $log = Join-Path 'out' "$Name.log"
    Write-Host "$Name..."
    & $Executable @Arguments *> $log
    if ($LASTEXITCODE -ne 0) {
        Get-Content -LiteralPath $log | Write-Host
        throw "$Name failed (exit $LASTEXITCODE). See $log."
    }
    Get-Content -LiteralPath $log | Where-Object { $_ -match '(?i)\bwarning\b|\berror\b' } | Write-Host
}

function Write-CrtManifest([string]$Root, $Toolchain, [bool]$Checked, [bool]$WinlexChecked) {
    $inputs = @()
    foreach ($file in Get-CrtSourceFiles $Root) {
        $inputs += [ordered]@{ path = [IO.Path]::GetRelativePath($Root, $file.FullName); sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant() }
    }
    $headers = @()
    foreach ($include in @((Join-Path $Toolchain.sdk_include 'ucrt'), (Join-Path $Toolchain.sdk_include 'shared'), $Toolchain.msvc_include)) {
        foreach ($file in Get-ChildItem -LiteralPath $include -File -Recurse) {
            $headers += [ordered]@{ path = $file.FullName; sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant() }
        }
    }
    $artifacts = @()
    foreach ($arch in @('x86', 'x64', 'arm64', '')) {
        foreach ($name in @('UCRT.winmd', 'UCRT.roundtrip.winmd')) {
            $relative = if ($arch) { Join-Path $arch $name } else { $name }
            $path = Join-Path (Join-Path $Root 'out') $relative
            $artifacts += [ordered]@{ path = [IO.Path]::GetRelativePath($Root, $path); sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() }
        }
    }
    $generation = @('x86', 'x64', 'arm64') | ForEach-Object { Get-Content "out/$_/generation.json" -Raw | ConvertFrom-Json }
    [ordered]@{
        schema_version = 1
        toolchain = $Toolchain
        generation = @($generation)
        checks = @{ contracts = $(if ($Checked -or $WinlexChecked) { 'passed' } else { 'not-run' }); ast_audit_and_clippy = $(if ($Checked) { 'passed' } else { 'not-run' }); winlex = $(if ($WinlexChecked) { 'passed' } else { 'not-run' }) }
        source_inputs = $inputs
        installed_headers = $headers
        artifacts = $artifacts
    } | ConvertTo-Json -Depth 10 | Set-Content out/manifest.json -Encoding utf8NoBOM
}

function Get-CrtSourceFiles([string]$Root) {
    $paths = @('Cargo.toml', 'Cargo.lock', '.gitignore', 'LICENSE', 'README.md', 'THIRD-PARTY-NOTICES.md', 'RELEASE.md', 'main.h', 'strings.rdl', 'declarations.rdl', 'build.ps1', 'package-source.ps1', 'audit.py', 'verify.py', 'verify_multiarch.py', 'src', 'scripts', 'tests', 'vendor')
    foreach ($path in $paths) {
        Get-ChildItem -LiteralPath (Join-Path $Root $path) -File -Recurse -Force | Sort-Object FullName
    }
}
