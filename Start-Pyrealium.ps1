# Start Pyrealium from pyrealium.toml (beside this script). The first run writes a commented
# pyrealium.toml to edit (DAT directory and PostgreSQL database).
[CmdletBinding()]
param(
    [string]$Config = (Join-Path $PSScriptRoot 'pyrealium.toml'),
    [switch]$Rebuild,
    [switch]$SkipBuild,
    [switch]$Stop
)
$ErrorActionPreference = 'Stop'
if ($Rebuild -and $SkipBuild) { throw '-Rebuild and -SkipBuild cannot be combined.' }
$binary = Join-Path $PSScriptRoot 'target/release/pyrealium.exe'
if ($Stop) {
    & $binary stop --config $Config --seconds 20
    exit $LASTEXITCODE
}
if ($Rebuild -or (!$SkipBuild -and !(Test-Path -LiteralPath $binary))) {
    & (Join-Path $PSScriptRoot 'Build-Pyrealium.ps1')
}
if (!(Test-Path -LiteralPath $binary)) { throw "No server binary at $binary; run Build-Pyrealium.ps1." }
if (!(Test-Path -LiteralPath $Config)) {
    & $binary init --config $Config
    Write-Output "Edit $Config (dat_dir and the [database] url), then run start-pyrealium.bat again."
    exit 1
}
Write-Output "Pyrealium: $binary"
& $binary run --config $Config
exit $LASTEXITCODE
