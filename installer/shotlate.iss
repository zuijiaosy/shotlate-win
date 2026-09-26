; Shotlate installer (Inno Setup 6). Per-user, no administrator rights, no wizard pages to click through.
;   iscc /DAppVersion=0.1.0 /DArch=x64 /DSourceDir=dist\x64 installer\shotlate.iss
; WinSparkle runs it with /SILENT when updating; it closes the app first and starts it again at the end.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef Arch
  #define Arch "x64"
#endif
#ifndef SourceDir
  #define SourceDir "..\dist\" + Arch
#endif

[Setup]
AppId={{6C4A2F3E-8B7D-4E61-9D2A-5F1B3C7E9A40}
AppName=Shotlate
AppVersion={#AppVersion}
AppVerName=Shotlate {#AppVersion}
AppPublisher=Shotlate
AppPublisherURL=https://shotlate.pages.dev
AppSupportURL=https://github.com/zuijiaosy/shotlate-win
DefaultDirName={localappdata}\Programs\Shotlate
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
DisableWelcomePage=yes
PrivilegesRequired=lowest
OutputBaseFilename=Shotlate-{#AppVersion}-{#Arch}
OutputDir=..\dist
SetupIconFile=..\res\shotlate.ico
UninstallDisplayIcon={app}\Shotlate.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Windows 10 2004: needed for excluding windows from screen capture later (WDA_EXCLUDEFROMCAPTURE).
MinVersion=10.0.19041
CloseApplications=yes
RestartApplications=no
#if Arch == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif

[Files]
Source: "{#SourceDir}\Shotlate.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\WinSparkle.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{userprograms}\Shotlate"; Filename: "{app}\Shotlate.exe"
Name: "{userdesktop}\Shotlate"; Filename: "{app}\Shotlate.exe"

[Run]
; Interactive install: start it right away. Silent update from WinSparkle: start it again too.
Filename: "{app}\Shotlate.exe"; Flags: nowait postinstall skipifsilent; Description: "启动 Shotlate"
Filename: "{app}\Shotlate.exe"; Flags: nowait runasoriginaluser; Check: WizardSilent

[Registry]
; The "登录时启动" value the app writes; removed on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "Shotlate"; ValueType: none; Flags: uninsdeletevalue dontcreatekey

[UninstallDelete]
; The downloaded recognition models (about 23 MB). Settings in %APPDATA%\Shotlate stay.
Type: filesandordirs; Name: "{localappdata}\Shotlate\models"

[UninstallRun]
Filename: "{cmd}"; Parameters: "/C taskkill /IM Shotlate.exe /F"; Flags: runhidden; RunOnceId: "StopShotlate"
