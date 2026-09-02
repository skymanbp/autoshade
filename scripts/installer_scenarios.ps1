<#
.SYNOPSIS
    Runs the Windows installer through the six states a user can actually put it
    in, and asserts what each one must leave behind.

.DESCRIPTION
    The installer's two promises -- it upgrades in place, and the user can choose
    to uninstall -- are behaviour, not settings, so they are checked by
    installing. Each scenario runs setup or the uninstaller EXACTLY ONCE,
    silently, and waits for it; nothing here loops over an installer.

    The chain is:

      1. fresh install of the base version
      2. upgrade to the new version, in place, over a running program
      3. downgrade refusal
      4. uninstall keeping the user's data
      5. reinstall
      6. uninstall deleting the user's data

    Identity is a parameter because the same chain runs in two places. On a CI
    runner there is no AutoShade to collide with, so it runs under the shipped
    AppId. On a developer machine there is one, so it runs under a throwaway
    AppId compiled with /DAppIdOverride and /DAppNameOverride -- and -ForbiddenAppId
    then names the shipped identity, whose uninstall entry, install directory and
    Start Menu folder are photographed before and after and must not have moved.

    Every path comes from a parameter or an environment variable. Nothing in this
    file names a machine.

.PARAMETER BaseSetup
    Installer used for the fresh install (scenario 1). On CI this is the previous
    published release; locally it is a build of this tree.

.PARAMETER UpgradeSetup
    Installer used for the upgrade and the reinstall (scenarios 2 and 5). Always
    built from this tree.

.PARAMETER DowngradeSetup
    Installer carrying a version BELOW UpgradeVersion, built from this tree
    (scenario 3). A published release predates the refusal and would simply
    install, which is why this cannot just be BaseSetup on CI.

.PARAMETER BaseFromThisTree
    The base installer was compiled from this .iss, so its log carries this
    tree's sentences and its tasks include addtopath. Off for a published
    release: the file and registry assertions still run, the log ones do not.

.PARAMETER PathSuffix
    Appended to the user PATH for the duration of the run and taken off again
    at the end (and on any error), so the uninstaller's promise -- the PATH
    comes back byte for byte -- is measured on more than one shape. A GitHub
    runner's PATH ends in ';' and a developer's usually does not, and the
    first CI run found the uninstaller eating that trailing separator. Pass
    ';' or ';;'; empty measures the PATH as found.
