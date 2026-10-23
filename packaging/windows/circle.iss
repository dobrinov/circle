; Inno Setup script for Circle. Driven by `cargo xtask dist`, which passes
; AppVersion, SourceDir, OutputDir, OutputName and Arch via /D.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{6C1E8B9A-3F4D-4E2B-9A7C-0C1C1E0B0001}
AppName=Circle
AppVersion={#AppVersion}
AppPublisher=Circle Browser
DefaultDirName={localappdata}\Programs\Circle
DefaultGroupName=Circle
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
SetupIconFile={#SourceDir}\circle.ico
UninstallDisplayIcon={app}\circle.ico
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
#if Arch == "aarch64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{autoprograms}\Circle"; Filename: "{app}\circle.exe"; IconFilename: "{app}\circle.ico"
Name: "{autodesktop}\Circle"; Filename: "{app}\circle.exe"; IconFilename: "{app}\circle.ico"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Registry]
; Register as a browser so it can be chosen in Settings > Default apps.
Root: HKCU; Subkey: "Software\Classes\CircleHTML"; ValueType: string; ValueData: "Circle HTML Document"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\CircleHTML\DefaultIcon"; ValueType: string; ValueData: "{app}\circle.ico"
Root: HKCU; Subkey: "Software\Classes\CircleHTML\shell\open\command"; ValueType: string; ValueData: """{app}\circle.exe"" ""%1"""
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\Circle"; ValueType: string; ValueData: "Circle"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\Circle\DefaultIcon"; ValueType: string; ValueData: "{app}\circle.ico"
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\Circle\shell\open\command"; ValueType: string; ValueData: """{app}\circle.exe"""
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\Circle\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "Circle"
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\Circle\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "A calm, sidebar-first web browser"
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\Circle\Capabilities"; ValueType: string; ValueName: "ApplicationIcon"; ValueData: "{app}\circle.ico"
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\Circle\Capabilities\FileAssociations"; ValueType: string; ValueName: ".html"; ValueData: "CircleHTML"
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\Circle\Capabilities\FileAssociations"; ValueType: string; ValueName: ".htm"; ValueData: "CircleHTML"
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\Circle\Capabilities\URLAssociations"; ValueType: string; ValueName: "http"; ValueData: "CircleHTML"
Root: HKCU; Subkey: "Software\Clients\StartMenuInternet\Circle\Capabilities\URLAssociations"; ValueType: string; ValueName: "https"; ValueData: "CircleHTML"
Root: HKCU; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "Circle"; ValueData: "Software\Clients\StartMenuInternet\Circle\Capabilities"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\circle.exe"; Description: "Launch Circle"; Flags: nowait postinstall skipifsilent
