[CmdletBinding()]
param([Parameter(Mandatory = $true)][string]$Installer)
$ErrorActionPreference = 'Stop'
$installerPath = (Resolve-Path -LiteralPath $Installer).Path
$root = Join-Path ([IO.Path]::GetTempPath()) ("raster-installer-test-" + [guid]::NewGuid().ToString('N'))
$installDir = Join-Path $root 'installed app'
New-Item -ItemType Directory -Path $root | Out-Null
try {
    # Run twice to exercise an upgrade over an existing installation.
    for ($i = 0; $i -lt 2; $i++) {
        $process = Start-Process -FilePath $installerPath -Wait -PassThru -ArgumentList @(
            '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', ("/DIR=`"$installDir`"")
        )
        if ($process.ExitCode -ne 0) { throw "Installer failed with exit code $($process.ExitCode)" }
    }
    foreach ($name in @('raster.exe', 'raster-logo.png', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'README.md', 'docs\LIMITATIONS.md', 'unins000.exe')) {
        if (-not (Test-Path -LiteralPath (Join-Path $installDir $name) -PathType Leaf)) { throw "Installed file missing: $name" }
    }
    $group = Join-Path ([Environment]::GetFolderPath('Programs')) 'Raster'
    $shellShortcut = Join-Path $group 'Raster.lnk'
    $editorShortcut = Join-Path $group 'Raster Editor.lnk'
    foreach ($shortcut in @($shellShortcut, $editorShortcut)) {
        if (-not (Test-Path -LiteralPath $shortcut -PathType Leaf)) { throw "Missing Start menu shortcut: $shortcut" }
    }
    $wsh = New-Object -ComObject WScript.Shell
    $promptLink = $wsh.CreateShortcut($shellShortcut)
    $editorLink = $wsh.CreateShortcut($editorShortcut)
    if ($promptLink.Arguments -ne '--shell' -or $editorLink.Arguments -ne '') { throw 'Incorrect Start menu shortcut arguments' }
    if ($promptLink.TargetPath -ne (Join-Path $installDir 'raster.exe') -or $editorLink.TargetPath -ne (Join-Path $installDir 'raster.exe')) { throw 'Incorrect Start menu shortcut target' }
    # Both the installer and installed app must contain the Raster icon.
    Add-Type -AssemblyName System.Drawing
    $setupIcon = [System.Drawing.Icon]::ExtractAssociatedIcon($installerPath)
    $appIcon = [System.Drawing.Icon]::ExtractAssociatedIcon((Join-Path $installDir 'raster.exe'))
    if ($null -eq $setupIcon -or $null -eq $appIcon) { throw 'Installer or app icon missing' }
    $setupBitmap = $setupIcon.ToBitmap()
    $appBitmap = $appIcon.ToBitmap()
    try {
        if ($setupBitmap.Width -ne $appBitmap.Width -or $setupBitmap.Height -ne $appBitmap.Height) { throw 'Icon dimensions differ' }
        for ($y = 0; $y -lt $setupBitmap.Height; $y++) {
            for ($x = 0; $x -lt $setupBitmap.Width; $x++) {
                if ($setupBitmap.GetPixel($x, $y).ToArgb() -ne $appBitmap.GetPixel($x, $y).ToArgb()) { throw 'Installer and app icons differ' }
            }
        }
    } finally {
        $setupBitmap.Dispose(); $appBitmap.Dispose(); $setupIcon.Dispose(); $appIcon.Dispose()
    }
    $versionOutput = & (Join-Path $installDir 'raster.exe') --version
    if ($LASTEXITCODE -ne 0 -or ($versionOutput -join "`n") -notmatch '^raster \d+\.\d+\.\d+') { throw 'Installed executable version check failed' }
    $health = & (Join-Path $installDir 'raster.exe') --no-config --health | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $health.status -ne 'ok' -or -not $health.config_valid) { throw 'Installed executable health check failed' }
    $sentinel = Join-Path $installDir 'user-data.txt'
    Set-Content -LiteralPath $sentinel -Value 'keep me'
    $process = Start-Process -FilePath (Join-Path $installDir 'unins000.exe') -Wait -PassThru -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART')
    if ($process.ExitCode -ne 0) { throw "Uninstaller failed with exit code $($process.ExitCode)" }
    if (Test-Path -LiteralPath (Join-Path $installDir 'raster.exe')) { throw 'Uninstaller left raster.exe behind' }
    if ((Test-Path -LiteralPath $shellShortcut) -or (Test-Path -LiteralPath $editorShortcut)) { throw 'Uninstaller left Start menu shortcuts behind' }
    if (-not (Test-Path -LiteralPath $sentinel)) { throw 'Uninstaller removed an untracked user file' }
    Write-Host 'Installer, upgrade, executable and uninstall checks passed.'
}
finally {
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
