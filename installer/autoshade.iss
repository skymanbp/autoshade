#ifndef AppVersion
  #error "AppVersion is required. Compile with ISCC.exe /DAppVersion=x.y.z installer\autoshade.iss"
#endif

; IDENTITY. AppId is the whole in-place-upgrade mechanism -- Inno recognises an
; existing install by AppId alone -- so it is a constant that already survived
; the Autoshop -> AutoShade rename and must never be derived from the version.
; A scenario run needs a DIFFERENT identity, because testing an upgrade
; otherwise means installing over the user's own copy; so the id and the
; display name can both be replaced on the ISCC command line:
;   ISCC /DAppVersion=1.2.3 /DAppIdOverride={2F0A...} /DAppNameOverride="AutoShade Test"
; with RAW braces -- the AppId= line below adds the escaping brace itself. A
; release compile passes neither define and therefore always gets the two
; constants here; scripts/installer_scenarios.ps1 asserts exactly that, and
; asserts the shipped uninstall key is never written while the scenarios run.
#ifndef AppIdOverride
  #define ShippedIdentity
  #define RawAppId "{B2C8B506-4DD8-4F06-B25D-7A3FBE9A742C}"
#else
  #define RawAppId AppIdOverride
#endif
#ifndef AppNameOverride
  #define AppName "AutoShade"
#else
  #define AppName AppNameOverride
#endif
; A leading `{{` is how an AppId says "one literal brace", so the setting
; carries one more brace than the GUID does.
#define AppIdSetting "{" + RawAppId

#define AppPublisher "skymanbp"
#define AppURL "https://github.com/skymanbp/autoshade"

