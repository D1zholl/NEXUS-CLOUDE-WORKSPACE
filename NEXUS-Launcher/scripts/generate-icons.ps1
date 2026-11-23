Add-Type -AssemblyName System.Drawing

$iconDir = Join-Path $PSScriptRoot '..\src-tauri\icons'
$iconDir = [System.IO.Path]::GetFullPath($iconDir)
New-Item -ItemType Directory -Force -Path $iconDir | Out-Null

function New-NexusPng([int]$Size, [string]$Path) {
    $bitmap = New-Object System.Drawing.Bitmap($Size, $Size)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $graphics.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $graphics.Clear([System.Drawing.Color]::FromArgb(255, 8, 11, 18))

    $brush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 116, 176, 255))
    $fontSize = [Math]::Max(10, [int]($Size * 0.52))
    $font = New-Object System.Drawing.Font('Segoe UI', $fontSize, [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
    $format = New-Object System.Drawing.StringFormat
    $format.Alignment = [System.Drawing.StringAlignment]::Center
    $format.LineAlignment = [System.Drawing.StringAlignment]::Center
    $rect = New-Object System.Drawing.RectangleF(0, 0, $Size, $Size)
    $graphics.DrawString('N', $font, $brush, $rect, $format)

    $font.Dispose()
    $brush.Dispose()
    $graphics.Dispose()
    $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
}

New-NexusPng 32 (Join-Path $iconDir '32x32.png')
New-NexusPng 128 (Join-Path $iconDir '128x128.png')
New-NexusPng 256 (Join-Path $iconDir '128x128@2x.png')
New-NexusPng 256 (Join-Path $iconDir 'icon-256.png')

# Build a PNG-compressed ICO. Modern Windows accepts PNG payloads in ICO files.
$pngPath = Join-Path $iconDir 'icon-256.png'
$icoPath = Join-Path $iconDir 'icon.ico'
$png = [System.IO.File]::ReadAllBytes($pngPath)

$stream = [System.IO.File]::Create($icoPath)
$writer = New-Object System.IO.BinaryWriter($stream)
$writer.Write([UInt16]0)
$writer.Write([UInt16]1)
$writer.Write([UInt16]1)
$writer.Write([Byte]0)
$writer.Write([Byte]0)
$writer.Write([Byte]0)
$writer.Write([Byte]0)
$writer.Write([UInt16]1)
$writer.Write([UInt16]32)
$writer.Write([UInt32]$png.Length)
$writer.Write([UInt32]22)
$writer.Write($png)
$writer.Flush()
$writer.Dispose()
$stream.Dispose()

Remove-Item $pngPath -Force
Write-Output "Icons written to $iconDir"