#>
[CmdletBinding()]
param(
    [string]$BaseSetup,
    [string]$BaseVersion,
    [string]$UpgradeSetup,
    [string]$UpgradeVersion,
    [string]$DowngradeSetup,
    [string]$DowngradeVersion,
    [string]$AppId,
    [string]$AppName,
    [string]$PayloadRoot,
    [string]$WorkRoot,
    [string]$ForbiddenAppId,
    [switch]$BaseFromThisTree,
    [string]$PathSuffix = '',
    # Run the [Setup] assertions and stop. Nothing is installed, no
    # installer is needed, and the whole file is read in under a second.
    [switch]$SourceOnly
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$script:Failures = New-Object System.Collections.Generic.List[string]
$script:Checks = 0

function Write-Head {
    param([string]$Text)
    Write-Host ''
    Write-Host "=== $Text"
}

function Assert-That {
    param([bool]$Condition, [string]$Message, [string]$Detail = '')
    $script:Checks++
    if ($Condition) {
        Write-Host "PASS  $Message"
    }
    else {
        $line = "FAIL  $Message"
        if ($Detail -ne '') { $line = "$line -- $Detail" }
        Write-Host $line
        $script:Failures.Add($line)
    }
}

function Assert-Same {
    param($Actual, $Expected, [string]$Message)
    Assert-That ([string]$Actual -eq [string]$Expected) $Message "expected '$Expected', got '$Actual'"
}

# --------------------------------------------------------------------------
# The user's environment, read from the registry rather than the process: the
# process copy is a snapshot taken at launch and cannot show what setup wrote.
# --------------------------------------------------------------------------
function Get-UserPathRaw {
    $key = Get-Item -LiteralPath 'HKCU:\Environment'
    return [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
}

# The uninstaller's promise about the user PATH, as it can be kept. An
# installer before 1.2.4 recorded that it added the entry but not whether it
# wrote the separator, so an uninstall over such a base is exact up to one
# trailing ';' -- exact outright when the PATH ended in ';' to begin with,
# which the Windows default user PATH does. From a 1.2.4 base on, byte for
# byte. The comparison is keyed on the base VERSION, so this relaxation
# expires by itself once the previous published release is 1.2.4 or later.
function Assert-UserPathRestored {
    param([string]$Before, [string]$What)
    $now = Get-UserPathRaw
    if ([version]$BaseVersion -lt [version]'1.2.4') {
        Assert-Same $now.TrimEnd(';') $Before.TrimEnd(';') "$What, up to the trailing separator a pre-1.2.4 installer could not record"
    }
    else {
        Assert-Same $now $Before $What
    }
}

function Set-UserPathRaw {
    # Written back with the kind it was found in, so a REG_SZ PATH is not
    # silently promoted to REG_EXPAND_SZ by the test harness.
    param([string]$Value)
    $key = Get-Item -LiteralPath 'HKCU:\Environment'
    $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
    if ($null -ne $key.GetValue('Path')) { $kind = $key.GetValueKind('Path') }
    [Microsoft.Win32.Registry]::SetValue('HKEY_CURRENT_USER\Environment', 'Path', $Value, $kind)
}

function Get-UserPathEntries {
    $raw = Get-UserPathRaw
    if ([string]::IsNullOrWhiteSpace($raw)) { return @() }
    return @($raw.Split(';') | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne '' })
}

function Get-PathEntryCount {
    param([string]$Directory)
    $wanted = $Directory.TrimEnd('\').Replace('/', '\')
    $n = 0
    foreach ($entry in Get-UserPathEntries) {
        if ($entry.Trim('"').TrimEnd('\').Replace('/', '\') -ieq $wanted) { $n++ }
    }
    return $n
}

function Get-UninstallKeyPath {
    param([string]$Id)
    return "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$($Id)_is1"
}

function Get-UninstallEntry {
    param([string]$Id)
    $path = Get-UninstallKeyPath $Id
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    return Get-ItemProperty -LiteralPath $path
}

# Inno's default AppVerName is "<name> version <version>", so the entry a user
# sees in Programs and Features is named that and not the bare program name; a
# longer program name never matches a shorter one's prefix, which is what keeps
# a scenario run from counting the shipped install as one of its own.
function Get-DisplayNameEntryCount {
    param([string]$Name)
    $root = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
    if (-not (Test-Path -LiteralPath $root)) { return 0 }
    $n = 0
    foreach ($child in Get-ChildItem -LiteralPath $root) {
        $props = Get-ItemProperty -LiteralPath $child.PSPath
        $displayName = $props.PSObject.Properties['DisplayName']
        if ($null -eq $displayName) { continue }
        $value = [string]$displayName.Value
        if (($value -eq $Name) -or $value.StartsWith("$Name version ")) { $n++ }
    }
    return $n
}

function Get-StartMenuGroup {
    param([string]$Name)
    return Join-Path ([Environment]::GetFolderPath('Programs')) $Name
}

function Get-TreeSignature {
    param([string]$Directory)
    if (-not (Test-Path -LiteralPath $Directory)) { return '<absent>' }
    $parts = New-Object System.Collections.Generic.List[string]
    foreach ($f in Get-ChildItem -LiteralPath $Directory -Recurse -File -Force |
             Sort-Object -Property FullName) {
        $parts.Add("$($f.FullName.Substring($Directory.Length))|$($f.Length)")
    }
    return ($parts -join "`n")
}

# A file that is not there has to compare UNEQUAL, not throw: an assertion
# that crashes reports nothing, and the interesting failures here are exactly
# the ones where something the installer must not remove has been removed.
function Get-Sha256 {
    param([string]$FilePath)
    if (-not (Test-Path -LiteralPath $FilePath -PathType Leaf)) { return '<missing>' }
    return (Get-FileHash -LiteralPath $FilePath -Algorithm SHA256).Hash.ToLowerInvariant()
}

# --------------------------------------------------------------------------
# Running setup and the uninstaller. Both are started once per scenario and
# waited for. The uninstaller relaunches itself from the temp directory so it
# can delete its own exe, so the first process exiting does NOT mean the
# uninstall is done -- unins000.exe disappearing does, in both data modes.
# --------------------------------------------------------------------------
function Invoke-Silently {
    param([string]$Exe, [string]$Arguments, [string]$Label)
    Write-Host "run   $Label"
    Write-Host "      $Exe $Arguments"
    $process = Start-Process -FilePath $Exe -ArgumentList $Arguments -Wait -PassThru
    Write-Host "      exit code $($process.ExitCode)"
    return $process.ExitCode
}

function Wait-ForUninstaller {
    param([string]$InstallDir, [int]$TimeoutSeconds = 300)
    $marker = Join-Path $InstallDir 'unins000.exe'
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Test-Path -LiteralPath $marker) -and ((Get-Date) -lt $deadline)) {
        Start-Sleep -Milliseconds 500
    }
    if (Test-Path -LiteralPath $marker) {
        throw "the uninstaller did not finish within $TimeoutSeconds s: $marker is still there"
    }
    # The registry and PATH writes happen before the self-deletion, but the
    # second-phase process is still tearing down; give it a moment to exit so
    # the directory assertions below do not race it.
    Start-Sleep -Milliseconds 1500
}

function Read-Log {
    param([string]$LogPath)
    if (-not (Test-Path -LiteralPath $LogPath)) { return '' }
    return [IO.File]::ReadAllText($LogPath)
}

function Assert-LogContains {
    param([string]$LogPath, [string]$Needle, [string]$Message)
    $text = Read-Log $LogPath
    Assert-That ($text.Contains($Needle)) $Message "log $LogPath has no line containing '$Needle'"
}

function Assert-LogMatches {
    param([string]$LogPath, [string]$Pattern, [string]$Message)
    $text = Read-Log $LogPath
    Assert-That ($text -match $Pattern) $Message "log $LogPath matches nothing like '$Pattern'"
}

# --------------------------------------------------------------------------
# Payload: what the installed tree must look like after any install from this
# tree. Mirrors the [Files] section, including its Excludes.
# --------------------------------------------------------------------------
function Get-PayloadMap {
    param([string]$Root)
    $map = [ordered]@{}
    $map['autoshade.exe'] = Join-Path $Root 'dist\autoshade.exe'
    $map['autoshade-gui.exe'] = Join-Path $Root 'dist\autoshade-gui.exe'
    $map['LICENSE'] = Join-Path $Root 'LICENSE'

    $assets = Join-Path $Root 'assets'
    foreach ($f in Get-ChildItem -LiteralPath $assets -Recurse -File) {
        $map["assets\$($f.FullName.Substring($assets.Length + 1))"] = $f.FullName
    }

    $python = Join-Path $Root 'python'
    foreach ($f in Get-ChildItem -LiteralPath $python -Recurse -File) {
        $rel = $f.FullName.Substring($python.Length + 1)
        if ($rel -like 'weights\*') { continue }
        if ($rel -like '*__pycache__\*') { continue }
        if ($rel -like '*.pyc') { continue }
        if ((Split-Path $rel -Leaf) -like 'test_*.py') { continue }
        $map["python\$rel"] = $f.FullName
    }
    return $map
}

function Assert-PayloadInstalled {
    param([string]$InstallDir, $Map, [string]$Label)
    $missing = New-Object System.Collections.Generic.List[string]
    $wrong = New-Object System.Collections.Generic.List[string]
    foreach ($rel in $Map.Keys) {
        $installed = Join-Path $InstallDir $rel
        if (-not (Test-Path -LiteralPath $installed -PathType Leaf)) {
            $missing.Add($rel)
        }
        elseif ((Get-Sha256 $installed) -ne (Get-Sha256 $Map[$rel])) {
            $wrong.Add($rel)
        }
    }
    Assert-That ($missing.Count -eq 0) "$Label -- every shipped file is present" ($missing -join ', ')
    Assert-That ($wrong.Count -eq 0) "$Label -- every shipped file hashes to this build" ($wrong -join ', ')
}

# --------------------------------------------------------------------------
# Source-level identity assertions. AppId is the upgrade mechanism, so it has to
# be a constant nobody can move by bumping a version, and the override that lets
# this script exist must be the only thing that changes it.
# --------------------------------------------------------------------------
function Assert-InstallerSource {
    param([string]$IssPath)
    $text = [IO.File]::ReadAllText($IssPath)
    Assert-That ($text -match '(?m)^\s*#define RawAppId "\{B2C8B506-4DD8-4F06-B25D-7A3FBE9A742C\}"\s*$') `
        'the shipped AppId is the constant GUID'
    Assert-That ($text -match '(?ms)#ifndef AppIdOverride.*?#define RawAppId "\{B2C8B506-.*?#else') `
        'the constant is the branch taken when no override is passed'
    Assert-That ($text -match '(?m)^AppId=\{#AppIdSetting\}\s*$') 'AppId reads that constant'
    Assert-That (-not ($text -match '(?m)^AppId=.*AppVersion')) 'AppId is not derived from the version'
    Assert-That ($text -match '(?m)^UsePreviousAppDir=yes\s*$') 'UsePreviousAppDir=yes'
    Assert-That ($text -match '(?m)^UsePreviousGroup=no\s*$') 'UsePreviousGroup=no'
    Assert-That ($text -match '(?m)^CloseApplications=yes\s*$') 'CloseApplications=yes'
    Assert-That ($text -match '(?m)^RestartApplications=no\s*$') 'RestartApplications=no'
    Assert-That ($text -match '(?m)^DisableWelcomePage=no\s*$') 'the welcome page is shown'

    $runSection = [regex]::Match($text, '(?ms)^\[Run\]\r?\n(?<body>.*?)(?=^\[|\z)')
    Assert-That ($runSection.Success) '[Run] section exists'
    if ($runSection.Success) {
        $entries = @($runSection.Groups['body'].Value -split "`r?`n" |
            ForEach-Object { $_.Trim() } |
            Where-Object { ($_ -ne '') -and (-not $_.StartsWith(';')) })
        Assert-That ($entries.Count -eq 0) 'nothing is launched after install' ($entries -join ' | ')
    }
    Assert-That ($text -match 'function DeleteDataSwitchGiven') 'the uninstaller parses a /DELETEDATA switch'
    Assert-That ($text -match 'ParamStr\(I\)') 'it parses it out of ParamStr'
    # The interactive question itself cannot be run from here -- putting a modal
    # dialog on screen is exactly what a scripted run must never do -- so what
    # is checked is what the dialog is made of: it names both measured sizes,
    # and both of its defaults are the answer that keeps the data.
    Assert-That ($text -match 'SizePhrase\(WeightsBytes\)') 'the question names the size of the weights'
    Assert-That ($text -match 'SizePhrase\(StoreBytes\)') 'the question names the size of the develop store'
    Assert-That ($text -match 'MB_YESNO or MB_DEFBUTTON2, IDNO') 'keeping the data is the default answer'
    Assert-That ($text -match 'SuppressibleMsgBox') 'message boxes honour /SUPPRESSMSGBOXES'
    # The one branch a scenario run must never take: proving it by installing
    # would mean pointing AUTOSHADE_DATA_DIR at %LOCALAPPDATA% and running an
    # uninstaller that, without the guard, deletes it. So it is asserted here.
    Assert-That ($text -match 'function StoreIsOursToDelete') `
        'the uninstaller refuses to delete a store path it does not own'
    Assert-That ($text -match 'if not StoreIsOursToDelete\(Store\) then') `
        'and the guard is on the call that deletes it'
    Assert-That (-not (($text -replace 'SuppressibleMsgBox', '') -match 'MsgBox\(')) `
        'no plain MsgBox is left to hang a silent run'
}

# ==========================================================================
# Preflight
# ==========================================================================
if ([string]::IsNullOrWhiteSpace($PayloadRoot)) {
    $PayloadRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
}
$issPath = Join-Path $PayloadRoot 'installer\autoshade.iss'

Write-Head 'The installer source says what it must say'
Assert-InstallerSource $issPath

if ($SourceOnly) {
    Write-Head 'result'
    Write-Host "$script:Checks assertions, $($script:Failures.Count) failed"
    foreach ($failure in $script:Failures) { Write-Host $failure }
    if ($script:Failures.Count -gt 0) {
        throw "$($script:Failures.Count) installer source assertion(s) failed"
    }
    Write-Host 'every installer source assertion passed'
    return
}

foreach ($needed in @('BaseSetup', 'BaseVersion', 'UpgradeSetup', 'UpgradeVersion',
                      'DowngradeSetup', 'DowngradeVersion', 'AppId', 'AppName')) {
    if ([string]::IsNullOrWhiteSpace((Get-Variable -Name $needed -ValueOnly))) {
        throw "-$needed is required unless -SourceOnly is given."
    }
}

if ([string]::IsNullOrWhiteSpace($WorkRoot)) {
    $tempRoot = $env:RUNNER_TEMP
    if ([string]::IsNullOrWhiteSpace($tempRoot)) { $tempRoot = $env:TEMP }
    if ([string]::IsNullOrWhiteSpace($tempRoot)) {
        throw 'No work root: pass -WorkRoot, or set RUNNER_TEMP or TEMP.'
    }
    $WorkRoot = Join-Path $tempRoot 'autoshade-installer-scenarios'
}

foreach ($required in @($BaseSetup, $UpgradeSetup, $DowngradeSetup)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "Installer not found: $required"
    }
}
$BaseSetup = (Resolve-Path -LiteralPath $BaseSetup).Path
$UpgradeSetup = (Resolve-Path -LiteralPath $UpgradeSetup).Path
$DowngradeSetup = (Resolve-Path -LiteralPath $DowngradeSetup).Path
$PayloadRoot = (Resolve-Path -LiteralPath $PayloadRoot).Path

$installDir = Join-Path $WorkRoot 'app'
$storeDir = Join-Path $WorkRoot 'store'
$logDir = Join-Path $WorkRoot 'logs'
$serveDir = Join-Path $WorkRoot 'serve'

# Scenario 1 is a FRESH install and every later assertion is written against
# that. An entry left behind by an interrupted earlier run would silently turn
# it into an upgrade, so the run stops before it deletes anything -- while that
# run's uninstaller is still on disk to be used.
if ($null -ne (Get-UninstallEntry $AppId)) {
    throw ("$AppId is still registered from an earlier run. Uninstall it with " +
        "'$installDir\unins000.exe' /VERYSILENT /SUPPRESSMSGBOXES /DELETEDATA=1, then rerun.")
}
foreach ($stale in @($installDir, $storeDir, $logDir, $serveDir)) {
    if (Test-Path -LiteralPath $stale) { Remove-Item -LiteralPath $stale -Recurse -Force }
}
foreach ($fresh in @($WorkRoot, $storeDir, $logDir, $serveDir)) {
    New-Item -ItemType Directory -Force -Path $fresh | Out-Null
}

# The installer records this as the store it must offer to delete, so it has to
# be set before the FIRST install and left alone afterwards.
$env:AUTOSHADE_DATA_DIR = $storeDir

$group = Get-StartMenuGroup $AppName
$payload = Get-PayloadMap $PayloadRoot

Write-Head 'What this run must not disturb'
$pathBefore = Get-UserPathRaw
$script:PathOriginal = $pathBefore
if ($PathSuffix -ne '') {
    Set-UserPathRaw ($script:PathOriginal + $PathSuffix)
    $pathBefore = Get-UserPathRaw
    Write-Host "user PATH given the suffix '$PathSuffix' for this run ($($script:PathOriginal.Length) -> $($pathBefore.Length) characters)"
}
# Whatever stops this script early, the suffix comes off again.
trap {
    if ($PathSuffix -ne '') {
        Set-UserPathRaw $script:PathOriginal
        Write-Host "user PATH restored to its original $($script:PathOriginal.Length) characters after an error"
    }
    break
}
$autoshopKeyBefore = Test-Path -LiteralPath 'HKCU:\Software\Autoshop'
[IO.File]::WriteAllText((Join-Path $WorkRoot 'user-path-before.txt'), $pathBefore)
Write-Host "user PATH: $($pathBefore.Length) characters, $(@(Get-UserPathEntries).Count) entries"
$forbiddenBefore = $null
$forbiddenGroupBefore = $null
$forbiddenDirBefore = $null
if (-not [string]::IsNullOrWhiteSpace($ForbiddenAppId)) {
    $forbiddenBefore = Get-UninstallEntry $ForbiddenAppId
    if ($null -ne $forbiddenBefore) {
        $forbiddenDirBefore = Get-TreeSignature ([string]$forbiddenBefore.InstallLocation)
        Write-Host "shipped install: $($forbiddenBefore.DisplayName) $($forbiddenBefore.DisplayVersion) at $($forbiddenBefore.InstallLocation)"
    }
    else {
        Write-Host "shipped install: none registered under $ForbiddenAppId"
    }
    $forbiddenGroupBefore = Get-TreeSignature (Get-StartMenuGroup 'AutoShade')
}
Assert-That ($AppId -ne $ForbiddenAppId) 'the scenarios run under an identity of their own'

# ==========================================================================
# 1. Fresh install of the base version
# ==========================================================================
Write-Head "1. fresh install of $BaseVersion into $installDir"
$log1 = Join-Path $logDir '01-fresh-install.log'
$code = Invoke-Silently $BaseSetup `
    "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /DIR=`"$installDir`" /TASKS=`"addtopath`" /LOG=`"$log1`"" `
    'fresh install'
Assert-Same $code 0 'the fresh install succeeds'
Assert-That (Test-Path -LiteralPath (Join-Path $installDir 'autoshade.exe')) 'the CLI is installed'
Assert-That (Test-Path -LiteralPath (Join-Path $installDir 'autoshade-gui.exe')) 'the desktop app is installed'
Assert-That (Test-Path -LiteralPath (Join-Path $installDir 'unins000.exe')) 'an uninstaller is installed'

$entry = Get-UninstallEntry $AppId
Assert-That ($null -ne $entry) 'Programs and Features has an entry'
if ($null -ne $entry) {
    Assert-Same $entry.DisplayVersion $BaseVersion 'the entry names the installed version'
    Assert-Same $entry.DisplayName "$AppName version $BaseVersion" 'the entry names the program'
}
Assert-Same (Get-DisplayNameEntryCount $AppName) 1 'exactly one Programs and Features entry'
Assert-That (Test-Path -LiteralPath (Join-Path $group "Uninstall $AppName.lnk")) 'the Start Menu offers an uninstall shortcut'
Assert-Same @(Get-ChildItem -LiteralPath $group -Filter '*.lnk').Count 3 'the Start Menu group holds three shortcuts'

if ($BaseFromThisTree) {
    Assert-Same (Get-PathEntryCount $installDir) 1 'exactly one PATH entry'
    Assert-LogContains $log1 "Welcome page: Setup will install $AppName $BaseVersion on this computer." `
        'the welcome page says install, not upgrade'
    Assert-LogContains $log1 "Recorded the develop store for uninstall: $storeDir" `
        'the install records which develop store it belongs to'
    Assert-PayloadInstalled $installDir $payload 'fresh install'
}

Write-Head 'planting the two things an upgrade must not touch'
$weightsDir = Join-Path $installDir 'python\weights'
New-Item -ItemType Directory -Force -Path (Join-Path $weightsDir 'nested') | Out-Null
$random = New-Object byte[] 1048576
(New-Object Random 20260902).NextBytes($random)
[IO.File]::WriteAllBytes((Join-Path $weightsDir 'birefnet.marker'), $random)
[IO.File]::WriteAllBytes((Join-Path $weightsDir 'nested\oneformer.marker'), $random[0..262143])
New-Item -ItemType Directory -Force -Path (Join-Path $storeDir 'develops') | Out-Null
[IO.File]::WriteAllText((Join-Path $storeDir 'develops\edit.json'), '{"marker":"the user work an uninstall must be asked about"}')

# ==========================================================================
# 2. Upgrade in place, over a running program
# ==========================================================================
Write-Head 'holding the installed CLI open, so the upgrade has something to close'
# A running CLI holds {app}\autoshade.exe open, and Restart Manager has to close
# it or the upgrade cannot replace the file. The GUI is never started by this
# script -- it would put a window on screen -- and the CLI locks a file in the
# same directory, which is all CloseApplications needs to have work to do.
$runningExe = Join-Path $installDir 'autoshade.exe'
$running = Start-Process -FilePath $runningExe `
    -ArgumentList "serve `"$serveDir`" --port 48213" -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 3
Assert-That (-not $running.HasExited) "the installed CLI is running and holding $runningExe open" `
    "it exited with $($running.ExitCode)"
Write-Host "      pid $($running.Id)"

Write-Head 'corrupting four installed files, so a replacement that does not happen is visible'
# autoshade.exe is deliberately not on this list: the process above has it
# locked, so it cannot be overwritten -- and the upgrade replacing a LOCKED exe
# is a stronger statement than replacing a junk one.
foreach ($victim in @('LICENSE', 'autoshade-gui.exe', 'python\segment.py', 'assets\autoshade.ico')) {
    $target = Join-Path $installDir $victim
    if (Test-Path -LiteralPath $target) {
        [IO.File]::WriteAllText($target, "this is not the file the installer ships`n")
        Write-Host "      overwrote $victim"
    }
    else {
        Write-Host "      $victim is not in this base payload, skipped"
    }
}

# Taken here rather than when the markers were planted: the holder process is
# running against this very store, and a file IT writes is not the installer's
# doing. This is the last moment before setup starts.
$weightsBefore = Get-TreeSignature $weightsDir
$weightsHash = Get-Sha256 (Join-Path $weightsDir 'birefnet.marker')
$storeBefore = Get-TreeSignature $storeDir
Write-Host "weights: $(@(Get-ChildItem -LiteralPath $weightsDir -Recurse -File).Count) files"
Write-Host "store:   $(@(Get-ChildItem -LiteralPath $storeDir -Recurse -File).Count) files"
Write-Head "2. upgrade $BaseVersion -> $UpgradeVersion, no /DIR given"
$log2 = Join-Path $logDir '02-upgrade.log'
try {
    $code = Invoke-Silently $UpgradeSetup `
        "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /TASKS=`"addtopath`" /LOG=`"$log2`"" `
        'upgrade'
}
finally {
    # Only ever a process this script started, and only if Restart Manager
    # left it running -- which is itself a failure, asserted just below.
    if (-not $running.HasExited) {
        Write-Host '      the holder process outlived setup; stopping it'
        $running.Kill()
        $running.WaitForExit(10000) | Out-Null
    }
}
Assert-Same $code 0 'the silent upgrade succeeds'
Assert-That (Test-Path -LiteralPath (Join-Path $installDir 'autoshade.exe')) 'it landed in the same directory'
Assert-That (-not (Test-Path -LiteralPath (Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) "Programs\$AppName"))) `
    'it did not fall back to the default directory'
Assert-PayloadInstalled $installDir $payload 'upgrade'
Assert-Same (Get-Sha256 (Join-Path $weightsDir 'birefnet.marker')) $weightsHash 'the downloaded weights are byte-identical'
Assert-Same (Get-TreeSignature $weightsDir) $weightsBefore 'the weights directory is unchanged'
Assert-Same (Get-TreeSignature $storeDir) $storeBefore 'the develop store is unchanged'

$entry = Get-UninstallEntry $AppId
Assert-That ($null -ne $entry) 'the Programs and Features entry is still there'
if ($null -ne $entry) {
    Assert-Same $entry.DisplayVersion $UpgradeVersion 'it now names the new version'
    Assert-Same $entry.InstallLocation (Join-Path $installDir '') 'it still points at the same directory'
}
Assert-Same (Get-DisplayNameEntryCount $AppName) 1 'still exactly one Programs and Features entry'
Assert-Same (Get-PathEntryCount $installDir) 1 'still exactly one PATH entry'
Assert-Same @(Get-ChildItem -LiteralPath $group -Filter '*.lnk').Count 3 'the shortcuts were replaced, not duplicated'
Assert-LogContains $log2 "Welcome page: Setup will upgrade $AppName from $BaseVersion to $UpgradeVersion on this computer." `
    'the welcome page names both versions'
# Inno 6.7's own wording, read out of Setup.e32; the %s it fills in is the
# executable Restart Manager reported.
Assert-LogMatches $log2 'RestartManager found an application using one of our files: \S*autoshade' `
    'the log names the running program it found'
Assert-LogContains $log2 'Shutting down applications using our files.' `
    'the log records that it closed it'
Assert-That $running.HasExited 'the running program was closed rather than left holding the file'

# ==========================================================================
# 3. Downgrade refusal
# ==========================================================================
Write-Head "3. running the $DowngradeVersion installer over $UpgradeVersion"
$log3 = Join-Path $logDir '03-downgrade.log'
$code = Invoke-Silently $DowngradeSetup `
    "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /LOG=`"$log3`"" `
    'downgrade attempt'
Assert-That ($code -ne 0) 'the downgrade is refused with a non-zero exit code' "exit code $code"
Assert-LogContains $log3 "Refusing to downgrade: $AppName $UpgradeVersion is installed and this installer carries $DowngradeVersion." `
    'the log names both versions'
# A refusal that puts a dialog on screen hangs every scripted caller until
# somebody clicks it. Inno's plain MsgBox does exactly that even under
# /SUPPRESSMSGBOXES; SuppressibleMsgBox logs this line instead.
Assert-LogContains $log3 'Defaulting to OK for suppressed message box' `
    'the refusal answers itself instead of waiting for a click'
$entry = Get-UninstallEntry $AppId
Assert-That ($null -ne $entry) 'the install is still registered'
if ($null -ne $entry) {
    Assert-Same $entry.DisplayVersion $UpgradeVersion 'the installed version did not move'
}
Assert-PayloadInstalled $installDir $payload 'after the refused downgrade'

# ==========================================================================
# 4. Uninstall, keeping the data
# ==========================================================================
Write-Head '4. silent uninstall with no /DELETEDATA switch'
$log4 = Join-Path $logDir '04-uninstall-keep.log'
$code = Invoke-Silently (Join-Path $installDir 'unins000.exe') `
    "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=`"$log4`"" `
    'uninstall (keep)'
Wait-ForUninstaller $installDir
Assert-Same $code 0 'the uninstaller starts cleanly'
Assert-That (-not (Test-Path -LiteralPath (Join-Path $installDir 'autoshade.exe'))) 'the CLI is gone'
Assert-That (-not (Test-Path -LiteralPath (Join-Path $installDir 'autoshade-gui.exe'))) 'the desktop app is gone'
Assert-That (-not (Test-Path -LiteralPath (Join-Path $installDir 'LICENSE'))) 'the licence is gone'
Assert-That (-not (Test-Path -LiteralPath (Join-Path $installDir 'assets'))) 'the assets are gone'
Assert-That (-not (Test-Path -LiteralPath (Join-Path $installDir 'python\segment.py'))) 'the sidecars are gone'
Assert-That (-not (Test-Path -LiteralPath $group)) 'the Start Menu group is gone'
Assert-Same (Get-DisplayNameEntryCount $AppName) 0 'the Programs and Features entry is gone'
Assert-Same (Get-PathEntryCount $installDir) 0 'the PATH entry is gone'
Assert-UserPathRestored $pathBefore 'the user PATH is byte-identical to before the run'
# Whichever key this build wrote -- the shipped one on a runner with no other
# AutoShade, the scenario one on a developer machine -- HKCU\Software\Autoshop
# has to be back to the state the run found it in. Naming only the scenario key
# would pass on CI without having checked anything.
Assert-Same (Test-Path -LiteralPath 'HKCU:\Software\Autoshop') $autoshopKeyBefore `
    "the installer's own state key is gone"
Assert-Same (Get-Sha256 (Join-Path $weightsDir 'birefnet.marker')) $weightsHash 'the weights were kept'
Assert-Same (Get-TreeSignature $weightsDir) $weightsBefore 'every weights file was kept'
Assert-Same (Get-TreeSignature $storeDir) $storeBefore 'the develop store was kept'
Assert-That (Test-Path -LiteralPath $installDir) 'the folder stays, because the data in it stayed'
Assert-LogContains $log4 'Silent uninstall without /DELETEDATA=1: keeping the weights and the develop store.' `
    'the log says it kept them and why'
# The two numbers the interactive dialog would put in front of the user are
# measured by the same code in both modes and written to the log before the
# silent branch, so a silent run is enough to check that they are the real
# sizes of the two directories.
$measured = [regex]::Match((Read-Log $log4),
    'User data: model weights (?<w>\d+) bytes at .*?; develop store (?<s>\d+) bytes at ')
Assert-That $measured.Success 'the uninstaller measured both directories before deciding'
if ($measured.Success) {
    $weightsBytes = (Get-ChildItem -LiteralPath $weightsDir -Recurse -File -Force |
        Measure-Object -Property Length -Sum).Sum
    $storeBytes = (Get-ChildItem -LiteralPath $storeDir -Recurse -File -Force |
        Measure-Object -Property Length -Sum).Sum
    Assert-Same $measured.Groups['w'].Value $weightsBytes 'it measured the weights correctly'
    Assert-Same $measured.Groups['s'].Value $storeBytes 'it measured the develop store correctly'
}

# ==========================================================================
# 5. Reinstall over the kept data
# ==========================================================================
Write-Head "5. installing $UpgradeVersion again into the folder the data is in"
$log5 = Join-Path $logDir '05-reinstall.log'
$code = Invoke-Silently $UpgradeSetup `
    "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /DIR=`"$installDir`" /TASKS=`"addtopath`" /LOG=`"$log5`"" `
    'reinstall'
Assert-Same $code 0 'the reinstall succeeds'
Assert-PayloadInstalled $installDir $payload 'reinstall'
Assert-LogContains $log5 "Welcome page: Setup will install $AppName $UpgradeVersion on this computer." `
    'with no entry to read, the wording is install again'
Assert-Same (Get-TreeSignature $weightsDir) $weightsBefore 'the kept weights were picked back up'
Assert-Same (Get-TreeSignature $storeDir) $storeBefore 'the kept develop store was picked back up'
Assert-Same (Get-PathEntryCount $installDir) 1 'exactly one PATH entry again'

# ==========================================================================
# 6. Uninstall, deleting the data
# ==========================================================================
Write-Head '6. silent uninstall with /DELETEDATA=1'
$log6 = Join-Path $logDir '06-uninstall-delete.log'
$code = Invoke-Silently (Join-Path $installDir 'unins000.exe') `
    "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /DELETEDATA=1 /LOG=`"$log6`"" `
    'uninstall (delete)'
Wait-ForUninstaller $installDir
Assert-Same $code 0 'the uninstaller starts cleanly'
Assert-LogContains $log6 'Silent uninstall with /DELETEDATA=1: deleting the weights and the develop store.' `
    'the log says it was told to delete them'
Assert-LogContains $log6 "Deleted the develop store: $storeDir" 'the log names the store it deleted'
Assert-That (-not (Test-Path -LiteralPath $storeDir)) 'the develop store is gone'
Assert-That (-not (Test-Path -LiteralPath $installDir)) 'the install folder is gone entirely'
Assert-Same (Get-DisplayNameEntryCount $AppName) 0 'no Programs and Features entry'
Assert-Same (Get-PathEntryCount $installDir) 0 'no PATH entry'
Assert-That (-not (Test-Path -LiteralPath $group)) 'no Start Menu group'

# ==========================================================================
# Cleanup proof
# ==========================================================================
Write-Head 'cleanup proof'
Assert-That (-not (Test-Path -LiteralPath (Get-UninstallKeyPath $AppId))) "no registry key for $AppId"
Assert-That (-not (Test-Path -LiteralPath $installDir)) "no directory at $installDir"
Assert-That (-not (Test-Path -LiteralPath $group)) "no Start Menu folder at $group"
Assert-UserPathRestored $pathBefore 'the user PATH is byte-identical to before the run'
Assert-That (-not (Test-Path -LiteralPath 'HKCU:\Software\Autoshop\InstallerScenario')) `
    'no scenario state left in the registry'
Assert-Same (Test-Path -LiteralPath 'HKCU:\Software\Autoshop') $autoshopKeyBefore `
    'the shared installer-state parent key is exactly as it was found'

if (-not [string]::IsNullOrWhiteSpace($ForbiddenAppId)) {
    $forbiddenAfter = Get-UninstallEntry $ForbiddenAppId
    if ($null -eq $forbiddenBefore) {
        Assert-That ($null -eq $forbiddenAfter) 'the shipped identity was not registered by this run'
    }
    else {
        Assert-That ($null -ne $forbiddenAfter) "the shipped install's registry entry survived"
        if ($null -ne $forbiddenAfter) {
            Assert-Same $forbiddenAfter.DisplayVersion $forbiddenBefore.DisplayVersion `
                "the shipped install's version is untouched"
            Assert-Same $forbiddenAfter.InstallLocation $forbiddenBefore.InstallLocation `
                "the shipped install's location is untouched"
            Assert-Same (Get-TreeSignature ([string]$forbiddenAfter.InstallLocation)) $forbiddenDirBefore `
                "every file of the shipped install is untouched"
        }
    }
    Assert-Same (Get-TreeSignature (Get-StartMenuGroup 'AutoShade')) $forbiddenGroupBefore `
        "the shipped install's Start Menu folder is untouched"
}

if ($PathSuffix -ne '') {
    Set-UserPathRaw $script:PathOriginal
    Assert-Same (Get-UserPathRaw) $script:PathOriginal 'the user PATH is back to the bytes found before the suffix went on'
}

Write-Head 'result'
Write-Host "$script:Checks assertions, $($script:Failures.Count) failed"
if ($script:Failures.Count -gt 0) {
    foreach ($failure in $script:Failures) { Write-Host $failure }
    throw "$($script:Failures.Count) installer scenario assertion(s) failed"
}
Write-Host 'every installer scenario assertion passed'
