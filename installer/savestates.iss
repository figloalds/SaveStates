; SaveStates per-user installer for Inno Setup 6.3 or newer (no administrator rights needed).
;
; Build it with `python scripts/build.py --installer`: that builds the app, then runs
;   ISCC /DAppVersion=... /DRunnablesDir=... /DDistDir=... /DIconFile=... installer\savestates.iss
; The defines just below are only the fallbacks for compiling this file by hand
; (run `python scripts/build.py --what app` first so the runnables folder exists).
; The installer is not code-signed: Windows SmartScreen warns about it (see LEIA-ME.md).

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef RunnablesDir
  #define RunnablesDir SourcePath + "..\target\runnables\windows-x86_64"
#endif
#ifndef DistDir
  #define DistDir SourcePath + "..\dist"
#endif
#ifndef IconFile
  #define IconFile SourcePath + "..\assets\savestates.ico"
#endif

[Setup]
; Never change AppId: it is how a newer installer finds the version to update.
AppId={{757D21C4-E993-418A-842A-8C5658609C96}
AppName=SaveStates
AppVersion={#AppVersion}
VersionInfoVersion={#AppVersion}.0
DefaultDirName={localappdata}\Programs\SaveStates
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#DistDir}
OutputBaseFilename=SaveStates-Setup-{#AppVersion}
SetupIconFile={#IconFile}
UninstallDisplayName=SaveStates
UninstallDisplayIcon={app}\savestates.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ShowLanguageDialog=auto

[Languages]
; The first one is used when the PC's language is neither of the two.
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#RunnablesDir}\savestates.exe"; DestDir: "{app}"; Flags: ignoreversion
; Raven is linked into savestates.exe, so the only DLL is the Visual C++ runtime, kept next to the exe.
Source: "{#RunnablesDir}\vcruntime140.dll"; DestDir: "{app}"; Flags: ignoreversion
; Raven is Apache-2.0: its license and notice travel with the program.
Source: "{#RunnablesDir}\RAVEN-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RunnablesDir}\RAVEN-NOTICE.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\SaveStates"; Filename: "{app}\savestates.exe"
Name: "{autodesktop}\SaveStates"; Filename: "{app}\savestates.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\savestates.exe"; Description: "{cm:LaunchProgram,SaveStates}"; Flags: nowait postinstall skipifsilent
