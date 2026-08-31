#ifndef AppVersion
  #error "AppVersion is required. Compile with ISCC.exe /DAppVersion=x.y.z installer\autoshade.iss"
#endif

#define AppName "AutoShade"
#define AppPublisher "skymanbp"
#define AppURL "https://github.com/skymanbp/autoshop"

[Setup]
AppId={{B2C8B506-4DD8-4F06-B25D-7A3FBE9A742C}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
AppUpdatesURL={#AppURL}/releases
DefaultDirName={autopf}\AutoShade
DefaultGroupName=AutoShade
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\LICENSE
OutputDir=..\target\installer
OutputBaseFilename=AutoShade-Setup-{#AppVersion}
SetupIconFile=autoshade.ico
UninstallDisplayIcon={app}\autoshade-gui.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
SetupLogging=yes
ChangesEnvironment=yes
CloseApplications=yes
RestartApplications=no

; User data intentionally survives uninstall. The per-user develop store is
; %LOCALAPPDATA%\autoshade\, outside {app}, and no [UninstallDelete] entry targets it.
; Downloaded model weights are not installer payloads either; the sidecars fetch
; them on first use into python\weights.

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked
; This updates only HKCU\Environment\Path, so it never needs elevation. Existing
; processes retain their old environment block; start a new terminal after setup.
Name: "addtopath"; Description: "Add the AutoShade CLI to my user &PATH (new terminals only)"; GroupDescription: "Command-line integration:"; Flags: unchecked

[Files]
Source: "..\dist\autoshade.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\autoshade-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs
; Runtime sidecars are copied recursively for forward-compatible additions, while
; developer tests, Python bytecode, and multi-gigabyte downloaded weights stay out.
Source: "..\python\*"; DestDir: "{app}\python"; Excludes: "weights\*,__pycache__\*,test_*.py,*.pyc"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\AutoShade"; Filename: "{app}\autoshade-gui.exe"; WorkingDir: "{app}"; Comment: "AutoShade desktop application"
Name: "{group}\AutoShade CLI"; Filename: "{app}\autoshade.exe"; WorkingDir: "{app}"; Comment: "AutoShade command-line interface (opens a console window)"
Name: "{group}\Uninstall AutoShade"; Filename: "{uninstallexe}"
Name: "{autodesktop}\AutoShade"; Filename: "{app}\autoshade-gui.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
; Deliberately no post-install launch: packaging validation must never start the GUI.

[Code]
const
  UserEnvironmentKey = 'Environment';
  // INSTALL STATE, not a display name — so it keeps the pre-rename spelling
  // for the same reason AppId does. This key records that THIS install put its
  // directory on the user's PATH, and the uninstaller removes that entry only
  // when it finds the marker. Renaming the key would hide the marker an
  // earlier Autoshop install wrote — same AppId, so setup upgrades in place —
  // and its uninstall would leave a dead directory on the user's PATH.
  InstallerStateKey = 'Software\Autoshop\Installer';
  PathMarkerName = 'PathAddedByInstaller';

function NormalizedPathEntry(Value: String): String;
begin
  Result := Trim(Value);
  if (Length(Result) >= 2) and (Result[1] = '"') and
     (Result[Length(Result)] = '"') then
  begin
    Delete(Result, Length(Result), 1);
    Delete(Result, 1, 1);
  end;
  StringChangeEx(Result, '/', '\', True);
  while (Length(Result) > 3) and (Result[Length(Result)] = '\') do
    Delete(Result, Length(Result), 1);
end;

function PathContainsEntry(const PathValue, Wanted: String): Boolean;
var
  Remaining, Entry: String;
  Separator: Integer;
begin
  Result := False;
  Remaining := PathValue;
  while True do
  begin
    Separator := Pos(';', Remaining);
    if Separator = 0 then
    begin
      Entry := Remaining;
      Remaining := '';
    end
    else
    begin
      Entry := Copy(Remaining, 1, Separator - 1);
      Delete(Remaining, 1, Separator);
    end;

    if CompareText(NormalizedPathEntry(Entry),
                   NormalizedPathEntry(Wanted)) = 0 then
    begin
      Result := True;
      Exit;
    end;

    if Separator = 0 then
      Exit;
  end;
end;

procedure AddInstallDirToUserPath;
var
  ExistingPath, NewPath, InstallDir: String;
begin
  InstallDir := ExpandConstant('{app}');
  if not RegQueryStringValue(HKCU, UserEnvironmentKey, 'Path', ExistingPath) then
    ExistingPath := '';

  if PathContainsEntry(ExistingPath, InstallDir) then
  begin
    Log('PATH task: install directory already exists in the user PATH; unchanged.');
    Exit;
  end;

  NewPath := ExistingPath;
  if (NewPath <> '') and (NewPath[Length(NewPath)] <> ';') then
    NewPath := NewPath + ';';
  NewPath := NewPath + InstallDir;

  if not RegWriteExpandStringValue(HKCU, UserEnvironmentKey, 'Path', NewPath) then
    RaiseException('Could not update the current user PATH.');
  if not RegWriteDWordValue(HKCU, InstallerStateKey, PathMarkerName, 1) then
    RaiseException('PATH was updated, but the uninstall marker could not be written.');
  Log('PATH task: appended the install directory to HKCU\Environment\Path.');
end;

procedure RemoveInstallDirFromUserPath;
var
  ExistingPath, Remaining, Entry, NewPath, InstallDir: String;
  Separator: Integer;
  HaveOutput, LastEntry: Boolean;
  Marker: Cardinal;
begin
  if (not RegQueryDWordValue(HKCU, InstallerStateKey, PathMarkerName, Marker)) or
     (Marker <> 1) then
    Exit;

  InstallDir := ExpandConstant('{app}');
  if RegQueryStringValue(HKCU, UserEnvironmentKey, 'Path', ExistingPath) then
  begin
    Remaining := ExistingPath;
    NewPath := '';
    HaveOutput := False;
    while True do
    begin
      Separator := Pos(';', Remaining);
      LastEntry := Separator = 0;
      if LastEntry then
      begin
        Entry := Remaining;
        Remaining := '';
      end
      else
      begin
        Entry := Copy(Remaining, 1, Separator - 1);
        Delete(Remaining, 1, Separator);
      end;

      if CompareText(NormalizedPathEntry(Entry),
                     NormalizedPathEntry(InstallDir)) <> 0 then
      begin
        if HaveOutput then
          NewPath := NewPath + ';';
        NewPath := NewPath + Entry;
        HaveOutput := True;
      end;

      if LastEntry then
        Break;
    end;

    if NewPath <> ExistingPath then
    begin
      if not RegWriteExpandStringValue(HKCU, UserEnvironmentKey, 'Path', NewPath) then
      begin
        Log('PATH cleanup: could not update HKCU\Environment\Path; leaving marker in place.');
        Exit;
      end;
      Log('PATH cleanup: removed the installer-added install directory.');
    end;
  end;

  RegDeleteValue(HKCU, InstallerStateKey, PathMarkerName);
  RegDeleteKeyIfEmpty(HKCU, InstallerStateKey);
  RegDeleteKeyIfEmpty(HKCU, 'Software\Autoshop');
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddInstallDirToUserPath;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    RemoveInstallDirFromUserPath;
end;

