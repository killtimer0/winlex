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
$verifyArguments = @("$PSScriptRoot/build-record.py", '--records', $CliBuildRecord, $GuiBuildRecord,
    '--architecture', $Architecture, '--configuration', $Configuration)
$records = & python @verifyArguments --json
if ($LASTEXITCODE -ne 0) { throw 'Build record or PE validation failed.' }
$records = $records | ConvertFrom-Json
$binaries = @($records | ForEach-Object { $_.executable })
$target = $records[0].policy.target
$output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDir)
New-Item -ItemType Directory -Force -Path $output | Out-Null
$zip = Join-Path $output "winlex-$Architecture.zip"
$licenseZip = Join-Path $output "winlex-$Architecture-licenses.zip"
foreach ($path in @($zip, "$zip.sha256", $licenseZip)) {
    if (Test-Path -LiteralPath $path) { throw "Output already exists: $path" }
}
$tmp = Join-Path $root 'tmp'
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
$stage = Join-Path $tmp ("winlex-package-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    $programStage = Join-Path $stage 'program'
    $licenseStage = Join-Path $stage 'licenses'
    New-Item -ItemType Directory -Path $programStage, $licenseStage | Out-Null
    foreach ($binary in $binaries) { Copy-Item -LiteralPath $binary -Destination $programStage }
    foreach ($path in @('LICENSE', 'THIRD-PARTY-NOTICES.md', 'docs/LICENSES.txt')) {
        Copy-Item -LiteralPath (Join-Path $root $path) -Destination $programStage
    }
    foreach ($path in @('LICENSE', 'THIRD-PARTY-NOTICES.md', 'licenses/YY-Thunks-LICENSE',
            'licenses/win32-darkmodelib-MPL-2.0.md', 'licenses/win32-darkmodelib-UAHMenuBar.md',
            'licenses/win32-darkmode-MIT.md',
            'licenses/VC-LTL-LICENSE', 'licenses/VC-LTL-sdk-license.rtf',
            'licenses/VC-LTL-sdk-third-party-notices.rtf',
            'vendor/PATCHES.md', 'vendor/windows-metadata/license-mit',
            'vendor/windows-metadata/license-apache-2.0', 'crates/winlex-gui/res/codicons/LICENSE')) {
        $destination = Join-Path $licenseStage $path
        New-Item -ItemType Directory -Force -Path (Split-Path $destination -Parent) | Out-Null
        Copy-Item -LiteralPath (Join-Path $root $path) -Destination $destination
    }
    $metadata = & cargo metadata --manifest-path "$root/Cargo.toml" --locked --format-version 1 --filter-platform $target
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
    $metadata = $metadata | ConvertFrom-Json
    foreach ($package in $metadata.packages | Where-Object { $_.source }) {
        $source = Split-Path $package.manifest_path -Parent
        $destination = Join-Path $licenseStage "licenses/crates/$($package.name)-$($package.version)"
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
        $destination = Join-Path $licenseStage 'licenses/rust'
        New-Item -ItemType Directory -Force -Path $destination | Out-Null
        Copy-Item -LiteralPath $source -Destination $destination -Recurse
    }
    $records | ConvertTo-Json -Depth 10 | & python "$PSScriptRoot/delivery.py" licenses `
        --stage $licenseStage --output $licenseZip --architecture $Architecture
    if ($LASTEXITCODE -ne 0) { throw 'License archive validation failed.' }
    $manifest = Get-ChildItem -LiteralPath $programStage -File -Recurse | Sort-Object FullName | ForEach-Object {
        $relative = $_.FullName.Substring($programStage.Length + 1).Replace('\', '/')
        "$( (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() )  $relative"
    }
    [IO.File]::WriteAllLines((Join-Path $programStage 'SHA256SUMS.txt'), [string[]]$manifest, [Text.UTF8Encoding]::new($false))
    $records | ConvertTo-Json -Depth 10 | & python "$PSScriptRoot/build-record.py" --archive $zip --stage $programStage
    if ($LASTEXITCODE -ne 0) { throw 'Archive validation failed.' }
    $hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText("$zip.sha256", "$hash  $([IO.Path]::GetFileName($zip))`n", [Text.UTF8Encoding]::new($false))
    Write-Host "Packaged $zip"
    Write-Host "Collected $licenseZip (merge all architectures with scripts/delivery.py)"
} catch {
    foreach ($path in @($zip, "$zip.sha256", $licenseZip)) {
        if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
    }
    throw
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force
}
