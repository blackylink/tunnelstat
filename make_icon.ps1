# Генератор vpnstat.ico: рисует иконку в нескольких размерах и собирает ICO.
# Запуск: powershell -ExecutionPolicy Bypass -File make_icon.ps1
param([string]$Out = "vpnstat.ico")

Add-Type -AssemblyName System.Drawing

$sizes = @(256, 128, 64, 48, 32, 24, 16)

$bgTop    = [System.Drawing.Color]::FromArgb(0x22, 0x22, 0x2C)
$bgBottom = [System.Drawing.Color]::FromArgb(0x14, 0x16, 0x1C)
$dot      = [System.Drawing.Color]::FromArgb(0x3D, 0xD6, 0x8C)
$bar1     = [System.Drawing.Color]::FromArgb(0xE9, 0xEC, 0xF2)
$bar2     = [System.Drawing.Color]::FromArgb(0x7C, 0x86, 0x96)
$border   = [System.Drawing.Color]::FromArgb(0x3E, 0x4A, 0x60)

function New-IconBitmap([int]$S) {
    $bmp = [System.Drawing.Bitmap]::new($S, $S, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([System.Drawing.Color]::Transparent)

    # скруглённый прямоугольник с градиентом
    $r = [single]($S * 0.20)
    $d = [single]($S * 0.02)
    $path = [System.Drawing.Drawing2D.GraphicsPath]::new()
    $path.AddArc($d, $d, $r, $r, 180, 90)
    $path.AddArc($S - $d - $r, $d, $r, $r, 270, 90)
    $path.AddArc($S - $d - $r, $S - $d - $r, $r, $r, 0, 90)
    $path.AddArc($d, $S - $d - $r, $r, $r, 90, 90)
    $path.CloseFigure()

    $p0 = [System.Drawing.PointF]::new(0, 0)
    $p1 = [System.Drawing.PointF]::new(0, $S)
    $br = [System.Drawing.Drawing2D.LinearGradientBrush]::new($p0, $p1, $bgTop, $bgBottom)
    $g.FillPath($br, $path)
    $br.Dispose()

    $bw = [single][math]::Max(1.0, $S / 128.0)
    $pen = [System.Drawing.Pen]::new($border, $bw)
    $g.DrawPath($pen, $path)
    $pen.Dispose()
    $path.Dispose()

    # индикатор с гало
    $cx = [single]($S * 0.27)
    $cy = [single]($S * 0.50)
    $dr = [single]($S * 0.105)
    if ($S -ge 32) {
        for ($i = 3; $i -ge 1; $i--) {
            $gr = [single]($dr * (1 + 0.55 * $i))
            $a = [int](46 / $i)
            $gb = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb($a, $dot.R, $dot.G, $dot.B))
            $g.FillEllipse($gb, [single]($cx - $gr), [single]($cy - $gr), [single]($gr * 2), [single]($gr * 2))
            $gb.Dispose()
        }
    }
    $db = [System.Drawing.SolidBrush]::new($dot)
    $g.FillEllipse($db, [single]($cx - $dr), [single]($cy - $dr), [single]($dr * 2), [single]($dr * 2))
    $db.Dispose()

    # две полосы = строки текста
    $bx = [single]($S * 0.45)
    $h1 = [single][math]::Max(2.0, $S * 0.105)
    $b1 = [System.Drawing.SolidBrush]::new($bar1)
    $g.FillRectangle($b1, $bx, [single]($S * 0.375), [single]($S * 0.40), $h1)
    $b1.Dispose()

    $h2 = [single][math]::Max(1.5, $S * 0.075)
    $b2 = [System.Drawing.SolidBrush]::new($bar2)
    $g.FillRectangle($b2, $bx, [single]($S * 0.565), [single]($S * 0.26), $h2)
    $b2.Dispose()

    $g.Dispose()
    return $bmp
}

# ICO = ICONDIR + N*ICONDIRENTRY + данные (PNG внутри допустим с Vista)
$entries = @()
foreach ($S in $sizes) {
    $bmp = New-IconBitmap $S
    $ms = [System.IO.MemoryStream]::new()
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bytes = $ms.ToArray()
    $ms.Dispose()
    $bmp.Dispose()
    $entries += , @{ size = $S; data = $bytes }
}

$dir = [System.IO.MemoryStream]::new()
$bw2 = [System.IO.BinaryWriter]::new($dir)
$bw2.Write([uint16]0)
$bw2.Write([uint16]1)
$bw2.Write([uint16]$entries.Count)

$offset = 6 + 16 * $entries.Count
foreach ($e in $entries) {
    $w = 0; $h = 0
    if ($e.size -lt 256) { $w = $e.size; $h = $e.size }
    $bw2.Write([byte]$w)
    $bw2.Write([byte]$h)
    $bw2.Write([byte]0)
    $bw2.Write([byte]0)
    $bw2.Write([uint16]1)
    $bw2.Write([uint16]32)
    $bw2.Write([uint32]$e.data.Length)
    $bw2.Write([uint32]$offset)
    $offset += $e.data.Length
}
foreach ($e in $entries) { $bw2.Write($e.data) }
$bw2.Flush()
[System.IO.File]::WriteAllBytes($Out, $dir.ToArray())
$bw2.Dispose()
$dir.Dispose()
Write-Output ("wrote {0} ({1} bytes, {2} sizes)" -f $Out, (Get-Item $Out).Length, $entries.Count)
