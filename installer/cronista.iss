; Inno Setup script: turns dist\Cronista (the PyInstaller build) into Cronista-Setup-<version>.exe.
; Build: "%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe" installer\cronista.iss
;
; Per-user install under %LOCALAPPDATA%\Programs: no administrator rights and no UAC prompt.
; Uninstalling removes the app only; recordings (Videos\Cronista) and the history
; (%LOCALAPPDATA%\Cronista) stay, since they are the user's.

#define AppName "Cronista"
#define AppVersion "0.7.0"

[Setup]
AppId={{6F0B2C8E-3D7A-4E1F-9A5B-2C4D8E6F1A30}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=Ignacio Chiappero
DefaultDirName={localappdata}\Programs\{#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=Cronista-Setup-{#AppVersion}
SetupIconFile=..\assets\cronista.ico
UninstallDisplayIcon={app}\Cronista.exe
; Tells Windows to refresh its icons when done: after an update in place, the taskbar and the
; Start menu kept showing the icon the previous version had (from Windows' icon cache).
ChangesAssociations=yes
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"

[Files]
Source: "..\dist\Cronista\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\Cronista.exe"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\Cronista.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\Cronista.exe"; Description: "Open Cronista"; Flags: nowait postinstall skipifsilent
