#define AppName "Resonance"
#define AppVersion "0.1.0"
#define AppPublisher "Resonance"
#define SourceRoot AddBackslash(SourcePath) + "..\..\"

[Setup]
AppId={{39B80C88-CF67-4A80-A029-B103C97F0B0B}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={localappdata}\Programs\Resonance
DefaultGroupName={#AppName}
UninstallDisplayIcon={app}\resonance.exe
PrivilegesRequired=lowest
ArchitecturesInstallIn64BitMode=x64
OutputDir={#SourceRoot}dist
OutputBaseFilename=Resonance-Setup-{#AppVersion}-x64
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Files]
Source: "{#SourceRoot}target\release\resonance.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Resonance"; Filename: "{app}\resonance.exe"
Name: "{autodesktop}\Resonance"; Filename: "{app}\resonance.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"
