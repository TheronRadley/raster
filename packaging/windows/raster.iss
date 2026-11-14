; Inno Setup 6. Built by build-installer.ps1 after the x64 Rust build.
#ifndef RasterVersion
  #error RasterVersion must be supplied by build-installer.ps1
#endif

#define AppName "Raster"
#define AppPublisher "Theron Radley"

[Setup]
AppId={{B708CC9D-C7A9-492D-9686-10C071C60E96}
AppName={#AppName}
AppVersion={#RasterVersion}
AppPublisher={#AppPublisher}
AppPublisherURL=https://github.com/TheronRadley/raster
AppSupportURL=https://github.com/TheronRadley/raster/issues
DefaultDirName={localappdata}\Programs\Raster
DefaultGroupName=Raster
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile=..\..\LICENSE
OutputDir=..\..\target\installer
OutputBaseFilename=Raster-Setup-{#RasterVersion}-x64
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\raster.exe
SetupLogging=yes

[Files]
Source: "..\..\target\x86_64-pc-windows-msvc\release\raster.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\docs\*"; DestDir: "{app}\docs"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Raster"; Filename: "{app}\raster.exe"; Parameters: "--shell"; WorkingDir: "{userdocs}"; Comment: "Open the Raster command prompt"
Name: "{group}\Raster Editor"; Filename: "{app}\raster.exe"; WorkingDir: "{userdocs}"; Comment: "Open the Raster terminal editor"
Name: "{group}\Uninstall Raster"; Filename: "{uninstallexe}"
