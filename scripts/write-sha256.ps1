param(
    [Parameter(Mandatory = $true)]
    [string]$Path
)

$ErrorActionPreference = 'Stop'
$item = Get-Item -LiteralPath $Path
$sha256 = [Security.Cryptography.SHA256]::Create()
$stream = [IO.File]::OpenRead($item.FullName)
try {
    $hash = [BitConverter]::ToString($sha256.ComputeHash($stream)).Replace('-', '').ToLowerInvariant()
} finally {
    $stream.Dispose()
    $sha256.Dispose()
}
$sidecar = "$($item.FullName).sha256"
# Same sidecar as the portable archive: lowercase hash, two spaces, file name, CRLF.
[IO.File]::WriteAllText($sidecar, "$hash  $($item.Name)`r`n", [Text.UTF8Encoding]::new($false))
Write-Output $hash
