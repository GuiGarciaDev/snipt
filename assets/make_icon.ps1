# Draws assets/snipt.ico: a cyan (#34D8EA) rounded tile with a dark "S".
# Every size is rendered separately (not scaled down), so small icons stay
# crisp. Run from anywhere:  powershell -ExecutionPolicy Bypass -File assets\make_icon.ps1
Add-Type -AssemblyName System.Drawing

$sizes = 16, 20, 24, 32, 40, 48, 64, 128, 256
$top = [System.Drawing.Color]::FromArgb(255, 72, 226, 242)    # a touch lighter than #34D8EA
$bottom = [System.Drawing.Color]::FromArgb(255, 34, 200, 222) # a touch deeper
$ink = [System.Drawing.Color]::FromArgb(255, 6, 26, 30)       # same as ON_ACCENT in the app
$family = if ((New-Object System.Drawing.Text.InstalledFontCollection).Families.Name -contains 'Segoe UI Variable Display') { 'Segoe UI Variable Display' } else { 'Segoe UI' }

function RoundRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
  $p = New-Object System.Drawing.Drawing2D.GraphicsPath
  $d = 2 * $r
  $p.AddArc($x, $y, $d, $d, 180, 90); $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
  $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90); $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
  $p.CloseFigure(); $p
}

$images = foreach ($n in $sizes) {
  $bmp = New-Object System.Drawing.Bitmap $n, $n, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode = 'AntiAlias'; $g.PixelOffsetMode = 'HighQuality'; $g.Clear([System.Drawing.Color]::Transparent)

  # Tile: tiny inset at small sizes so the edge stays sharp, ~22% corner radius.
  $inset = if ($n -le 24) { 0 } else { $n * 0.04 }
  $side = $n - 2 * $inset
  $tile = RoundRect $inset $inset $side $side ($side * 0.22)
  $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush ([System.Drawing.RectangleF]::new(0, $inset, $n, $side)), $top, $bottom, 90
  $g.FillPath($brush, $tile)

  # "S", centred on its actual outline rather than the font's line box.
  $path = New-Object System.Drawing.Drawing2D.GraphicsPath
  $em = $side * $(if ($n -le 20) { 0.86 } else { 0.78 })
  $path.AddString('S', (New-Object System.Drawing.FontFamily $family), [int][System.Drawing.FontStyle]::Bold, $em, [System.Drawing.PointF]::new(0, 0), [System.Drawing.StringFormat]::GenericTypographic)
  $b = $path.GetBounds()
  $m = New-Object System.Drawing.Drawing2D.Matrix
  $m.Translate(($n - $b.Width) / 2 - $b.X, ($n - $b.Height) / 2 - $b.Y)
  $path.Transform($m)
  $g.FillPath((New-Object System.Drawing.SolidBrush $ink), $path)
  $g.Dispose()

  $ms = New-Object System.IO.MemoryStream
  $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
  [pscustomobject]@{ Size = $n; Png = $ms.ToArray() }
}

# ICO container with PNG-compressed images (supported since Windows Vista).
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $out
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$images.Count)
$offset = 6 + 16 * $images.Count
foreach ($i in $images) {
  $dim = if ($i.Size -ge 256) { 0 } else { $i.Size }
  $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
  $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]$i.Png.Length); $w.Write([uint32]$offset)
  $offset += $i.Png.Length
}
foreach ($i in $images) { $w.Write($i.Png) }
$w.Flush()
$dest = Join-Path $PSScriptRoot 'snipt.ico'
[System.IO.File]::WriteAllBytes($dest, $out.ToArray())
"wrote $dest ($([math]::Round($out.Length / 1KB, 1)) KB, sizes: $($sizes -join ', '))"
