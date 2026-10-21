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
    & cargo build --manifest-path src-tauri/Cargo.toml --release --features portable --target x86_64-pc-windows-msvc --locked --offline
    if ($LASTEXITCODE -ne 0) { throw 'Portable release build failed.' }

    $executable = Join-Path $workspaceRoot 'src-tauri/target/x86_64-pc-windows-msvc/release/dev-tools.exe'
    if (-not (Test-Path -LiteralPath $executable)) { throw "Release executable not found: $executable" }
    $stage = Join-Path $OutputDirectory ('.portable-' + [guid]::NewGuid().ToString('N'))
    $folder = Join-Path $stage $name
    $data = Join-Path $folder 'data'
    New-Item -ItemType Directory -Path $data -Force | Out-Null
    Copy-Item -LiteralPath $executable -Destination (Join-Path $folder 'Dev-Tools.exe')
    # Seed only a new empty store; never copy the developer's runtime configuration.
    [IO.File]::WriteAllText((Join-Path $data 'config.json'), '{"version":2,"settings":{},"workspaces":[],"services":[],"history":[],"favorites":[]}', [Text.UTF8Encoding]::new($false))
    $instructions = @"
Dev-Tools $($package.version) - Windows x64 Portable

Extract this entire folder to a writable location and run Dev-Tools.exe.
No application installer or administrator access is required.
Requires Windows 10/11 x64 and Microsoft Edge WebView2 Runtime.
This lightweight archive uses the system WebView2 Runtime; it is not bundled.

Application settings: data/config.json
WebView storage and cache: data/webview/
Close the application before moving or backing up the entire folder.
Do not run directly inside the ZIP file.
Developer workspaces, credentials and browsing data are not included.
"@
    [IO.File]::WriteAllText((Join-Path $folder 'README.txt'), $instructions, [Text.UTF8Encoding]::new($false))
    Compress-Archive -LiteralPath $folder -DestinationPath $zip -CompressionLevel Optimal
    $sha256 = [Security.Cryptography.SHA256]::Create()
    $stream = [IO.File]::OpenRead($zip)
    try {
        $hash = [BitConverter]::ToString($sha256.ComputeHash($stream)).Replace('-', '').ToLowerInvariant()
    } finally {
        $stream.Dispose()
        $sha256.Dispose()
    }
    [IO.File]::WriteAllText("$zip.sha256", "$hash  $name.zip`r`n", [Text.UTF8Encoding]::new($false))
    Write-Output "Portable archive: $zip"
    Write-Output "SHA256: $hash"
} finally {
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = $previousRustFlags
    Pop-Location
}
