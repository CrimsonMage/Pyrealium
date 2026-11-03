[CmdletBinding()]
param(
    [string]$Bind='127.0.0.1',
    [ValidateRange(1,65534)][int]$Port=9000,
    [string]$Account='local',
    [string]$DatDirectory='',
    [string]$DataDirectory='',
    [string]$AuthDirectory='',
    [string]$PublicOrigin='',
    [ValidateRange(1024,65535)][int]$AccountPort=19400,
    [string]$GameHost='127.0.0.1',
    [switch]$ExternalAccounts,
    [switch]$DevelopmentLogin,
    [switch]$DisableMapTravel,
    [string]$Content='',
    [string]$ContentOverlay='',
    [string]$Python='',
    [switch]$SkipBuild,
    [switch]$Rebuild,
    [switch]$Stop
)
$ErrorActionPreference='Stop'
$root=$PSScriptRoot
if(!$Python){$Python=Join-Path $env:USERPROFILE '.cache/codex-runtimes/codex-primary-runtime/dependencies/python/python.exe';if(!(Test-Path -LiteralPath $Python)){$Python='python'}}
if(!$DatDirectory){$DatDirectory=Join-Path $root "Asheron's Call 2 - Legions"}
if(!$DataDirectory){$DataDirectory=Join-Path $PSScriptRoot 'data/world'}
$DataDirectory=$ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($DataDirectory)
. (Join-Path $PSScriptRoot 'Server-Binaries.ps1')
$server=Get-PyrealiumGameBinary
$managedBinaries=Get-PyrealiumManagedBinaries
$accountServer=Join-Path $PSScriptRoot 'target/release/pyrealium.exe'
if($Stop){& $server stop --data $DataDirectory --seconds 20; if($LASTEXITCODE){throw 'Server stop failed.'};return}
. (Join-Path $PSScriptRoot 'Recover-StaleLock.ps1')
. (Join-Path $PSScriptRoot 'Find-AccountService.ps1')
Restore-PyrealiumStaleLock $DataDirectory 'server.lock'
if($SkipBuild -and $Rebuild){throw 'Choose either -SkipBuild or -Rebuild.'}
# The store's native exclusive lock remains authoritative. Only report a live
# owner after checking its executable and data directory, since PIDs are reused.
# A lock remaining after exclusive stale recovery must stay intact.
$lockFile=Join-Path $DataDirectory 'server.lock'
if(Test-Path -LiteralPath $lockFile -PathType Leaf){
    $lockOwner=[uint32]0
    $lockText=([string](Get-Content -LiteralPath $lockFile -Raw)).Trim()
    if(![uint32]::TryParse($lockText,[ref]$lockOwner) -or $lockOwner -eq 0){
        throw "The store is locked, but its owner could not be verified: $lockFile. Wait for any starting server to finish, then retry. The lock was left intact."
    }
    $owner=Get-CimInstance Win32_Process -Filter "ProcessId=$lockOwner"
    $ownerData=$null
    if($owner -and $owner.ExecutablePath -in $managedBinaries -and $owner.CommandLine -match '^(?:"[^"]+"|\S+)\s+serve(?:-training)?\s'){
        $dataMatch=[regex]::Match($owner.CommandLine,'(?:^|\s)--data\s+(?:"(?<quoted>[^"]+)"|(?<plain>\S+))')
        if($dataMatch.Success){
            $dataArgument=if($dataMatch.Groups['quoted'].Success){$dataMatch.Groups['quoted'].Value}else{$dataMatch.Groups['plain'].Value}
            if([IO.Path]::IsPathRooted($dataArgument)){$ownerData=[IO.Path]::GetFullPath($dataArgument)}
        }
    }
    if(!$ownerData -or $ownerData.TrimEnd('\','/') -ne $DataDirectory.TrimEnd('\','/')){
        throw "A server lock exists, but its recorded owner (PID $lockOwner) cannot be confirmed for this store: $lockFile. Verify that no server is using this store before removing a stale lock. The launcher has not removed it."
    }
    if($Rebuild){throw "Pyrealium is already running (PID $lockOwner). Stop it with Start-Pyrealium.ps1 -Stop -DataDirectory `"$DataDirectory`" before rebuilding."}
    $bindMatch=[regex]::Match($owner.CommandLine,'(?:^|\s)--bind\s+(?:"(?<quoted>[^"]+)"|(?<plain>\S+))')
    $endpoint=if($bindMatch.Groups['quoted'].Success){$bindMatch.Groups['quoted'].Value}else{$bindMatch.Groups['plain'].Value}
    Write-Output "Pyrealium is already running (PID $lockOwner) at $endpoint. Data: $DataDirectory"
    Write-Output 'No second server was started. Existing settings remain in effect; stop the server before changing them.'
    Write-Output "To stop: powershell.exe -File `"$PSScriptRoot\Start-Pyrealium.ps1`" -Stop -DataDirectory `"$DataDirectory`""
    return
}
if($Rebuild -or (!(Test-Path -LiteralPath $server -PathType Leaf) -and !$SkipBuild)){
    & (Join-Path $PSScriptRoot 'Build-Pyrealium.ps1') -Python $Python
    $server=Get-PyrealiumGameBinary
}
if(!(Test-Path -LiteralPath $server -PathType Leaf)){
    throw 'Pyrealium server binary is missing. Run Build-Pyrealium.ps1, or start without -SkipBuild to build it.'
}
if(!$Content){
    $Content=Join-Path $DataDirectory 'legions-world.tsv'
    & $Python (Join-Path $root 'Network/prepare_content.py') $DatDirectory $Content
    if($LASTEXITCODE){throw 'DAT content preparation failed.'}
}
$ip=[Net.IPAddress]::Parse($Bind)
if($DevelopmentLogin -and ![Net.IPAddress]::IsLoopback($ip)){throw '-DevelopmentLogin is only supported on loopback.'}
if(![Net.IPAddress]::IsLoopback($ip) -and $PublicOrigin -notmatch '^https://[a-zA-Z0-9.:-]+$'){throw 'Public hosting requires -PublicOrigin https://accounts.example.com, serving the account backend and its source archive.'}
if(!$DevelopmentLogin){
    if(!$AuthDirectory){$AuthDirectory=Join-Path (Split-Path $DataDirectory) 'accounts'}
    $AuthDirectory=$ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($AuthDirectory)
    if(!$PublicOrigin){$PublicOrigin="http://127.0.0.1:$AccountPort"}
}
$env:PYREALIUM_PUBLIC_ORIGIN=$PublicOrigin
$env:PYREALIUM_UNREAL_LEGIONS_DATA='1'
$serverArgs=@('serve','--bind',"${Bind}:$Port",'--account',$Account,'--data',$DataDirectory,'--game-data',$DatDirectory,
    '--content',$Content,'--initialize-ui','true','--training-kit','false','--lifestones','true','--portals','true')
# Local testing convenience only: never enable map relocation on a public bind.
# World-builder overlay; its NAME.rules.tsv creature rules load automatically.
if($ContentOverlay){$serverArgs+=@('--content-overlay',$ContentOverlay)}
if([Net.IPAddress]::IsLoopback($ip) -and !$DisableMapTravel){$serverArgs+=@('--test-map-travel','true')}
if(!$DevelopmentLogin){$serverArgs+=@('--auth-data',$AuthDirectory)}
$accounts=$null
try {
    if(!$DevelopmentLogin -and !$ExternalAccounts){
        Restore-PyrealiumStaleLock $AuthDirectory 'accounts.lock'
        $existingAccounts=Get-PyrealiumAccountService $managedBinaries $AuthDirectory $DataDirectory $PublicOrigin $AccountPort $GameHost $Port
        if($existingAccounts){
            Write-Output "Reusing matching account service (PID $existingAccounts) at $PublicOrigin. It will remain running when this game server stops."
        } else {
            # Bind-check before spawning, so an unrelated service is never mistaken for ours.
            $probe=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,$AccountPort)
            try {$probe.Start()} finally {$probe.Stop()}
            New-Item -ItemType Directory -Path $AuthDirectory -Force | Out-Null
            $accountArgs=@('account-web','--auth-data',$AuthDirectory,'--data',$DataDirectory,
                '--web-bind',"127.0.0.1:$AccountPort",'--public-origin',$PublicOrigin,
                '--public-host',$GameHost,'--public-port',"$Port")
            # Arguments are Windows argv values, never shell commands. Reject quotes/newlines
            # rather than allowing paths or endpoints to change argument boundaries.
            if($accountArgs | Where-Object {$_ -match '["\r\n]'} ) {throw 'Invalid account-service argument.'}
            $quotedArgs=($accountArgs | ForEach-Object {'"'+$_.TrimEnd('\')+'"'}) -join ' '
            if(!(Test-Path -LiteralPath $accountServer -PathType Leaf)){throw 'Account service binary is missing. Run Build-Pyrealium.ps1.'}
            $accounts=Start-Process -FilePath $accountServer -ArgumentList $quotedArgs -PassThru -WindowStyle Hidden `
                -RedirectStandardOutput (Join-Path $AuthDirectory 'service.log') -RedirectStandardError (Join-Path $AuthDirectory 'service-error.log')
            $ready=$false
            for($attempt=0;$attempt -lt 50;$attempt++){
                $accounts.Refresh()
                if($accounts.HasExited){throw "Account service failed. See $AuthDirectory\service-error.log"}
                try {
                    $health=Invoke-RestMethod -Uri "http://127.0.0.1:$AccountPort/health" -TimeoutSec 1
                    if($health.status -eq 'ok'){$ready=$true;break}
                } catch {}
                Start-Sleep -Milliseconds 100
            }
            if(!$ready){throw 'Account service did not become ready.'}
        }
    }
    if(!$DevelopmentLogin){Write-Output "Accounts: $PublicOrigin (first sign-in creates an account; passwords require 12-128 characters)."}
    Write-Output "Pyrealium: UDP $Port and $($Port+1); persistent store $DataDirectory"
    Write-Output "Game executable: $server"
    & $server @serverArgs
    if($LASTEXITCODE){throw "Pyrealium exited with code $LASTEXITCODE"}
} finally {
    # Only stop the child owned by this launch; external account services keep running.
    if($accounts){
        $accounts.Refresh()
        if(!$accounts.HasExited){
            & $server account-stop --auth-data $AuthDirectory --seconds 20
            if(!$accounts.WaitForExit(5000)){Write-Warning 'Account service is still stopping; check its log.'}
        }
        $accounts.Dispose()
    }
}
