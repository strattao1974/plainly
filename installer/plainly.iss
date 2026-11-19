; Builds PlainlySetup.exe. Needs Inno Setup 6 (https://jrsoftware.org/isinfo.php) and a release build:
;   cargo build --release
;   iscc installer\plainly.iss

#define AppName "Plainly"
#define AppVersion "0.1.0"
#define AppExe "plainly.exe"

[Setup]
AppId={{6F3C2A1E-9B7D-4C1A-8E52-3D7F0B9A4C21}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=Plainly
AppComments=Your PC, explained in plain English.
; Per-user install: no admin prompt, nothing written outside the user's profile.
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=yes
OutputDir=Output
OutputBaseFilename=PlainlySetup
SetupIconFile=plainly.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0

[Tasks]
Name: "startup"; Description: "Start Plainly with Windows (recommended: it can only notice changes while it's running)"
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{userdesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Plainly"; ValueData: """{app}\{#AppExe}"" --background"; Tasks: startup
; The app can also turn this on itself, so always remove it on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Plainly"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#AppExe}"; Description: "Open Plainly now"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM {#AppExe} /F"; Flags: runhidden; RunOnceId: "StopPlainly"

[UninstallDelete]
Type: filesandordirs; Name: "{localappdata}\Plainly"

[Code]
// Plainly hides to the tray instead of closing, so stop it before files are replaced on upgrade.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM {#AppExe} /F', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Result := '';
end;
