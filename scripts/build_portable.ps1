# Build the portable ZIP asset.
#
# It exists as a script rather than as a line of workflow YAML for the same
# reason `build_installer.ps1` does: the portable archive and the installer must
# carry the SAME payload, and that payload is defined once, in
# `installer/autoshop.iss`'s [Files] section. A zip assembled inline in a
# workflow would drift from the installer the first time a runtime file moved,
# and the drift would ship silently — the archive would still extract, the exe
# would still start, and a sidecar would fail on someone else's machine.
#
# Excludes match the installer's exactly: downloaded weights (multi-gigabyte,
# fetched on first use), Python bytecode and the sidecars' own tests.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$')]
    [string]$Version
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptDirectory
$distDirectory = Join-Path $repoRoot 'dist'
$stagingDirectory = Join-Path $repoRoot 'target\portable'
$archivePath = Join-Path $distDirectory ("autoshop-$Version-windows-x64.zip")

$cargoTomlPath = Join-Path $repoRoot 'Cargo.toml'
$cargoVersionLine = Select-String -LiteralPath $cargoTomlPath -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1
if ($null -eq $cargoVersionLine) {
    throw "Could not read the package version from $cargoTomlPath."
}
$cargoVersion = $cargoVersionLine.Matches[0].Groups[1].Value
if ($cargoVersion -ne $Version) {
    throw ("Version mismatch: -Version is '$Version' but Cargo.toml declares " +
        "'$cargoVersion'. The archive name is part of the published asset table, " +
        'so this is refused rather than renamed.')
}

foreach ($required in @(
        (Join-Path $distDirectory 'autoshop.exe'),
        (Join-Path $distDirectory 'autoshop-gui.exe'),
        (Join-Path $repoRoot 'LICENSE'),
        (Join-Path $repoRoot 'README.md'))) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "Required file not found: $required"
    }
}

if (Test-Path -LiteralPath $stagingDirectory) {
    Remove-Item -LiteralPath $stagingDirectory -Recurse -Force
}
$null = New-Item -ItemType Directory -Path $stagingDirectory -Force

Copy-Item -LiteralPath (Join-Path $distDirectory 'autoshop.exe') -Destination $stagingDirectory
Copy-Item -LiteralPath (Join-Path $distDirectory 'autoshop-gui.exe') -Destination $stagingDirectory
Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE') -Destination $stagingDirectory
Copy-Item -LiteralPath (Join-Path $repoRoot 'README.md') -Destination $stagingDirectory
Copy-Item -LiteralPath (Join-Path $repoRoot 'assets') -Destination (Join-Path $stagingDirectory 'assets') -Recurse

$pythonSource = Join-Path $repoRoot 'python'
$pythonTarget = Join-Path $stagingDirectory 'python'
$null = New-Item -ItemType Directory -Path $pythonTarget -Force
$sourcePrefix = (Resolve-Path -LiteralPath $pythonSource).Path.TrimEnd('\') + '\'
Get-ChildItem -LiteralPath $pythonSource -Recurse -File | ForEach-Object {
    $relative = $_.FullName.Substring($sourcePrefix.Length)
    if ($relative -like 'weights\*' -or $relative -like '*__pycache__\*' -or
        $relative -like 'test_*.py' -or $relative -like '*\test_*.py' -or
        $relative -like '*.pyc') {
        return
    }
    $destination = Join-Path $pythonTarget $relative
    $destinationDirectory = Split-Path -Parent $destination
    if (-not (Test-Path -LiteralPath $destinationDirectory)) {
        $null = New-Item -ItemType Directory -Path $destinationDirectory -Force
    }
    Copy-Item -LiteralPath $_.FullName -Destination $destination
}

if (Test-Path -LiteralPath $archivePath) {
    Remove-Item -LiteralPath $archivePath -Force
}
Compress-Archive -Path (Join-Path $stagingDirectory '*') -DestinationPath $archivePath -CompressionLevel Optimal

$archive = Get-Item -LiteralPath $archivePath
$hash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLower()
Write-Output ("portable archive: {0}" -f $archive.FullName)
Write-Output ("bytes           : {0}" -f $archive.Length)
Write-Output ("sha256          : {0}" -f $hash)
$sidecarCount = (Get-ChildItem -LiteralPath $pythonTarget -Recurse -File | Measure-Object).Count
Write-Output ("python sidecars : {0} files (weights excluded)" -f $sidecarCount)
