; Inno Setup script for the Windows installer. Built by .github/workflows/build.yml:
;   iscc /DAppVersion=1.3.2 installer\greenline.iss
; Per-user install (no admin rights needed).
; The app was called Redline before 1.3. AppId is unchanged so this upgrades
; that install; [InstallDelete] and the deletekey entries clear its leftovers.

#ifndef AppVersion
  #define AppVersion "1.3.2"
#endif

[Setup]
AppId={{9CD48A8B-E8E2-417B-BF40-2016BFE2F448}
AppName=Greenline
AppVersion={#AppVersion}
AppVerName=Greenline {#AppVersion}
AppPublisher=Copeland Building Envelope Consulting, Inc.
AppPublisherURL=https://github.com/CopelandBEC/greenline
AppCopyright=Copyright (c) 2026 Copeland Building Envelope Consulting, Inc.
VersionInfoVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\Greenline
UsePreviousAppDir=no
DefaultGroupName=Greenline
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=Greenline-{#AppVersion}-setup
SetupIconFile=..\assets\greenline.ico
UninstallDisplayIcon={app}\greenline.exe
UninstallDisplayName=Greenline {#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes
CloseApplications=yes

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked
Name: "openwith"; Description: "Add Greenline to ""Open with"" for PDF files (does not change your default PDF app)"

[Files]
Source: "..\target\release\greenline.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\vendor\pdfium.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\vendor\PDFIUM-LICENSE"; DestDir: "{app}\licenses"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\LICENSE-MIT"; DestDir: "{app}\licenses"; DestName: "Greenline-LICENSE-MIT.txt"; Flags: ignoreversion
Source: "..\LICENSE-APACHE"; DestDir: "{app}\licenses"; DestName: "Greenline-LICENSE-APACHE.txt"; Flags: ignoreversion
Source: "..\THIRD-PARTY-LICENSES.html"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "..\assets\fonts\OFL.txt"; DestDir: "{app}\licenses"; DestName: "Montserrat-OFL.txt"; Flags: ignoreversion

[InstallDelete]
Type: filesandordirs; Name: "{localappdata}\Programs\Redline"
Type: files; Name: "{autoprograms}\Redline.lnk"
Type: files; Name: "{autodesktop}\Redline.lnk"

[Icons]
Name: "{autoprograms}\Greenline"; Filename: "{app}\greenline.exe"
Name: "{autodesktop}\Greenline"; Filename: "{app}\greenline.exe"; Tasks: desktopicon

[Registry]
; Redline's "Open with" entries.
Root: HKA; Subkey: "Software\Classes\Redline.PDF"; Flags: deletekey
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueName: "Redline.PDF"; Flags: deletevalue
Root: HKA; Subkey: "Software\Classes\Applications\redline.exe"; Flags: deletekey
; "Open with" registration only; the user's default PDF viewer is left alone.
Root: HKA; Subkey: "Software\Classes\Greenline.PDF"; ValueType: string; ValueData: "PDF document"; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Greenline.PDF\DefaultIcon"; ValueType: string; ValueData: "{app}\greenline.exe,0"; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Greenline.PDF\shell\open\command"; ValueType: string; ValueData: """{app}\greenline.exe"" ""%1"""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "Greenline.PDF"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\greenline.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Greenline"; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\greenline.exe\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\Applications\greenline.exe\shell\open\command"; ValueType: string; ValueData: """{app}\greenline.exe"" ""%1"""; Tasks: openwith

[Run]
Filename: "{app}\greenline.exe"; Description: "Launch Greenline"; Flags: nowait postinstall skipifsilent
