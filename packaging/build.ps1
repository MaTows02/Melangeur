# Construit tout ce qui est distribué, dans dist\ : installateur MSI, version portable, paquet pour les stores.
# -WixBin : dossier contenant candle.exe et light.exe (WiX Toolset 3.14).
param([Parameter(Mandatory)][string]$WixBin)
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $root
$version = '1.0.1'
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

# Licence au format RTF, exigé par l'écran de l'installateur.
$rtf = New-Object Text.StringBuilder
[void]$rtf.Append('{\rtf1\ansi\deff0{\fonttbl{\f0 Segoe UI;}}\fs18 ')
foreach ($character in [IO.File]::ReadAllText((Join-Path $root 'LICENSE.txt')).Replace("`r`n", "`n").ToCharArray()) {
    switch ($character) {
        "`n" { [void]$rtf.Append('\par ') }
        '\' { [void]$rtf.Append('\\') }
        '{' { [void]$rtf.Append('\{') }
        '}' { [void]$rtf.Append('\}') }
        default {
            if ([int]$character -lt 128) { [void]$rtf.Append($character) } else { [void]$rtf.Append("\u$([int]$character)?") }
        }
    }
}
[void]$rtf.Append('}')
[IO.File]::WriteAllText((Join-Path $dist 'license.rtf'), $rtf.ToString(), [Text.Encoding]::ASCII)

# Installateur.
$objects = Join-Path $dist 'obj'
New-Item -ItemType Directory -Force $objects | Out-Null
& (Join-Path $WixBin 'candle.exe') -nologo -arch x64 -ext WixUIExtension -out "$objects\" 'packaging\melangeur.wxs'
if ($LASTEXITCODE -ne 0) { throw "La compilation de l'installateur a échoué." }
& (Join-Path $WixBin 'light.exe') -nologo -ext WixUIExtension -cultures:fr-fr -spdb -out (Join-Path $dist "Melangeur-$version.msi") (Join-Path $objects 'melangeur.wixobj')
if ($LASTEXITCODE -ne 0) { throw "La création de l'installateur a échoué." }
Remove-Item -Recurse -Force $objects
Remove-Item -Force (Join-Path $dist 'license.rtf')

Get-ChildItem $dist -Filter "*$version*" | ForEach-Object { '{0}  {1:N0} Ko' -f $_.Name, ($_.Length / 1KB) }
