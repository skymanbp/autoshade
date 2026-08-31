[CmdletBinding()]
param(
    [Parameter(Mandatory = $false)]
    [ValidatePattern('^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$')]
    [string]$ExpectedVersion
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

function Find-Iscc {
    $candidates = New-Object System.Collections.Generic.List[string]
    $registryKeys = @(
        'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1',
        'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1',
        'Registry::HKEY_CURRENT_USER\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1',
        'Registry::HKEY_CURRENT_USER\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1'
    )

    foreach ($key in $registryKeys) {
        if (Test-Path -LiteralPath $key) {
            $properties = Get-ItemProperty -LiteralPath $key
            $installLocation = $properties.PSObject.Properties['InstallLocation']
            if (($null -ne $installLocation) -and
                (-not [string]::IsNullOrWhiteSpace([string]$installLocation.Value))) {
                $candidates.Add((Join-Path ([string]$installLocation.Value) 'ISCC.exe'))
            }
        }
    }

    $programFilesX86 = [Environment]::GetEnvironmentVariable('ProgramFiles(x86)')
    if ([string]::IsNullOrWhiteSpace($programFilesX86)) {
        $programFilesX86 = [Environment]::GetFolderPath(
            [Environment+SpecialFolder]::ProgramFilesX86
        )
    }
    if (-not [string]::IsNullOrWhiteSpace($programFilesX86)) {
        $candidates.Add((Join-Path $programFilesX86 'Inno Setup 6\ISCC.exe'))
    }

    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }

    throw ('Inno Setup 6 compiler (ISCC.exe) was not found. Install Inno Setup 6 ' +
        'or repair its uninstall-registry entry, then rerun this script.')
}

$scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptDirectory
$cargoTomlPath = Join-Path $repoRoot 'Cargo.toml'
$installerScriptPath = Join-Path $repoRoot 'installer\autoshade.iss'
$cliPath = Join-Path $repoRoot 'dist\autoshade.exe'
$guiPath = Join-Path $repoRoot 'dist\autoshade-gui.exe'
$outputDirectory = Join-Path $repoRoot 'target\installer'

foreach ($requiredPath in @($cargoTomlPath, $installerScriptPath, $cliPath, $guiPath)) {
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
        throw "Required installer input is missing: $requiredPath"
    }
}

$cargoText = [IO.File]::ReadAllText($cargoTomlPath)
$packageMatch = [regex]::Match(
    $cargoText,
    '(?ms)^\[package\]\s*(?<body>.*?)(?=^\[|\z)'
)
if (-not $packageMatch.Success) {
    throw "Could not find the [package] section in $cargoTomlPath"
}
$versionMatch = [regex]::Match(
    $packageMatch.Groups['body'].Value,
    '(?m)^version\s*=\s*"(?<version>[^"]+)"\s*$'
)
if (-not $versionMatch.Success) {
    throw "Could not read package.version from $cargoTomlPath"
}
$cargoVersion = $versionMatch.Groups['version'].Value

$buildVersion = $cargoVersion
if (-not [string]::IsNullOrWhiteSpace($ExpectedVersion)) {
    $buildVersion = $ExpectedVersion
}

[IO.Directory]::CreateDirectory($outputDirectory) | Out-Null
$outputPath = Join-Path $outputDirectory "AutoShade-Setup-$buildVersion.exe"
$logPath = Join-Path $outputDirectory "build-$buildVersion.log"
$logLines = New-Object System.Collections.Generic.List[string]
$utf8NoBom = New-Object -TypeName System.Text.UTF8Encoding -ArgumentList $false

function Write-LogLine {
    param(
        [AllowEmptyString()]
        [string]$Message = '',
        [switch]$Warning
    )

    if ($Warning) {
        Write-Warning $Message
        $logLines.Add("WARNING: $Message")
    }
    else {
        Write-Host $Message
        $logLines.Add($Message)
    }
}

function Save-BuildLog {
    [IO.File]::WriteAllLines($logPath, [string[]]$logLines, $utf8NoBom)
}

Write-LogLine "Cargo.toml package version: $cargoVersion"
if ($buildVersion -ne $cargoVersion) {
    Write-LogLine "Validation override active: building installer version $buildVersion instead of Cargo.toml version $cargoVersion." -Warning
}

$versionOutputLines = & $cliPath --version 2>&1
$versionExitCode = $LASTEXITCODE
$versionOutput = (($versionOutputLines | ForEach-Object { $_.ToString() }) -join "`n").Trim()
if ($versionExitCode -ne 0) {
    throw "Version probe failed with exit code $versionExitCode`: $versionOutput"
}
$requiredVersionOutput = "autoshade $buildVersion"
if ($versionOutput -ne $requiredVersionOutput) {
    throw ("dist\autoshade.exe version mismatch. Expected exactly '{0}', got '{1}'. " +
        'Rebuild/copy the matching release binaries, or use -ExpectedVersion only for an intentional pipeline validation.') -f
        $requiredVersionOutput, $versionOutput
}
Write-LogLine "Verified dist\autoshade.exe: $versionOutput"

$isccPath = Find-Iscc
Write-LogLine "ISCC: $isccPath"

# Remove only the exact versioned artifact so a failed compile cannot be
# mistaken for a fresh successful build.
if (Test-Path -LiteralPath $outputPath -PathType Leaf) {
    Remove-Item -LiteralPath $outputPath -Force
}

$compilerArguments = @(
    "/DAppVersion=$buildVersion",
    $installerScriptPath
)
$compilerOutput = & $isccPath $compilerArguments 2>&1
$compilerExitCode = $LASTEXITCODE
$compilerOutput | ForEach-Object { Write-LogLine $_.ToString() }
if ($compilerExitCode -ne 0) {
    Save-BuildLog
    throw "ISCC failed with exit code $compilerExitCode"
}

if (-not (Test-Path -LiteralPath $outputPath -PathType Leaf)) {
    Save-BuildLog
    throw "ISCC reported success, but the expected installer was not produced: $outputPath"
}

$outputFile = Get-Item -LiteralPath $outputPath
$outputHash = Get-FileHash -LiteralPath $outputPath -Algorithm SHA256
Write-LogLine
Write-LogLine "Installer: $($outputFile.FullName)"
Write-LogLine "Size: $($outputFile.Length) bytes"
Write-LogLine "SHA-256: $($outputHash.Hash.ToLowerInvariant())"
Write-LogLine "Build log: $logPath"
Save-BuildLog
