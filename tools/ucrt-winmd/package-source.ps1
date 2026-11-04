#requires -Version 7.2
[CmdletBinding()]
param([string]$Destination = (Join-Path $PSScriptRoot 'out/ucrt-winmd-source.zip'))

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/scripts/toolchain.ps1"
$Destination = [IO.Path]::GetFullPath($Destination)
New-Item (Split-Path $Destination) -ItemType Directory -Force | Out-Null
$stream = [IO.File]::Open($Destination, [IO.FileMode]::CreateNew)
$archive = [IO.Compression.ZipArchive]::new($stream, [IO.Compression.ZipArchiveMode]::Create)
try {
    foreach ($file in Get-CrtSourceFiles $PSScriptRoot) {
        $relative = [IO.Path]::GetRelativePath($PSScriptRoot, $file.FullName).Replace('\', '/')
        [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $file.FullName, $relative) | Out-Null
    }
}
finally { $archive.Dispose(); $stream.Dispose() }
Write-Host "Source archive: $Destination"
Write-Host 'Review RELEASE.md and licensing before public distribution.'
