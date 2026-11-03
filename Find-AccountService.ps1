# Reuse only the account process identified by this store's marker, with the
# exact requested configuration. A healthy HTTP endpoint alone is insufficient.
function Get-PyrealiumAccountService(
    [string[]]$Server, [string]$AuthDirectory, [string]$DataDirectory,
    [string]$PublicOrigin, [int]$AccountPort, [string]$GameHost, [int]$GamePort
) {
    $lockFile=Join-Path $AuthDirectory 'accounts.lock'
    if(!(Test-Path -LiteralPath $lockFile -PathType Leaf)){return $null}
    $ownerId=[uint32]0
    $marker=([string](Get-Content -LiteralPath $lockFile -Raw)).Trim()
    if(![uint32]::TryParse($marker,[ref]$ownerId) -or $ownerId -eq 0){
        throw "The account lock owner cannot be verified: $lockFile. The lock was left intact."
    }
    $owner=Get-CimInstance Win32_Process -Filter "ProcessId=$ownerId"
    if(!$owner -or !$owner.ExecutablePath -or
       [IO.Path]::GetFullPath($owner.ExecutablePath) -notin @($Server | ForEach-Object {[IO.Path]::GetFullPath($_)}) -or !$owner.CommandLine){
        throw "The account lock owner (PID $ownerId) cannot be confirmed as this Pyrealium service. The lock was left intact: $lockFile"
    }
    # Windows argv parsing handles quoted switches and directories containing
    # spaces, as well as invocations from Start-Accounts.ps1 or the managed pair.
    if(-not ('PyrealiumLauncher.NativeCommandLine' -as [type])){
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace PyrealiumLauncher {
    public static class NativeCommandLine {
        [DllImport("shell32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
        public static extern IntPtr CommandLineToArgvW(string commandLine, out int argc);
        [DllImport("kernel32.dll")]
        public static extern IntPtr LocalFree(IntPtr memory);
    }
}
'@
    }
    $count=0
    $pointer=[PyrealiumLauncher.NativeCommandLine]::CommandLineToArgvW($owner.CommandLine,[ref]$count)
    if($pointer -eq [IntPtr]::Zero){throw 'Cannot inspect account-service arguments. The lock was left intact.'}
    $arguments=@()
    try {
        for($index=0;$index -lt $count;$index++){
            $value=[Runtime.InteropServices.Marshal]::ReadIntPtr($pointer,$index*[IntPtr]::Size)
            $arguments+=[Runtime.InteropServices.Marshal]::PtrToStringUni($value)
        }
    } finally {[void][PyrealiumLauncher.NativeCommandLine]::LocalFree($pointer)}
    if($arguments.Count -lt 2 -or $arguments[1] -cne 'account-web'){
        throw "The account lock owner (PID $ownerId) is not an account-web process. The lock was left intact."
    }
    $expected=@{
        '--auth-data'=$AuthDirectory; '--data'=$DataDirectory
        '--web-bind'="127.0.0.1:$AccountPort"; '--public-origin'=$PublicOrigin
        '--public-host'=$GameHost; '--public-port'=[string]$GamePort
    }
    foreach($option in $expected.Keys){
        $values=@(for($index=2;$index -lt $arguments.Count-1;$index++){
            if($arguments[$index] -ceq $option){$arguments[$index+1]}
        })
        $matching=$values.Count -eq 1
        if($matching){
            $actual=$values[0]
            $wanted=$expected[$option]
            if($option -in @('--auth-data','--data')){
                $matching=[IO.Path]::IsPathRooted($actual)
                if($matching){
                    $actual=[IO.Path]::GetFullPath($actual).TrimEnd('\','/')
                    $wanted=[IO.Path]::GetFullPath($wanted).TrimEnd('\','/')
                }
            }
            $matching=$matching -and [string]::Equals($actual,$wanted,[StringComparison]::OrdinalIgnoreCase)
        }
        if(!$matching){
            throw "Account service PID $ownerId has different or unverifiable $option settings. Stop it with Start-Accounts.ps1 -Stop -AuthDirectory `"$AuthDirectory`", or use matching settings in both launchers. The service and lock were left intact."
        }
    }
    $ready=$false
    for($attempt=0;$attempt -lt 20;$attempt++){
        try {
            $listener=Get-NetTCPConnection -State Listen -LocalPort $AccountPort -ErrorAction Stop |
                Where-Object {$_.LocalAddress -eq '127.0.0.1' -and $_.OwningProcess -eq $ownerId}
            if($listener){
                $health=Invoke-RestMethod -Uri "http://127.0.0.1:$AccountPort/health" -TimeoutSec 1
                if($health.status -eq 'ok'){$ready=$true;break}
            }
        } catch {}
        Start-Sleep -Milliseconds 100
    }
    if(!$ready){throw "Account service PID $ownerId is not ready on 127.0.0.1:$AccountPort. Check $AuthDirectory\service-error.log. The service and lock were left intact."}
    return $ownerId
}
