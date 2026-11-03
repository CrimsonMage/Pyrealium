# Game builds must not overwrite the executable held by a standalone account
# service. Keep the legacy path for accounts and for existing installations.
function Get-PyrealiumGameBinary {
    $game=Join-Path $PSScriptRoot 'target/game/release/pyrealium.exe'
    if(Test-Path -LiteralPath $game -PathType Leaf){return $game}
    return (Join-Path $PSScriptRoot 'target/release/pyrealium.exe')
}
function Get-PyrealiumManagedBinaries {
    return @((Join-Path $PSScriptRoot 'target/game/release/pyrealium.exe'),
             (Join-Path $PSScriptRoot 'target/release/pyrealium.exe'))
}
