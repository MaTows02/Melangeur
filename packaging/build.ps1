# Construit tout ce qui est distribué, dans dist\ : installateur, version portable, paquet pour les stores.
param([string]$Iscc = 'ISCC.exe')
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $root
$version = '1.0.0'
$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force $dist | Out-Null

cargo build --release
if ($LASTEXITCODE -ne 0) { throw 'La compilation a échoué.' }

# Version portable.
$portable = Join-Path $dist "Melangeur-$version-portable.zip"
Compress-Archive -Force -DestinationPath $portable -Path 'target\release\melangeur.exe', 'LICENSE.txt', 'PRIVACY.md', 'THIRD_PARTY_NOTICES.md'

# Paquet pour les stores : ils refusent le champ "key" et attribuent leur propre identifiant.
$staging = Join-Path $dist 'store'
Remove-Item -Recurse -Force $staging -ErrorAction SilentlyContinue
New-Item -ItemType Directory $staging | Out-Null
Copy-Item 'extension\*' $staging
$manifestPath = Join-Path $staging 'manifest.json'
$manifest = [IO.File]::ReadAllText($manifestPath) -replace '\s*"key":\s*"[^"]*",', ''
[IO.File]::WriteAllText($manifestPath, $manifest, (New-Object Text.UTF8Encoding($false)))
Compress-Archive -Force -DestinationPath (Join-Path $dist "Melangeur-extension-$version-store.zip") -Path (Join-Path $staging '*')
Remove-Item -Recurse -Force $staging

# Installateur.
& $Iscc /Q 'packaging\melangeur.iss'
if ($LASTEXITCODE -ne 0) { throw "La création de l'installateur a échoué." }

Get-ChildItem $dist | ForEach-Object { '{0}  {1:N0} Ko  SHA-256 {2}' -f $_.Name, ($_.Length / 1KB), (Get-FileHash $_.FullName).Hash }
