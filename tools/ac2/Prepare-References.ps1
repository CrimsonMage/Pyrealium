# Extract analysis input locally. These files are evidence, never build inputs.
[CmdletBinding()]
param([string]$ProjectRoot = (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent))
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$destination = [IO.Path]::GetFullPath((Join-Path $ProjectRoot 'Saved/AC2Research'))
foreach ($name in @('decompiled-admin7148.zip', 'decompiled-legions7160.zip', 'decompiled-client0930.zip')) {
    $archive = [IO.Compression.ZipFile]::OpenRead((Join-Path $ProjectRoot $name))
    try {
        foreach ($entry in $archive.Entries) {
            $target = [IO.Path]::GetFullPath((Join-Path $destination $entry.FullName))
            if (!$target.StartsWith($destination + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw "Archive path escapes destination: $($entry.FullName)" }
            if (!$entry.Name) { continue }
            [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($target)) | Out-Null
            if (!(Test-Path -LiteralPath $target)) { [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $target, $false) }
        }
        Write-Output "Extracted $name into $destination"
    } finally { $archive.Dispose() }
}
