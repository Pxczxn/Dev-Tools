param(
    [string]$Tag = $env:GITHUB_REF_NAME
)

$ErrorActionPreference = 'Stop'
if (-not $Tag) { throw 'Pass -Tag or set GITHUB_REF_NAME, for example v1.0.0.' }
if ($Tag -notmatch '^v(?<version>\d+\.\d+\.\d+)$') {
    throw "Tag '$Tag' must use the form v1.2.3 with no prerelease suffix."
}
$version = $Matches.version
$root = Split-Path $PSScriptRoot -Parent

function Read-Text([string]$Path) {
    return [IO.File]::ReadAllText((Join-Path $root $Path))
}

$packageText = Read-Text 'package.json'
$lockText = Read-Text 'package-lock.json'
$tauriText = Read-Text 'src-tauri/tauri.conf.json'
$cargo = Read-Text 'src-tauri/Cargo.toml'
$lockfile = Read-Text 'src-tauri/Cargo.lock'
if ($packageText -notmatch '"version": "([^"]+)"') { throw 'Cannot read package.json version.' }
$packageVersion = $Matches[1]
$lockVersions = [regex]::Matches($lockText, '"version": "([^"]+)"')
if ($lockVersions.Count -lt 2) { throw 'Cannot read package-lock.json versions.' }
if ($tauriText -notmatch '"version": "([^"]+)"') { throw 'Cannot read tauri.conf.json version.' }
$tauriVersion = $Matches[1]
if ($cargo -notmatch '(?sm)\[package\].*?^version = "([^"]+)"') { throw 'Cannot read src-tauri/Cargo.toml version.' }
$cargoVersion = $Matches[1]
if ($lockfile -notmatch '(?s)name = "xchendev-tools"\s+version = "([^"]+)"') { throw 'Cannot read xchendev-tools version from Cargo.lock.' }
$cargoLockVersion = $Matches[1]

$found = [ordered]@{
    'package.json' = $packageVersion
    'package-lock.json' = $lockVersions[0].Groups[1].Value
    'package-lock packages' = $lockVersions[1].Groups[1].Value
    'src-tauri/Cargo.toml' = $cargoVersion
    'src-tauri/Cargo.lock' = $cargoLockVersion
    'src-tauri/tauri.conf.json' = $tauriVersion
}
$mismatch = $found.GetEnumerator() | Where-Object { $_.Value -ne $version }
if ($mismatch) {
    $details = ($mismatch | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ', '
    throw "Tag v$version does not match: $details"
}
Write-Output "Version $version matches the tag $Tag."
