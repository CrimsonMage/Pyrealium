[CmdletBinding()]
param([switch]$Test,[string]$Python='')
$ErrorActionPreference='Stop'
$root=$PSScriptRoot
if(!$Python){$Python=Join-Path $env:USERPROFILE '.cache/codex-runtimes/codex-primary-runtime/dependencies/python/python.exe';if(!(Test-Path -LiteralPath $Python)){$Python='python'}}
$localCargo=Join-Path $root 'Tools/Rust/cargo/bin/cargo.exe'
$cargo='cargo'
if (Test-Path -LiteralPath $localCargo) {
    $cargo=$localCargo
    $env:CARGO_HOME=Join-Path $root 'Tools/Rust/cargo'
    $env:RUSTUP_HOME=Join-Path $root 'Tools/Rust/rustup'
    $env:PATH=(Split-Path $cargo)+';'+$env:PATH
} elseif (!(Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'Rust toolchain missing. Install Rust with rustup and reopen your terminal so cargo is on PATH.'
}
if (!(Test-Path -LiteralPath (Join-Path $root 'References/AC2_Rusted/crates/ac2-net/Cargo.toml'))) {
    throw 'The pinned AC2_Rusted reference is required. Run git submodule update --init --recursive from the project root.'
}
$referenceHead=& git -C (Join-Path $root 'References/AC2_Rusted') rev-parse HEAD
if($LASTEXITCODE -ne 0 -or $referenceHead -ne 'bcf67b4b9ac3ee9eb429d68ba6766cfce47c5a25'){throw 'AC2_Rusted reference does not match the supported protocol revision.'}
& $cargo build --release --locked --manifest-path (Join-Path $root 'Network/Bridge/Cargo.toml')
if($LASTEXITCODE -ne 0){throw 'Native bridge build failed.'}
& (Join-Path $root 'Build-Pyrealium.ps1') -Python $Python -Test:$Test
if($Test){
    & $cargo test --locked --manifest-path (Join-Path $root 'Network/Bridge/Cargo.toml')
    if($LASTEXITCODE -ne 0){throw 'Bridge tests failed.'}
}
Write-Output 'Pyrealium server and native network bridge built.'
