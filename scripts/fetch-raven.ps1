<#
.SYNOPSIS
  Downloads Raven v0.2.0 (static MSVC x64), verifies its SHA256 and extracts it.

.DESCRIPTION
  Extracts lib/* into third_party/raven-bin/lib/ and LICENSE/NOTICE into
  third_party/raven-bin/. With -Vendor, also copies
  share/raven/bindings/rust/* into vendor/raven-gui/ (the Rust binding, verbatim).

.PARAMETER Vendor
  Also (re)vendor the Rust binding into vendor/raven-gui/.
#>
param(
    [switch]$Vendor
)

$ErrorActionPreference = 'Stop'

$Url = 'https://github.com/oscar-soirey/Raven/releases/download/v0.2.0/raven-windows-msvc-x64-static.zip'
$ExpectedSha256 = '774cb1fe9382750a0b7a310c23e8e0a82357ea6b9d0914cec6c6775b62d2011f'

$Root = Split-Path -Parent $PSScriptRoot
$Temp = Join-Path ([System.IO.Path]::GetTempPath()) ("raven-fetch-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $Temp | Out-Null

try {
    $Zip = Join-Path $Temp 'raven.zip'
    Write-Host "Downloading $Url"
    Invoke-WebRequest -Uri $Url -OutFile $Zip -UseBasicParsing

    $Actual = (Get-FileHash -Algorithm SHA256 -Path $Zip).Hash.ToLowerInvariant()
    if ($Actual -ne $ExpectedSha256) {
        throw "Raven zip SHA256 mismatch: expected $ExpectedSha256, got $Actual"
    }
    Write-Host "SHA256 OK ($Actual)"

    $Extract = Join-Path $Temp 'x'
    Expand-Archive -Path $Zip -DestinationPath $Extract

    # The release zip wraps everything in a single top-level folder
    # (raven-windows-msvc-x64-static/); fall back to the extract root if it doesn't.
    $Top = Get-ChildItem -Path $Extract -Directory
    if (-not (Test-Path (Join-Path $Extract 'lib')) -and $Top.Count -eq 1) {
        $Extract = $Top[0].FullName
    }

    $LibSrc = Join-Path $Extract 'lib'
    $LibDst = Join-Path $Root 'third_party/raven-bin/lib'
    New-Item -ItemType Directory -Path $LibDst -Force | Out-Null
    Copy-Item -Path (Join-Path $LibSrc '*') -Destination $LibDst -Recurse -Force
    Write-Host "Extracted lib/* -> $LibDst"

    # Raven is Apache-2.0: redistributed builds ship its LICENSE and NOTICE (scripts/build.py).
    $BinDst = Join-Path $Root 'third_party/raven-bin'
    foreach ($Name in 'LICENSE', 'NOTICE') {
        Copy-Item -Path (Join-Path $Extract $Name) -Destination $BinDst -Force
    }
    Write-Host "Copied LICENSE and NOTICE -> $BinDst"

    if ($Vendor) {
        $BindSrc = Join-Path $Extract 'share/raven/bindings/rust'
        $BindDst = Join-Path $Root 'vendor/raven-gui'
        New-Item -ItemType Directory -Path $BindDst -Force | Out-Null
        Copy-Item -Path (Join-Path $BindSrc '*') -Destination $BindDst -Recurse -Force
        Write-Host "Vendored share/raven/bindings/rust/* -> $BindDst"
    }
}
finally {
    Remove-Item -Path $Temp -Recurse -Force -ErrorAction SilentlyContinue
}
