#requires -Version 7.2
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][ValidateSet('x86','x64','arm64')][string]$Architecture,
    [Parameter(Mandatory=$true)][string]$CliBuildRecord,
    [Parameter(Mandatory=$true)][string]$GuiBuildRecord,
    [Parameter(Mandatory=$true)][string]$OutputDir,
    [ValidateSet('Debug','Release','Dist')][string]$Configuration = 'Dist'
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$target = switch ($Architecture) {
    'x86' { 'i686-pc-windows-msvc' }
    'x64' { 'x86_64-pc-windows-msvc' }
    'arm64' { 'aarch64-pc-windows-msvc' }
}
$records = @($CliBuildRecord, $GuiBuildRecord)
$names = @('winlex.exe', 'winlex-gui.exe')
$binaries = @()
for ($i = 0; $i -lt $records.Count; $i++) {
    $record = Get-Content -LiteralPath $records[$i] -Raw | ConvertFrom-Json
    if ($record.target -ne $target -or $record.configuration -ne $Configuration) {
        throw "Build record target or configuration mismatch: $($records[$i])"
    }
    $exe = (Resolve-Path -LiteralPath $record.executable).Path
    if ([IO.Path]::GetFileName($exe) -ine $names[$i] -or
        (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash -ine $record.executable_sha256) {
        throw "Executable name or hash mismatch: $exe"
    }
    $binaries += $exe
}
$output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDir)
New-Item -ItemType Directory -Force -Path $output | Out-Null
$zip = Join-Path $output "winlex-$Architecture.zip"
if ((Test-Path -LiteralPath $zip) -or (Test-Path -LiteralPath "$zip.sha256")) {
    throw "Output already exists: $zip or $zip.sha256"
}
$tmp = Join-Path $root 'tmp'
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
$stage = Join-Path $tmp ("winlex-package-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    foreach ($binary in $binaries) { Copy-Item -LiteralPath $binary -Destination $stage }
    foreach ($path in @('LICENSE', 'THIRD-PARTY-NOTICES.md', 'licenses/YY-Thunks-LICENSE',
            'licenses/VC-LTL-LICENSE', 'licenses/VC-LTL-sdk-license.rtf',
            'licenses/VC-LTL-sdk-third-party-notices.rtf',
            'vendor/PATCHES.md', 'vendor/windows-metadata/license-mit',
            'vendor/windows-metadata/license-apache-2.0', 'crates/winlex-gui/res/codicons/LICENSE')) {
        $destination = Join-Path $stage $path
        New-Item -ItemType Directory -Force -Path (Split-Path $destination -Parent) | Out-Null
        Copy-Item -LiteralPath (Join-Path $root $path) -Destination $destination
    }
    $metadata = & cargo metadata --manifest-path "$root/Cargo.toml" --locked --format-version 1 --filter-platform $target
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
    $metadata = $metadata | ConvertFrom-Json
    foreach ($package in $metadata.packages | Where-Object { $_.source }) {
        $source = Split-Path $package.manifest_path -Parent
        $destination = Join-Path $stage "licenses/crates/$($package.name)-$($package.version)"
        New-Item -ItemType Directory -Force -Path $destination | Out-Null
        $files = @(Get-ChildItem -LiteralPath $source -File | Where-Object {
            $_.Name -match '^(license|copying|notice|copyright|unlicense)' -or $_.Name -eq 'README.md'
        })
        if ($package.license_file) {
            $license = Join-Path $source $package.license_file
            if (-not (Test-Path -LiteralPath $license -PathType Leaf)) { throw "Missing license: $license" }
            $files += Get-Item -LiteralPath $license
        }
        if (-not ($files | Where-Object { $_.Name -match '^(license|copying|notice|copyright|unlicense)' })) {
            Write-Warning "No standalone license text in $($package.name) $($package.version) source package."
        }
        $files | Select-Object -Unique | Copy-Item -Destination $destination
    }
    $sysroot = (& rustc --print sysroot)
    if ($LASTEXITCODE -ne 0) { throw 'Unable to locate Rust sysroot.' }
    $rustDocs = Join-Path $sysroot 'share/doc/rust'
    foreach ($path in @('COPYRIGHT-library.html', 'licenses')) {
        $source = Join-Path $rustDocs $path
        if (-not (Test-Path -LiteralPath $source)) { throw "Missing Rust license documentation: $source" }
        $destination = Join-Path $stage 'licenses/rust'
        New-Item -ItemType Directory -Force -Path $destination | Out-Null
        Copy-Item -LiteralPath $source -Destination $destination -Recurse
    }
    $manifest = Get-ChildItem -LiteralPath $stage -File -Recurse | Sort-Object FullName | ForEach-Object {
        $relative = $_.FullName.Substring($stage.Length + 1).Replace('\', '/')
        "$( (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() )  $relative"
    }
    [IO.File]::WriteAllLines((Join-Path $stage 'SHA256SUMS.txt'), [string[]]$manifest, [Text.UTF8Encoding]::new($false))
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
    $hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText("$zip.sha256", "$hash  $([IO.Path]::GetFileName($zip))`n", [Text.UTF8Encoding]::new($false))
    Write-Host "Packaged $zip"
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force
}
