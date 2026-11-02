param(
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$PortableFileName,
    [Parameter(Mandatory = $true)][string]$SetupFileName,
    [Parameter(Mandatory = $true)][string]$PortableSignature,
    [Parameter(Mandatory = $true)][string]$SetupSignature,
    [string]$Notes = '',
    [Parameter(Mandatory = $true)][string]$Output
)

$ErrorActionPreference = 'Stop'
$base = "https://github.com/Pxczxn/Dev-Tools/releases/download/v$Version"
$manifest = [ordered]@{
    version = $Version
    notes = $Notes
    pub_date = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
    platforms = [ordered]@{
        'windows-x86_64-portable' = [ordered]@{
            signature = $PortableSignature.Trim()
            url = "$base/$PortableFileName"
        }
        'windows-x86_64' = [ordered]@{
            signature = $SetupSignature.Trim()
            url = "$base/$SetupFileName"
        }
    }
}
$json = $manifest | ConvertTo-Json -Depth 6
$parent = Split-Path -Parent $Output
if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
[IO.File]::WriteAllText($Output, $json.Trim() + "`n", [Text.UTF8Encoding]::new($false))
