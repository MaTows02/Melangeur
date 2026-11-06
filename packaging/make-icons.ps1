# Génère les icônes de Melangeur à partir d'un dessin simple : trois curseurs sur fond bleu.
param([string]$Out = (Join-Path $PSScriptRoot '..\assets'))
Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Force $Out | Out-Null

function New-IconPng([int]$Size) {
    $bitmap = New-Object Drawing.Bitmap $Size, $Size, ([Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [Drawing.Graphics]::FromImage($bitmap)
    $g.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([Drawing.Color]::Transparent)
    $radius = [single]($Size * 0.22); $diameter = $radius * 2; $edge = [single]($Size - 1)
    $path = New-Object Drawing.Drawing2D.GraphicsPath
    $path.AddArc(0, 0, $diameter, $diameter, 180, 90)
    $path.AddArc($edge - $diameter, 0, $diameter, $diameter, 270, 90)
    $path.AddArc($edge - $diameter, $edge - $diameter, $diameter, $diameter, 0, 90)
    $path.AddArc(0, $edge - $diameter, $diameter, $diameter, 90, 90)
    $path.CloseFigure()
    $background = New-Object Drawing.SolidBrush ([Drawing.Color]::FromArgb(255, 0, 120, 212))
    $g.FillPath($background, $path)
    $white = New-Object Drawing.SolidBrush ([Drawing.Color]::White)
    $track = [single][Math]::Max(1, $Size * 0.07)
    $knobWidth = [single]($Size * 0.17); $knobHeight = [single]($Size * 0.11)
    foreach ($slider in @(@(0.25, 0.62), @(0.50, 0.32), @(0.75, 0.50))) {
        $x = [single]($Size * $slider[0]); $y = [single]($Size * $slider[1])
        $g.FillRectangle($white, [single]($x - $track / 2), [single]($Size * 0.18), $track, [single]($Size * 0.64))
        $g.FillRectangle($white, [single]($x - $knobWidth / 2), [single]($y - $knobHeight / 2), $knobWidth, $knobHeight)
    }
    $g.Dispose()
    $stream = New-Object IO.MemoryStream
    $bitmap.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
    return ,$stream.ToArray()
}

# Fichier .ico : un en-tête, puis une image PNG par taille.
$sizes = 16, 24, 32, 48, 64, 128, 256
$images = foreach ($size in $sizes) { ,(New-IconPng $size) }
$icon = New-Object IO.MemoryStream
$writer = New-Object IO.BinaryWriter $icon
$writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $dimension = if ($sizes[$i] -ge 256) { 0 } else { $sizes[$i] }
    $writer.Write([byte]$dimension); $writer.Write([byte]$dimension); $writer.Write([byte]0); $writer.Write([byte]0)
    $writer.Write([uint16]1); $writer.Write([uint16]32)
    $writer.Write([uint32]$images[$i].Length); $writer.Write([uint32]$offset)
    $offset += $images[$i].Length
}
foreach ($image in $images) { $writer.Write([byte[]]$image) }
$writer.Flush()
[IO.File]::WriteAllBytes((Join-Path (Resolve-Path $Out) 'melangeur.ico'), $icon.ToArray())

# Icônes de l'extension.
$extension = Join-Path $PSScriptRoot '..\extension'
foreach ($size in 16, 48, 128) {
    [IO.File]::WriteAllBytes((Join-Path (Resolve-Path $extension) "icon$size.png"), (New-IconPng $size))
}
"Icônes générées."
