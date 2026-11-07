#requires -Version 7.2

function Initialize-WinlexToolchain {
    param([string]$Architecture, $Policy)
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $vs = & $vswhere -latest -products * -requires $Policy.vs_component -property installationPath
    if ($LASTEXITCODE -ne 0 -or -not $vs) { throw "Visual Studio tools not found for $Architecture." }
    $devcmd = Join-Path $vs 'Common7/Tools/VsDevCmd.bat'
    $sdkArgument = ''
    if ($env:WINLEX_SDK_VERSION) {
        if ($env:WINLEX_SDK_VERSION -notmatch '^\d+\.\d+\.\d+\.\d+$') { throw 'Invalid WINLEX_SDK_VERSION.' }
        $header = Join-Path ${env:ProgramFiles(x86)} "Windows Kits/10/Include/$env:WINLEX_SDK_VERSION/um/windows.h"
        if (-not (Test-Path -LiteralPath $header -PathType Leaf)) { throw "Requested Windows SDK $env:WINLEX_SDK_VERSION is not installed." }
        $sdkArgument = " -winsdk=$env:WINLEX_SDK_VERSION"
    }
    $PSNativeCommandArgumentPassing = 'Legacy'
    $output = @(cmd.exe /d /s /c "call `"$devcmd`" -no_logo -host_arch=x64 -arch=$Architecture$sdkArgument && set")
    if ($LASTEXITCODE -ne 0) {
        $output | ForEach-Object { [Console]::Error.WriteLine($_) }
        throw 'VsDevCmd failed.'
    }
    $output | ForEach-Object {
        if ($_ -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($matches[1], $matches[2], 'Process') }
    }
    if ($env:WINLEX_SDK_VERSION -and $env:WindowsSDKVersion.TrimEnd('\') -ne $env:WINLEX_SDK_VERSION) {
        throw "Requested Windows SDK $env:WINLEX_SDK_VERSION was not selected."
    }
    return @{
        LinkExe = Join-Path $env:VCToolsInstallDir "bin/Hostx64/$Architecture/link.exe"
        RcExe = Join-Path $env:WindowsSdkVerBinPath 'x64/rc.exe'
    }
}
