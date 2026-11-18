# Draws the app icon (a blue folder, same style as the icon view) and writes assets\app.ico.
# Run from the repository root:  powershell -ExecutionPolicy Bypass -File packaging\make-icon.ps1
Add-Type -AssemblyName System.Drawing
$ErrorActionPreference = 'Stop'

function Rounded([System.Drawing.Graphics]$g, [System.Drawing.Brush]$b, [float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = 2 * $r
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    $g.FillPath($b, $p)
}

function Frame([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.Clear([System.Drawing.Color]::Transparent)
    $u = $size / 16.0
    $back = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 0x5A, 0xA8, 0xEC))
    $front = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 0x84, 0xC1, 0xF5))
    Rounded $g $back (1.5 * $u) (3.0 * $u) (5.5 * $u) (2.5 * $u) ([Math]::Max(0.5, 1.0 * $u))
    Rounded $g $back (1.5 * $u) (4.0 * $u) (13.0 * $u) (9.5 * $u) ([Math]::Max(0.5, 1.2 * $u))
    Rounded $g $front (1.5 * $u) (5.5 * $u) (13.0 * $u) (8.0 * $u) ([Math]::Max(0.5, 1.2 * $u))
    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    if ($size -ge 256) {
        # PNG for the 256 px frame (the format Windows expects there).
        $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    } else {
        # Classic 32-bit DIB for small frames: header, bottom-up BGRA rows, then an all-zero AND mask.
        $bw = New-Object System.IO.BinaryWriter $ms
        $bw.Write([uint32]40); $bw.Write([int32]$size); $bw.Write([int32]($size * 2))
        $bw.Write([uint16]1); $bw.Write([uint16]32); $bw.Write([uint32]0)
        $bw.Write([uint32]0); $bw.Write([int32]0); $bw.Write([int32]0); $bw.Write([uint32]0); $bw.Write([uint32]0)
        for ($y = $size - 1; $y -ge 0; $y--) {
            for ($x = 0; $x -lt $size; $x++) {
                $c = $bmp.GetPixel($x, $y)
                $bw.Write([byte]$c.B); $bw.Write([byte]$c.G); $bw.Write([byte]$c.R); $bw.Write([byte]$c.A)
            }
        }
        $maskRow = [int]([Math]::Ceiling($size / 32.0) * 4)
        $bw.Write((New-Object byte[] ($maskRow * $size)))
        $bw.Flush()
    }
    $bmp.Dispose()
    , $ms.ToArray()
}

$sizes = 16, 24, 32, 48, 64, 128, 256
$frames = $sizes | ForEach-Object { , (Frame $_) }
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $out
# ICONDIR: reserved, type 1 (icon), image count.
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i]; $len = $frames[$i].Length
    # ICONDIRENTRY: width/height (0 means 256), colors, reserved, planes, bpp, bytes, offset.
    $w.Write([byte]($s % 256)); $w.Write([byte]($s % 256)); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]$len); $w.Write([uint32]$offset)
    $offset += $len
}
foreach ($f in $frames) { $w.Write($f) }
$w.Flush()
New-Item -ItemType Directory -Force assets | Out-Null
[System.IO.File]::WriteAllBytes((Join-Path (Get-Location) 'assets\app.ico'), $out.ToArray())
"wrote assets\app.ico ($($out.Length) bytes, sizes $($sizes -join ', '))"
