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
& $cargo build --release --locked --manifest-path (Join-Path $PSScriptRoot 'Cargo.toml')
if($LASTEXITCODE){throw 'Pyrealium build failed.'}
if($Test){
    & $cargo test --locked --manifest-path (Join-Path $PSScriptRoot 'Cargo.toml')
    if($LASTEXITCODE){throw 'Pyrealium tests failed.'}
}
$gameBinary=Join-Path $PSScriptRoot 'target/release/pyrealium.exe'
Write-Output "Pyrealium game binary: $gameBinary"
