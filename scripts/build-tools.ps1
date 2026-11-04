#requires -Version 7.2

function Initialize-WinlexToolchain {
    param([string]$Architecture, $Policy)
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $vs = & $vswhere -latest -products * -requires $Policy.vs_component -property installationPath
    if ($LASTEXITCODE -ne 0 -or -not $vs) { throw "Visual Studio tools not found for $Architecture." }
    $devcmd = Join-Path $vs 'Common7/Tools/VsDevCmd.bat'
    $PSNativeCommandArgumentPassing = 'Legacy'
    cmd.exe /d /s /c "call `"$devcmd`" -no_logo -host_arch=x64 -arch=$Architecture && set" | ForEach-Object {
        if ($_ -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($matches[1], $matches[2], 'Process') }
    }
    if ($LASTEXITCODE -ne 0) { throw 'VsDevCmd failed.' }
    return @{
        LinkExe = Join-Path $env:VCToolsInstallDir "bin/Hostx64/$Architecture/link.exe"
        RcExe = Join-Path $env:WindowsSdkVerBinPath 'x64/rc.exe'
    }
}
