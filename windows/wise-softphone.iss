; Inno Setup 6.3+ script for the Wise Softphone installer (Windows 10 / 11, x64).
; Build through windows/package-windows.ps1, which passes the version and paths.
#ifndef AppVersion
  #error Pass /DAppVersion=<version> (windows/package-windows.ps1 does this).
#endif
#ifndef ExePath
  #error Pass /DExePath=<path to softphone.exe>.
#endif

[Setup]
AppId={{6F1D2C4A-8B3E-4D57-9A61-5C0E7B2F9D84}
AppName=Wise Softphone
AppVersion={#AppVersion}
AppPublisher=Wise
DefaultDirName={autopf}\Wise Softphone
DefaultGroupName=Wise Softphone
DisableProgramGroupPage=yes
; Installs for the current user by default; the wizard offers an all-users install.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
; Windows 10 and Windows 11, 64-bit (also runs on Windows 11 on ARM through emulation).
MinVersion=10.0
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=wise-softphone.ico
UninstallDisplayIcon={app}\softphone.exe
OutputDir=..\dist
OutputBaseFilename=wise-softphone-{#AppVersion}-windows-x64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "{#ExePath}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Wise Softphone"; Filename: "{app}\softphone.exe"
Name: "{autodesktop}\Wise Softphone"; Filename: "{app}\softphone.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\softphone.exe"; Description: "Start Wise Softphone"; Flags: nowait postinstall skipifsilent
