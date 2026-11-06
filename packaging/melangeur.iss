; Installateur de Melangeur, par utilisateur : aucun droit administrateur n'est demandé.
#define AppVersion "1.0.0"

[Setup]
AppId={{7E3B5A16-52C4-4E0B-9A63-3C1D7F2B8E41}
AppName=Melangeur
AppVersion={#AppVersion}
AppVerName=Melangeur {#AppVersion}
AppPublisher=MaTows
AppPublisherURL=https://github.com/MaTows02/Melangeur
DefaultDirName={localappdata}\Programs\Melangeur
DefaultGroupName=Melangeur
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
LicenseFile=..\LICENSE.txt
OutputDir=..\dist
OutputBaseFilename=Melangeur-Setup-{#AppVersion}
SetupIconFile=..\assets\melangeur.ico
UninstallDisplayIcon={app}\melangeur.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=no

[Languages]
Name: "french"; MessagesFile: "compiler:Languages\French.isl"

[Tasks]
Name: "autostart"; Description: "Lancer Melangeur au démarrage de Windows"

[Files]
Source: "..\target\release\melangeur.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\PRIVACY.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Melangeur"; Filename: "{app}\melangeur.exe"

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Melangeur"; ValueData: """{app}\melangeur.exe"""; Tasks: autostart

[Run]
Filename: "{app}\melangeur.exe"; Description: "Lancer Melangeur"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{app}\melangeur.exe"; Parameters: "--uninstall"; RunOnceId: "RemoveMelangeurData"; Flags: runhidden waituntilterminated

[UninstallDelete]
Type: filesandordirs; Name: "{localappdata}\Melangeur"
Type: files; Name: "{app}\melangeur.exe.old"

[Code]
// Une version déjà lancée verrouille le .exe : on la ferme avant de la remplacer.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
  Existing: String;
begin
  Result := '';
  Existing := ExpandConstant('{app}\melangeur.exe');
  if FileExists(Existing) then
  begin
    Exec(Existing, '--quit', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    // Le relais lancé par un navigateur ouvert peut encore verrouiller le fichier :
    // Windows permet de le renommer, ce qui libère son nom pour la nouvelle version.
    DeleteFile(Existing + '.old');
    RenameFile(Existing, Existing + '.old');
  end;
end;
