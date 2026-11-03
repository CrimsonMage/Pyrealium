[CmdletBinding()]
param([switch]$Test,[string]$Python='')
$ErrorActionPreference='Stop'
$root=$PSScriptRoot
$localCargo=Join-Path $root 'Tools/Rust/cargo/bin/cargo.exe'
$cargo='cargo'
if(Test-Path -LiteralPath $localCargo){
    $cargo=$localCargo
    $env:CARGO_HOME=Join-Path $root 'Tools/Rust/cargo'
    $env:RUSTUP_HOME=Join-Path $root 'Tools/Rust/rustup'
    $env:PATH=(Split-Path $cargo)+';'+$env:PATH
} elseif(!(Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'Rust toolchain missing. Install Rust with rustup and reopen your terminal so cargo is on PATH. An already-built server can be started with start-pyrealium.bat without Rust.'
}
if(!$Python){$Python=Join-Path $env:USERPROFILE '.cache/codex-runtimes/codex-primary-runtime/dependencies/python/python.exe';if(!(Test-Path -LiteralPath $Python)){$Python='python'}}
& $Python (Join-Path $PSScriptRoot 'export_source.py')
if($LASTEXITCODE){throw 'Source archive generation failed.'}
& $cargo build --release --locked --manifest-path (Join-Path $PSScriptRoot 'emulator/Cargo.toml') --target-dir (Join-Path $PSScriptRoot 'target/game')
if($LASTEXITCODE){throw 'Pyrealium build failed.'}
if($Test){
    & $cargo test --locked --manifest-path (Join-Path $PSScriptRoot 'emulator/Cargo.toml') --target-dir (Join-Path $PSScriptRoot 'target/game')
    if($LASTEXITCODE){throw 'Pyrealium tests failed.'}
}
$accountBinary=Join-Path $PSScriptRoot 'target/release/pyrealium.exe'
$gameBinary=Join-Path $PSScriptRoot 'target/game/release/pyrealium.exe'
# Never stop a running service to publish a game build. Its account executable
# can be refreshed on a subsequent build after the service has been stopped.
$accountOwner=Get-CimInstance Win32_Process -Filter "Name='pyrealium.exe'" |
    Where-Object {$_.ExecutablePath -eq $accountBinary}
if($accountOwner){
    Write-Output 'Game server updated. The running account service keeps its existing executable; rebuild after stopping accounts to update that copy.'
} else {
    New-Item -ItemType Directory -Path (Split-Path $accountBinary) -Force | Out-Null
    try {Copy-Item -LiteralPath $gameBinary -Destination $accountBinary -Force}
    catch {Write-Warning "Game server updated, but the account executable could not be refreshed: $($_.Exception.Message)"}
}
Write-Output "Pyrealium game binary: $gameBinary"
