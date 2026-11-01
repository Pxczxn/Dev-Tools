param(
    [string]$PortableZip,
    [string]$SetupExe
)

$ErrorActionPreference = 'Stop'

function Assert-StaysRunning([string]$Executable, [string]$WorkingDirectory) {
    if (-not (Test-Path -LiteralPath $Executable)) { throw "Cannot start missing program: $Executable" }
    $process = Start-Process -FilePath $Executable -WorkingDirectory $WorkingDirectory -PassThru
    try {
        $deadline = (Get-Date).AddSeconds(12)
        do {
            Start-Sleep -Seconds 1
            $process.Refresh()
            if ($process.HasExited) { throw "$Executable exited early with code $($process.ExitCode)." }
        } while ((Get-Date) -lt $deadline)
        Write-Output "Started $Executable"
    } finally {
        if (-not $process.HasExited) {
            & taskkill.exe /PID $process.Id /T /F | Out-Null
        }
    }
}

if ($PortableZip) {
    $expand = Join-Path ([IO.Path]::GetTempPath()) ('dev-tools-smoke-' + [guid]::NewGuid().ToString('N'))
    Expand-Archive -LiteralPath $PortableZip -DestinationPath $expand
    $portable = Get-ChildItem -LiteralPath $expand -Recurse -Filter 'Dev-Tools.exe' | Select-Object -First 1
    if (-not $portable) { throw "Portable archive has no Dev-Tools.exe: $PortableZip" }
    Assert-StaysRunning $portable.FullName $portable.DirectoryName
    Remove-Item -LiteralPath $expand -Recurse -Force
}

if ($SetupExe) {
    $installRoot = Join-Path $env:LOCALAPPDATA 'Dev-Tools'
    Start-Process -FilePath $SetupExe -ArgumentList '/S' -Wait
    $installed = Get-ChildItem -LiteralPath $installRoot -Recurse -Filter 'Dev-Tools.exe' -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $installed) { throw "NSIS install did not produce Dev-Tools.exe under $installRoot" }
    try {
        Assert-StaysRunning $installed.FullName $installed.DirectoryName
    } finally {
        $uninstaller = Join-Path $installRoot 'uninstall.exe'
        if (Test-Path -LiteralPath $uninstaller) {
            Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait
        }
    }
}
