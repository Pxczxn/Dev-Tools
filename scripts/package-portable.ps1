param([string]$OutputDirectory)

$ErrorActionPreference = 'Stop'
$workspaceRoot = Split-Path $PSScriptRoot -Parent
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $workspaceRoot 'dist' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$package = Get-Content -LiteralPath (Join-Path $workspaceRoot 'package.json') -Raw | ConvertFrom-Json
$name = "Dev-Tools-$($package.version)-windows-x64-portable"
$zip = Join-Path $OutputDirectory "$name.zip"
if (Test-Path -LiteralPath $zip) { throw "Output already exists: $zip. Choose another -OutputDirectory." }

Push-Location $workspaceRoot
$previousRustFlags = $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS
try {
    # Static MSVC CRT: no VC++ redistributable installation for the portable EXE.
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = "$previousRustFlags -C target-feature=+crt-static".Trim()
    $iconSource = Get-Item -LiteralPath (Join-Path $workspaceRoot 'public/app-icon.svg')
    $icon = Get-Item -LiteralPath (Join-Path $workspaceRoot 'src-tauri/icons/icon.ico') -ErrorAction SilentlyContinue
    if (-not $icon -or $icon.LastWriteTimeUtc -lt $iconSource.LastWriteTimeUtc) {
        & npm.cmd run icons
        if ($LASTEXITCODE -ne 0) { throw 'Icon generation failed.' }
    }
    # The frontend is already built in frontend/. Build the locked Rust release
    # directly; portable enables Tauri's production asset protocol.
    $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $workspaceRoot 'src-tauri/target' }
    & cargo build --manifest-path src-tauri/Cargo.toml --release --features portable --target x86_64-pc-windows-msvc --locked --offline
    if ($LASTEXITCODE -ne 0) { throw 'Portable release build failed.' }

    $executable = Join-Path $targetDir 'x86_64-pc-windows-msvc/release/dev-tools.exe'
    if (-not (Test-Path -LiteralPath $executable)) { throw "Release executable not found: $executable" }
    $stage = Join-Path $OutputDirectory ('.portable-' + [guid]::NewGuid().ToString('N'))
    $folder = Join-Path $stage $name
    $data = Join-Path $folder 'data'
    New-Item -ItemType Directory -Path $data -Force | Out-Null
    Copy-Item -LiteralPath $executable -Destination (Join-Path $folder 'Dev-Tools.exe')
    # The Evergreen bootstrapper is about 1.8MB. A fixed WebView2 runtime is about 180MB
    # and uses the same renderer process, so it does not reduce memory or improve speed.
    $cache = Join-Path $OutputDirectory '.cache'
    New-Item -ItemType Directory -Path $cache -Force | Out-Null
    $bootstrapper = Join-Path $cache 'MicrosoftEdgeWebview2Setup.exe'
    if (-not (Test-Path -LiteralPath $bootstrapper) -or (Get-Item -LiteralPath $bootstrapper).Length -lt 500KB -or (Get-Item -LiteralPath $bootstrapper).Length -gt 20MB) {
        Invoke-WebRequest -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile $bootstrapper
    }
    $bundledBootstrapper = Get-Item -LiteralPath $bootstrapper
    if ($bundledBootstrapper.Length -lt 500KB -or $bundledBootstrapper.Length -gt 20MB) {
        throw "WebView2 bootstrapper size is unexpected: $($bundledBootstrapper.Length) bytes"
    }
    Copy-Item -LiteralPath $bootstrapper -Destination (Join-Path $folder 'MicrosoftEdgeWebview2Setup.exe')
    $launcher = @"
@echo off
setlocal
set "FOUND="
reg query "HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" /v pv >nul 2>&1 && set "FOUND=1"
if not defined FOUND reg query "HKLM\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" /v pv >nul 2>&1 && set "FOUND=1"
if not defined FOUND reg query "HKCU\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" /v pv >nul 2>&1 && set "FOUND=1"
if not defined FOUND (
  echo WebView2 is missing. The bundled bootstrapper will install the shared runtime. An internet connection is required.
  "%~dp0MicrosoftEdgeWebview2Setup.exe" /silent /install
  if errorlevel 1 exit /b 1
)
start "" "%~dp0Dev-Tools.exe"
"@
    $launcher = ($launcher -replace "`r`n", "`n" -replace "`n", "`r`n")
    [IO.File]::WriteAllText((Join-Path $folder 'Dev-Tools.cmd'), $launcher, [Text.UTF8Encoding]::new($false))
    # Seed only a new empty store; never copy the developer's runtime configuration.
    [IO.File]::WriteAllText((Join-Path $data 'config.json'), '{"version":2,"settings":{},"workspaces":[],"services":[],"history":[],"favorites":[]}', [Text.UTF8Encoding]::new($false))
    $instructions = @"
Dev-Tools $($package.version) - Windows x64 Portable

Extract this entire folder to a writable location and run Dev-Tools.cmd.
Dev-Tools.cmd starts the app directly when WebView2 is already installed.
If it is missing, the bundled bootstrapper installs the shared Microsoft runtime. That step needs an internet connection.
The archive does not include a private WebView2 engine. Running memory stays the same as the system runtime.

Application settings: data/config.json
WebView storage and cache: data/webview/
Close the application before moving or backing up the entire folder.
Do not run directly inside the ZIP file.
Developer workspaces, credentials and browsing data are not included.
"@
    [IO.File]::WriteAllText((Join-Path $folder 'README.txt'), $instructions, [Text.UTF8Encoding]::new($false))
    Compress-Archive -LiteralPath $folder -DestinationPath $zip -CompressionLevel Optimal
    $hash = & (Join-Path $PSScriptRoot 'write-sha256.ps1') -Path $zip
    Write-Output "Portable archive: $zip"
    Write-Output "SHA256: $hash"
} finally {
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = $previousRustFlags
    Pop-Location
}
