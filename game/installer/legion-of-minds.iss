; Inno Setup script for the Windows installer. Built by build-installer.ps1,
; which passes the version and the folder of files to install:
;   ISCC /DAppVersion=0.1.0 /DStage=..\dist\stage legion-of-minds.iss

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef Stage
  #define Stage "..\dist\stage"
#endif

[Setup]
AppId={{7C4E2B1A-5D3F-4A8B-9E21-6F0D3C9A7B42}
AppName=Legion of Minds
AppVersion={#AppVersion}
AppPublisher=Legion of Minds
DefaultDirName={localappdata}\Programs\Legion of Minds
DefaultGroupName=Legion of Minds
; Installs for the current user only, so no administrator prompt.
PrivilegesRequired=lowest
DisableProgramGroupPage=yes
OutputDir=..\dist
OutputBaseFilename=LegionOfMinds-{#AppVersion}-setup
SetupIconFile=icon.ico
UninstallDisplayIcon={app}\legion-of-minds.exe
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#Stage}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Legion of Minds"; Filename: "{app}\legion-of-minds.exe"
Name: "{group}\Uninstall Legion of Minds"; Filename: "{uninstallexe}"
Name: "{userdesktop}\Legion of Minds"; Filename: "{app}\legion-of-minds.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\legion-of-minds.exe"; Description: "{cm:LaunchProgram,Legion of Minds}"; Flags: nowait postinstall skipifsilent
