[CmdletBinding()]
param(
    [string]$InnoCompiler = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if (-not (Test-Path -LiteralPath $InnoCompiler -PathType Leaf)) {
    throw "Inno Setup 6 compiler not found at '$InnoCompiler'. Install Inno Setup 6 or pass -InnoCompiler."
}
Push-Location $root
try {
    $metadataText = & cargo metadata --offline --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $metadata = $metadataText | ConvertFrom-Json
    $package = @($metadata.packages | Where-Object { $_.name -eq 'raster' })
    if ($package.Count -ne 1 -or $package[0].version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') {
        throw 'Could not determine a safe Raster package version'
    }
    $version = $package[0].version
    & cargo build --release --offline --locked --bin raster --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Cargo build failed' }
    & $InnoCompiler "/DRasterVersion=$version" (Join-Path $PSScriptRoot 'raster.iss')
    if ($LASTEXITCODE -ne 0) { throw 'Inno Setup compilation failed' }
    Write-Host "Installer: $(Join-Path $root "target/installer/Raster-Setup-$version-x64.exe")"
}
finally {
    Pop-Location
}
