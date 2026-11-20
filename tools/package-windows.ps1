# Package the already built Windows x64 Release. No network access or installation.
[CmdletBinding()]
param(
    [string] $RcloneExe,
    [string] $RatholeExe
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$resources = Join-Path $PSScriptRoot 'packaging'
if (-not $RcloneExe)  { $RcloneExe  = Join-Path $root 'target/local-tools/rclone-1.72.1/rclone.exe' }
if (-not $RatholeExe) { $RatholeExe = Join-Path $root 'target/local-tools/rathole-0.5.0/rathole.exe' }
$app = (Get-Item -LiteralPath (Join-Path $root 'target/release/rust-remote.exe')).FullName
$rclone = (Get-Item -LiteralPath $RcloneExe).FullName
$rathole = (Get-Item -LiteralPath $RatholeExe).FullName

# All three executables must match the platform stated on the archive.
foreach ($exe in @($app, $rclone, $rathole)) {
    $reader = [IO.BinaryReader]::new([IO.File]::OpenRead($exe))
    try {
        $reader.BaseStream.Position = 0x3c
        $reader.BaseStream.Position = $reader.ReadUInt32()
        if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) {
            throw "Expected a Windows x64 executable: $exe"
        }
    } finally { $reader.Dispose() }
}
$rcloneVersion = @(& $rclone version)
if ($LASTEXITCODE -ne 0 -or $rcloneVersion[0] -ne 'rclone v1.72.1') { throw 'Expected rclone v1.72.1.' }
$ratholeVersion = @(& $rathole --version)
if ($LASTEXITCODE -ne 0 -or -not ($ratholeVersion -match '^Build Version:\s+0\.5\.0\s*$')) { throw 'Expected rathole 0.5.0.' }

$entries = [ordered]@{
    'START.cmd'                  = Join-Path $resources 'START.cmd'
    'README.txt'                 = Join-Path $resources 'WINDOWS.txt'
    'rust-remote.exe'            = $app
    'rclone.exe'                 = $rclone
    'rathole.exe'                = $rathole
    'LICENSE'                    = Join-Path $root 'LICENSE'
    'licenses/rclone-1.72.1.txt' = Join-Path $resources 'licenses/rclone-1.72.1.txt'
    'licenses/rathole-0.5.0.txt' = Join-Path $resources 'licenses/rathole-0.5.0.txt'
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
$dist = [IO.Directory]::CreateDirectory((Join-Path $root 'target/dist')).FullName
$archive = Join-Path $dist 'rust-remote-windows-x64.zip'
$temp = Join-Path $dist ([IO.Path]::GetRandomFileName())
try {
    $zip = [IO.Compression.ZipFile]::Open($temp, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($entry in $entries.GetEnumerator()) {
            [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                $zip, $entry.Value, $entry.Key, [IO.Compression.CompressionLevel]::Optimal)
        }
    } finally { $zip.Dispose() }
    Move-Item -LiteralPath $temp -Destination $archive -Force
    Get-Item -LiteralPath $archive
} finally {
    if ([IO.File]::Exists($temp)) { [IO.File]::Delete($temp) }
}