[Setup]
AppId={#AppIdSetting}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
AppUpdatesURL={#AppURL}/releases
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
; The upgrade guarantee: version N+1 lands in the directory version N chose,
; even when that is not DefaultDirName. This is Inno's default and is stated
; anyway, because deleting it would silently move a portable-drive install back
; under %LOCALAPPDATA% and orphan everything the user put beside it.
UsePreviousAppDir=yes
; The pre-rename group is deleted by name in [InstallDelete]; without this
; an upgrade would keep writing AutoShade shortcuts into a folder called
; Autoshop, because UsePreviousGroup defaults to yes.
UsePreviousGroup=no
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\LICENSE
OutputDir=..\target\installer
; Derived from the name so a scenario compile cannot land on the release
; artifact's filename; with no override this is exactly AutoShade-Setup-<v>.exe,
; the name scripts/build_installer.ps1 and the release workflow expect.
OutputBaseFilename={#AppName}-Setup-{#AppVersion}
SetupIconFile=autoshade.ico
UninstallDisplayIcon={app}\autoshade-gui.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
SetupLogging=yes
ChangesEnvironment=yes
; Inno 6 hides the welcome page by default. This installer shows it, because it
; is the one place an upgrade can say which version it is replacing before the
; user commits to it (see WelcomeSentence in [Code]).
DisableWelcomePage=no
; A running AutoShade holds its own .exe open, so an upgrade that did not close
; it would fail to replace the file. Restart Manager asks the app to close;
; nothing is restarted afterwards, because the finish page must never start the
; GUI (there is no [Run] section for the same reason).
CloseApplications=yes
RestartApplications=no

; User data intentionally survives uninstall unless the user says otherwise.
; The per-user develop store is %LOCALAPPDATA%\autoshade\ (or whatever
; AUTOSHADE_DATA_DIR names), outside {app}, and no [UninstallDelete] entry
; targets it. Downloaded model weights are not installer payloads either; the
; sidecars fetch them on first use into python\weights. Both are offered for
; deletion by the uninstaller, which asks; see AskAboutUserData in [Code].

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked
; This updates only HKCU\Environment\Path, so it never needs elevation. Existing
; processes retain their old environment block; start a new terminal after setup.
Name: "addtopath"; Description: "Add the AutoShade CLI to my user &PATH (new terminals only)"; GroupDescription: "Command-line integration:"; Flags: unchecked

[InstallDelete]
#ifdef ShippedIdentity
; The rename (Autoshop -> AutoShade) changed the name of every artifact this
; installer ships, but AppId deliberately did NOT (see the identity block at the
; top), so setup UPGRADES a pre-rename install in place -- and Inno leaves a file
; it no longer ships exactly where it is. Without this section an upgraded
; machine keeps two runnable CLIs and two runnable GUIs side by side, plus a
; Start Menu group whose shortcuts still launch the 1.0.x binaries. This section
; runs BEFORE [Files]. Every entry is a name that CHANGED in the rename: nothing
; here can reach a file the current payload still ships, the per-user develop
; store (%LOCALAPPDATA%\autoshade\, outside {app}), or python\weights.
;
; It is compiled only into the shipped identity. A scenario build installs under
; its own AppId and its own Start Menu group, and these entries name the REAL
; product's leftovers -- a test run must not be able to delete them.
Type: files; Name: "{app}\autoshop.exe"
Type: files; Name: "{app}\autoshop-gui.exe"
Type: files; Name: "{app}\assets\autoshop.ico"
Type: files; Name: "{app}\assets\fonts\*-autoshop.ttf"
; Shortcuts go by their own old names rather than by wiping the folder, so
; anything the user put in that group survives; the folder itself goes only if
; it empties. {group} is already the NEW group (UsePreviousGroup=no) and so
; cannot reach any of these.
Type: files; Name: "{autoprograms}\Autoshop\Autoshop.lnk"
Type: files; Name: "{autoprograms}\Autoshop\Autoshop CLI.lnk"
Type: files; Name: "{autoprograms}\Autoshop\Uninstall Autoshop.lnk"
Type: dirifempty; Name: "{autoprograms}\Autoshop"
Type: files; Name: "{autodesktop}\Autoshop.lnk"
#endif

[Files]
Source: "..\dist\autoshade.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\autoshade-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs
; Runtime sidecars are copied recursively for forward-compatible additions, while
; developer tests, Python bytecode, and multi-gigabyte downloaded weights stay out.
Source: "..\python\*"; DestDir: "{app}\python"; Excludes: "weights\*,__pycache__\*,test_*.py,*.pyc"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\autoshade-gui.exe"; WorkingDir: "{app}"; Comment: "AutoShade desktop application"
Name: "{group}\{#AppName} CLI"; Filename: "{app}\autoshade.exe"; WorkingDir: "{app}"; Comment: "AutoShade command-line interface (opens a console window)"
; The second uninstall door. The first is the Programs and Features entry Inno
; writes from AppId; a user who never opens that control panel still has this.
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\autoshade-gui.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
; Deliberately no post-install launch: packaging validation must never start the
; GUI, and a silent upgrade run by a script must never put a window on screen.

[Code]
const
  UserEnvironmentKey = 'Environment';
  // INSTALL STATE, not a display name — so it keeps the pre-rename spelling
  // for the same reason AppId does. This key records that THIS install put its
  // directory on the user's PATH, and the uninstaller removes that entry only
  // when it finds the marker. Renaming the key would hide the marker an
  // earlier Autoshop install wrote — same AppId, so setup upgrades in place —
  // and its uninstall would leave a dead directory on the user's PATH.
#ifdef ShippedIdentity
  InstallerStateKey = 'Software\Autoshop\Installer';
#else
  // A scenario compile keeps its own state. Sharing the key would let a test
  // uninstall delete the real install's PATH marker, after which the real
  // uninstall leaves a dead directory on the user's PATH forever.
  InstallerStateKey = 'Software\Autoshop\InstallerScenario';
#endif
  // 1 = setup wrote ';' and then the install directory; 2 = setup wrote the
  // directory alone, because the PATH was empty or already ended in ';'. The
  // uninstaller deletes exactly that span and nothing else, so a PATH that
  // carried a trailing or doubled separator before setup ran carries it after.
  PathMarkerName = 'PathAddedByInstaller';
  DevelopStoreName = 'DevelopStore';
  // Where Inno itself records this install. DisplayVersion in here is the only
  // way setup can learn which version it is about to replace.
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#RawAppId}_is1';

var
  // Decided once, during usUninstall, and acted on in usPostUninstall — the
  // weights and the store may only be removed after the files that use them.
  DeleteUserData: Boolean;

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

// The span of the entry in PathValue that names Wanted: Start is 1-based and
// Len is the entry's own length, separators excluded. False when absent.
function FindPathEntry(const PathValue, Wanted: String;
                       var Start, Len: Integer): Boolean;
var
  I, SegStart: Integer;
  Entry: String;
begin
  Result := False;
  SegStart := 1;
  I := 1;
  while I <= Length(PathValue) + 1 do
  begin
    if (I > Length(PathValue)) or (PathValue[I] = ';') then
    begin
      Entry := Copy(PathValue, SegStart, I - SegStart);
      if (Entry <> '') and
         (CompareText(NormalizedPathEntry(Entry),
                      NormalizedPathEntry(Wanted)) = 0) then
      begin
        Start := SegStart;
        Len := I - SegStart;
        Result := True;
        Exit;
      end;
      SegStart := I + 1;
    end;
    I := I + 1;
  end;
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

// Takes the leading run of digits off a dotted version string and steps over
// the separator. Anything that is not a digit ends the number and, unless it is
// a dot, ends the string: `3-rc1` yields 3 and then nothing, so a pre-release
// suffix never has to be ranked against a number.
function NextVersionNumber(var Rest: String): Integer;
var
  Digits: String;
begin
  Digits := '';
  while (Rest <> '') and (Rest[1] >= '0') and (Rest[1] <= '9') do
  begin
    Digits := Digits + Rest[1];
    Delete(Rest, 1, 1);
  end;
  if (Rest <> '') and (Rest[1] = '.') then
    Delete(Rest, 1, 1)
  else
    Rest := '';
  Result := StrToIntDef(Digits, 0);
end;

// Compares two dotted versions numerically, returning <0, 0 or >0. It has to be
// numeric: a text compare puts 1.2.10 BEFORE 1.2.9, and this is the function
// the downgrade refusal is built on.
function CompareVersionStrings(const A, B: String): Integer;
var
  RestA, RestB: String;
  NumA, NumB: Integer;
begin
  RestA := A;
  RestB := B;
  Result := 0;
  while (Result = 0) and ((RestA <> '') or (RestB <> '')) do
  begin
    NumA := NextVersionNumber(RestA);
    NumB := NextVersionNumber(RestB);
    if NumA < NumB then
      Result := -1
    else if NumA > NumB then
      Result := 1;
  end;
end;

// The version currently installed under this AppId, or False when there is no
// install to upgrade. Per-user installs write HKCU; an install made while
// elevated writes HKLM, and the same machine must still be recognised.
function PreviousInstallVersion(var Version: String): Boolean;
begin
  Result := RegQueryStringValue(HKCU, UninstallKey, 'DisplayVersion', Version);
  if not Result then
    Result := RegQueryStringValue(HKLM, UninstallKey, 'DisplayVersion', Version);
  if Result and (Trim(Version) = '') then
    Result := False;
end;

// The sentence the welcome page shows and the log records. The previous version
// is only knowable from the uninstall entry — {#AppVersion} is this installer's
// own — and a silent run never shows a page, so the log is the only place a
// /VERYSILENT upgrade can be read afterwards.
function WelcomeSentence: String;
var
  Installed: String;
begin
  if PreviousInstallVersion(Installed) then
    Result := Format('Setup will upgrade %s from %s to %s on this computer.', [
      '{#AppName}', Installed, '{#AppVersion}'])
  else
    Result := Format('Setup will install %s %s on this computer.', [
      '{#AppName}', '{#AppVersion}']);
end;

// Refuses to put an older build over a newer one. Inno would happily do it, and
// the result is a store written by a version the running program is older than.
// Returning False here ends setup with a non-zero exit code, which is what a
// silent caller has to go on; the log carries the reason either way. The call
// is SuppressibleMsgBox and not MsgBox on purpose: a plain MsgBox from Pascal
// Script displays even under /VERYSILENT /SUPPRESSMSGBOXES and then waits for a
// click that a scripted install has nobody to give it.
function InitializeSetup(): Boolean;
var
  Installed: String;
begin
  Result := True;
  if PreviousInstallVersion(Installed) and
     (CompareVersionStrings(Installed, '{#AppVersion}') > 0) then
  begin
    Log(Format('Refusing to downgrade: %s %s is installed and this installer carries %s.', [
      '{#AppName}', Installed, '{#AppVersion}']));
    SuppressibleMsgBox(Format('%s %s is already installed.' + #13#10#13#10 +
      'This installer carries version %s, which is older. Setup will not replace ' +
      'a newer install with an older one, because the develop store it would ' +
      'then be reading was written by %s.' + #13#10#13#10 +
      'Uninstall %s first if you really want to go back to %s.', [
      '{#AppName}', Installed, '{#AppVersion}', Installed, Installed, '{#AppVersion}']),
      mbCriticalError, MB_OK, IDOK);
    Result := False;
  end;
end;

procedure InitializeWizard;
var
  Sentence: String;
begin
  Sentence := WelcomeSentence;
  Log('Welcome page: ' + Sentence);
  WizardForm.WelcomeLabel2.Caption := Sentence + #13#10#13#10 +
    'Your develop store and any model weights already downloaded are left ' +
    'untouched. It is recommended that you close AutoShade before continuing; ' +
    'setup will offer to close it for you if you do not.';
end;

// The develop store, resolved the way the program resolves it (src/store.rs:
// AUTOSHADE_DATA_DIR names the root outright, otherwise it is
// %LOCALAPPDATA%\autoshade).
function ResolveDevelopStore: String;
begin
  Result := Trim(GetEnv('AUTOSHADE_DATA_DIR'));
  if Result = '' then
    Result := ExpandConstant('{localappdata}\autoshade');
end;

// The store this install actually used. It is RECORDED at install time rather
// than resolved at uninstall time, because the uninstaller offers to delete it
// and must never guess: an install made with AUTOSHADE_DATA_DIR pointing
// elsewhere would otherwise be uninstalled by deleting a directory it never
// owned. Falling back to the live resolution covers installs made before this
// value existed.
function RecordedDevelopStore: String;
begin
  if (not RegQueryStringValue(HKCU, InstallerStateKey, DevelopStoreName, Result)) or
     (Trim(Result) = '') then
    Result := ResolveDevelopStore;
end;

procedure RecordDevelopStore;
var
  Store: String;
begin
  Store := ResolveDevelopStore;
  if RegWriteStringValue(HKCU, InstallerStateKey, DevelopStoreName, Store) then
    Log('Recorded the develop store for uninstall: ' + Store)
  else
    Log('Could not record the develop store; uninstall will resolve it again.');
end;

procedure AddInstallDirToUserPath;
var
  ExistingPath, NewPath, InstallDir: String;
  Marker: Cardinal;
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
  Marker := 2;
  if (NewPath <> '') and (NewPath[Length(NewPath)] <> ';') then
  begin
    NewPath := NewPath + ';';
    Marker := 1;
  end;
  NewPath := NewPath + InstallDir;

  if not RegWriteExpandStringValue(HKCU, UserEnvironmentKey, 'Path', NewPath) then
    RaiseException('Could not update the current user PATH.');
  if not RegWriteDWordValue(HKCU, InstallerStateKey, PathMarkerName, Marker) then
    RaiseException('PATH was updated, but the uninstall marker could not be written.');
  if Marker = 1 then
    Log('PATH task: appended a separator and the install directory to HKCU\Environment\Path.')
  else
    Log('PATH task: appended the install directory to HKCU\Environment\Path (it already ended in a separator, or was empty).');
end;

procedure RemoveInstallDirFromUserPath;
var
  ExistingPath, NewPath, InstallDir: String;
  Start, Len: Integer;
  Marker: Cardinal;
begin
  if (not RegQueryDWordValue(HKCU, InstallerStateKey, PathMarkerName, Marker)) or
     (Marker = 0) then
    Exit;

  InstallDir := ExpandConstant('{app}');
  if not RegQueryStringValue(HKCU, UserEnvironmentKey, 'Path', ExistingPath) then
    Exit;
  if not FindPathEntry(ExistingPath, InstallDir, Start, Len) then
  begin
    Log('PATH cleanup: the install directory is no longer on the user PATH; nothing to remove.');
    Exit;
  end;

  // Take back exactly the bytes AddInstallDirToUserPath wrote: the entry, and
  // the one separator it put in front of the entry (marker 1). When it wrote
  // the entry straight after a separator the user already had (marker 2), the
  // entry alone goes, and that separator stays where it was. Re-joining the
  // parsed entries instead dropped a trailing ';' and would collapse a doubled
  // one — the user's PATH must come back byte for byte. Should the user have
  // moved the entry off the end, the separator on the side setup did not
  // write is taken instead, so no empty entry is left behind.
  NewPath := ExistingPath;
  if (Marker = 1) and (Start > 1) and (NewPath[Start - 1] = ';') then
    Delete(NewPath, Start - 1, Len + 1)
  else if (Marker = 2) and (Start + Len > Length(NewPath)) then
    Delete(NewPath, Start, Len)
  else if (Start + Len <= Length(NewPath)) and (NewPath[Start + Len] = ';') then
    Delete(NewPath, Start, Len + 1)
  else if (Start > 1) and (NewPath[Start - 1] = ';') then
    Delete(NewPath, Start - 1, Len + 1)
  else
    Delete(NewPath, Start, Len);

  if not RegWriteExpandStringValue(HKCU, UserEnvironmentKey, 'Path', NewPath) then
  begin
    Log('PATH cleanup: could not update HKCU\Environment\Path; leaving marker in place.');
    Exit;
  end;
  Log('PATH cleanup: removed the installer-added install directory and the separator setup wrote for it.');
end;

// Everything under the installer's state key describes THIS install, so the
// uninstall takes it along. The parent key goes only if nothing else is left
// under it — a scenario install and the real one both live there.
procedure ForgetInstallerState;
begin
  RegDeleteValue(HKCU, InstallerStateKey, PathMarkerName);
  RegDeleteValue(HKCU, InstallerStateKey, DevelopStoreName);
  RegDeleteKeyIfEmpty(HKCU, InstallerStateKey);
  RegDeleteKeyIfEmpty(HKCU, 'Software\Autoshop');
end;

// Adds up a directory tree, so the uninstall question can name what it is
// offering to delete. "832 MB of model weights" is a decision the user can
// make; "the model weights" is not.
function DirectoryBytes(const Dir: String): Int64;
var
  Rec: TFindRec;
begin
  Result := 0;
  if FindFirst(AddBackslash(Dir) + '*', Rec) then
  try
    repeat
      if (Rec.Name <> '.') and (Rec.Name <> '..') then
      begin
        if (Rec.Attributes and FILE_ATTRIBUTE_DIRECTORY) <> 0 then
          Result := Result + DirectoryBytes(AddBackslash(Dir) + Rec.Name)
        else
          Result := Result + (Int64(Rec.SizeHigh) * 4294967296) + Int64(Rec.SizeLow);
      end;
    until not FindNext(Rec);
  finally
    FindClose(Rec);
  end;
end;

// Whole units only: the two numbers exist to be compared with what the user
// knows about their disk, and a byte count is not a quantity anyone reads.
function SizePhrase(Bytes: Int64): String;
begin
  if Bytes >= 1048576 then
    Result := IntToStr(Bytes div 1048576) + ' MB'
  else if Bytes >= 1024 then
    Result := IntToStr(Bytes div 1024) + ' KB'
  else
    Result := IntToStr(Bytes) + ' bytes';
end;

// A silent uninstall cannot be asked, so the answer arrives as a switch.
// Absent, the answer is "keep" — the same default the dialog offers, and the
// only safe one for a directory holding the user's edits. The last occurrence
// wins, so a wrapper script can append an override to a fixed argument list.
function DeleteDataSwitchGiven: Boolean;
var
  I: Integer;
  Param: String;
begin
  Result := False;
  for I := 1 to ParamCount do
  begin
    Param := Uppercase(Trim(ParamStr(I)));
    if (Param = '/DELETEDATA=1') or (Param = '/DELETEDATA=YES') then
      Result := True
    else if (Param = '/DELETEDATA=0') or (Param = '/DELETEDATA=NO') then
      Result := False;
  end;
end;

// The uninstall's one question. Two things outlive the program files — the
// downloaded weights inside {app}\python\weights and the develop store outside
// {app} — and neither is the installer's to throw away without being told.
function AskAboutUserData: Boolean;
var
  Weights, Store, Question: String;
  WeightsBytes, StoreBytes: Int64;
begin
  Weights := ExpandConstant('{app}\python\weights');
  Store := RecordedDevelopStore;
  WeightsBytes := DirectoryBytes(Weights);
  StoreBytes := DirectoryBytes(Store);
  Log(Format('User data: model weights %d bytes at %s; develop store %d bytes at %s.', [
    WeightsBytes, Weights, StoreBytes, Store]));

  if (WeightsBytes = 0) and (StoreBytes = 0) then
  begin
    Log('Nothing to ask about: no downloaded weights and no develop store.');
    Result := False;
    Exit;
  end;

  if UninstallSilent then
  begin
    Result := DeleteDataSwitchGiven;
    if Result then
      Log('Silent uninstall with /DELETEDATA=1: deleting the weights and the develop store.')
    else
      Log('Silent uninstall without /DELETEDATA=1: keeping the weights and the develop store.');
    Exit;
  end;

  Question := Format(
    'Uninstalling %s leaves behind two things it never installed:' + #13#10#13#10 +
    'Downloaded model weights, %s' + #13#10 + '%s' + #13#10#13#10 +
    'Your develop store — edits, thumbnails and the style index, %s' + #13#10 + '%s' + #13#10#13#10 +
    'Delete these as well?' + #13#10#13#10 +
    'Choose No to keep them. That is what you want if you will install %s again: ' +
    'the weights are a large download, and the store holds your work.', [
    '{#AppName}', SizePhrase(WeightsBytes), Weights,
    SizePhrase(StoreBytes), Store, '{#AppName}']);
  Result := SuppressibleMsgBox(Question, mbConfirmation,
    MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES;
  if Result then
    Log('The user chose to delete the model weights and the develop store.')
  else
    Log('The user chose to keep the model weights and the develop store.');
end;

// The recorded store path came from the user's own AUTOSHADE_DATA_DIR, which
// src/store.rs takes at face value because that is how a portable setup names
// its root. DelTree does not ask questions, so a value naming a drive root,
// %LOCALAPPDATA% itself or the profile directory would take everything under
// it. Anything this uninstaller does not clearly own is logged and left alone.
function StoreIsOursToDelete(const Dir: String): Boolean;
var
  Normalized: String;
begin
  Normalized := RemoveBackslashUnlessRoot(Trim(Dir));
  Result := (Length(Normalized) > 3) and
            (CompareText(Normalized, ExpandConstant('{localappdata}')) <> 0) and
            (CompareText(Normalized, ExpandConstant('{userappdata}')) <> 0) and
            (CompareText(Normalized, ExpandConstant('{userdocs}')) <> 0) and
            (CompareText(Normalized, ExpandConstant('{app}')) <> 0);
end;

// Runs only after the installed files are gone. {app}\python is entirely
// installer-owned (sidecars, their __pycache__, the downloaded weights), so it
// goes whole; {app} itself is only ever asked to go if it is already empty,
// because that is the one directory a user could have put files of their own in.
procedure RemoveUserData;
var
  Weights, Store, App: String;
begin
  App := ExpandConstant('{app}');
  Weights := AddBackslash(App) + 'python\weights';
  Store := RecordedDevelopStore;

  if DirExists(Weights) then
  begin
    if DelTree(Weights, True, True, True) then
      Log('Deleted the downloaded model weights: ' + Weights)
    else
      Log('Could not delete the model weights: ' + Weights);
  end;
  DelTree(AddBackslash(App) + 'python', True, True, True);

  if DirExists(Store) then
  begin
    if not StoreIsOursToDelete(Store) then
      Log('Refusing to delete the recorded develop store, it is not a directory ' +
          'this install owns: ' + Store)
    else if DelTree(Store, True, True, True) then
      Log('Deleted the develop store: ' + Store)
    else
      Log('Could not delete the develop store: ' + Store);
  end;

  if RemoveDir(App) then
    Log('Removed the install directory: ' + App)
  else
    Log('Left the install directory in place, it is not empty: ' + App);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    if WizardIsTaskSelected('addtopath') then
      AddInstallDirToUserPath;
    RecordDevelopStore;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    RemoveInstallDirFromUserPath;
    DeleteUserData := AskAboutUserData;
  end
  else if CurUninstallStep = usPostUninstall then
  begin
    if DeleteUserData then
      RemoveUserData;
    ForgetInstallerState;
  end;
end;
