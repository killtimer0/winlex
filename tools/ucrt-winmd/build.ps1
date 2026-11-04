#requires -Version 7.2
[CmdletBinding()]
param(
    [string]$SdkIncludePath,
    [string]$MsvcIncludePath,
    [string]$ClangPath = 'clang',
    [string]$LibClangPath,
    [switch]$Check,
    [string]$PythonPath = 'python',
    [string]$WinlexPath
)

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/scripts/toolchain.ps1"
Push-Location $PSScriptRoot
$saved = @{}
foreach ($name in @('PATH', 'LIBCLANG_PATH', 'CARGO_TARGET_DIR')) {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
try {
    New-Item out -ItemType Directory -Force | Out-Null
    Remove-Item out/manifest.json -ErrorAction SilentlyContinue
    $toolchain = Get-CrtToolchain -SdkIncludePath $SdkIncludePath -MsvcIncludePath $MsvcIncludePath -ClangPath $ClangPath -LibClangPath $LibClangPath
    $toolchain | ConvertTo-Json -Depth 8 | Set-Content out/toolchain.json -Encoding utf8NoBOM
    $env:LIBCLANG_PATH = $toolchain.libclang_path
    $env:PATH = "$(Split-Path $toolchain.clang_path);$env:PATH"
    $env:CARGO_TARGET_DIR = Join-Path $PSScriptRoot 'target'
    $python = $null
    if ($Check -or $WinlexPath) {
        $python = Resolve-CrtExecutable $PythonPath
        Invoke-CrtStep 'python-version' $python @('-c', 'import sys; sys.exit("Python 3.10+ without optimization required") if sys.version_info < (3, 10) or not __debug__ else print(sys.version)')
        $toolchain['python_version'] = (Get-Content out/python-version.log -Raw).Trim()
        $toolchain['python_path'] = $python
    }
    if ($WinlexPath) { $WinlexPath = Resolve-CrtExecutable $WinlexPath }
    foreach ($arch in @('x86', 'x64', 'arm64', 'merge')) {
        Invoke-CrtStep "build-$arch" 'cargo' @('run', '--locked', '--manifest-path', 'Cargo.toml', '--', $arch)
    }
    if ($Check -or $WinlexPath) {
        Invoke-CrtStep 'tests' 'cargo' @('test', '--locked')
    }
    if ($Check) {
        Invoke-CrtStep 'toolchain-tests' 'pwsh' @('-NoProfile', '-File', 'tests/toolchain.tests.ps1')
        Invoke-CrtStep 'audit' $python @('audit.py')
        Invoke-CrtStep 'clippy' 'cargo' @('clippy', '--locked', '--all-targets', '--', '-D', 'warnings')
    }
    if ($WinlexPath) {
        Invoke-CrtStep 'winlex-query' $python @('verify.py', '--winlex', $WinlexPath)
        Invoke-CrtStep 'winlex-multiarch' $python @('verify_multiarch.py', '--winlex', $WinlexPath)
    }
    Write-CrtManifest -Root $PSScriptRoot -Toolchain $toolchain -Checked ([bool]$Check) -WinlexChecked ([bool]$WinlexPath)
    Write-Host 'Generated out/UCRT.winmd and out/manifest.json'
    if (-not $Check) { Write-Host 'SKIPPED: contract tests, AST audit and Clippy (use -Check)' }
    if (-not $WinlexPath) { Write-Host 'SKIPPED: optional Winlex integration tests' }
}
finally {
    foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
    Pop-Location
}
