; Inno Setup script for the Windows installer. Built by .github/workflows/build.yml:
;   iscc /DAppVersion=1.1.0 installer\redline.iss
; Per-user install (no admin rights needed).

#ifndef AppVersion
  #define AppVersion "1.1.0"
#endif

[Setup]
AppId={{9CD48A8B-E8E2-417B-BF40-2016BFE2F448}
AppName=Redline
AppVersion={#AppVersion}
AppVerName=Redline {#AppVersion}
AppPublisher=Copeland Building Envelope Consulting
VersionInfoVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\Redline
DefaultGroupName=Redline
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=Redline-{#AppVersion}-setup
SetupIconFile=..\assets\redline.ico
UninstallDisplayIcon={app}\redline.exe
UninstallDisplayName=Redline {#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes
CloseApplications=yes

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked
Name: "openwith"; Description: "Add Redline to ""Open with"" for PDF files (does not change your default PDF app)"

[Files]
Source: "..\target\release\redline.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\vendor\pdfium.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\vendor\PDFIUM-LICENSE"; DestDir: "{app}\licenses"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{autoprograms}\Redline"; Filename: "{app}\redline.exe"
Name: "{autodesktop}\Redline"; Filename: "{app}\redline.exe"; Tasks: desktopicon

[Registry]
; "Open with" registration only; the user's default PDF viewer is left alone.
Root: HKA; Subkey: "Software\Classes\Redline.PDF"; ValueType: string; ValueData: "PDF document"; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Redline.PDF\DefaultIcon"; ValueType: string; ValueData: "{app}\redline.exe,0"; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Redline.PDF\shell\open\command"; ValueType: string; ValueData: """{app}\redline.exe"" ""%1"""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "Redline.PDF"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\redline.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Redline"; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\redline.exe\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\redline.exe\shell\open\command"; ValueType: string; ValueData: """{app}\redline.exe"" ""%1"""; Tasks: openwith

[Run]
Filename: "{app}\redline.exe"; Description: "Launch Redline"; Flags: nowait postinstall skipifsilent
