# Generates the Windows installer assets from their repo sources of truth:
#   backend/windows/jumbie.ico  <- frontend/icons/icon-512.png
#   backend/windows/LICENSE.rtf <- LICENSE
#
# The .ico is a multi-size ICO with PNG-compressed frames (valid since Vista)
# so transparency from the RGBA source PNG is preserved at every size. The exe
# itself embeds this .ico via embed-resource in backend/build.rs, and the MSI
# (backend/windows/jumbie.wxs) uses it for Add/Remove Programs.
#
# Run with Windows PowerShell (powershell.exe): System.Drawing is only
# available on .NET Framework, which pwsh 7 does not ship with.
$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Drawing

# ── jumbie.ico from frontend/icons/icon-512.png ─────────────────────────────
$src = [System.Drawing.Image]::FromFile((Resolve-Path "frontend\icons\icon-512.png").Path)
$sizes = @(16, 32, 48, 64, 256)

$entries = New-Object System.Collections.Generic.List[byte[]]
$images  = New-Object System.Collections.Generic.List[byte[]]

foreach ($s in $sizes) {
    $bmp = New-Object System.Drawing.Bitmap($src, $s, $s)
    $png = New-Object System.IO.MemoryStream
    $bmp.Save($png, [System.Drawing.Imaging.ImageFormat]::Png)
    $pngBytes = $png.ToArray()
    $bmp.Dispose()
    $png.Dispose()

    # ICONDIRENTRY: width, height, colors, reserved, planes, bitcount,
    # bytesInRes, imageOffset (offset patched below)
    if ($s -ge 256) { $w = 0 } else { $w = $s }
    $entry = New-Object System.IO.MemoryStream
    $ew = New-Object System.IO.BinaryWriter($entry)
    $ew.Write([Byte]$w)
    $ew.Write([Byte]$w)
    $ew.Write([Byte]0)
    $ew.Write([Byte]0)
    $ew.Write([UInt16]1)
    $ew.Write([UInt16]32)
    $ew.Write([UInt32]$pngBytes.Length)
    $ew.Write([UInt32]0)
    $entries.Add($entry.ToArray())
    $images.Add($pngBytes)
}

$outPath = "backend\windows\jumbie.ico"
$fs = [System.IO.File]::Create($outPath)
$out = New-Object System.IO.BinaryWriter($fs)

# ICONDIR header: reserved, type=icon, count
$out.Write([UInt16]0)
$out.Write([UInt16]1)
$out.Write([UInt16]$sizes.Count)

$offset = 6 + (16 * $sizes.Count)
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $entryBytes = $entries[$i]
    $entryBytes[12] = [Byte]($offset -band 0xFF)
    $entryBytes[13] = [Byte](($offset -shr 8) -band 0xFF)
    $entryBytes[14] = [Byte](($offset -shr 16) -band 0xFF)
    $entryBytes[15] = [Byte](($offset -shr 24) -band 0xFF)
    $out.Write($entryBytes)
    $offset += $images[$i].Length
}
foreach ($img in $images) { $out.Write($img) }

$out.Close()
$fs.Close()
$src.Dispose()

if (-not (Test-Path $outPath)) { throw "Failed to generate $outPath" }
Write-Output "Generated $outPath ($((Get-Item $outPath).Length) bytes)"

# ── LICENSE.rtf from LICENSE ───────────────────────────────────────────────
# The RTF shown on the WixUI license page is derived at build time — LICENSE
# stays the single source of truth; nothing generated is committed. Light
# Markdown (headers, Setext titles, rules, lists, bold/italic/code, links) is
# translated to RTF formatting. Non-ASCII characters (©, curly quotes, …) are
# emitted as \uN unicode escapes so \ansi output renders them correctly.
$lines = @(Get-Content -Encoding UTF8 LICENSE)
$lines[0] = $lines[0] -replace "^\uFEFF", ""   # strip UTF-8 BOM if present

# Fold Setext headers: a line of === (or ---) directly under a text line
# promotes that line to H1 (or H2) — "Title" + "====" -> "# Title".
$merged = New-Object System.Collections.Generic.List[string]
for ($i = 0; $i -lt $lines.Count; $i++) {
    $line = $lines[$i]
    if ($line -match '^\s*(=+|-+)\s*$') {
        if ($i -gt 0) {
            $prev = $lines[$i - 1]
            if ($prev.Trim().Length -gt 0 -and
                $prev -notmatch '^\s*#' -and
                $prev -notmatch '^\s*(=+|-+)\s*$') {
                $merged[$merged.Count - 1] = if ($line -match '^\s*=+') { "# $prev" } else { "## $prev" }
                continue
            }
        }
    }
    $merged.Add($line)
}
$lines = $merged

# Use StringBuilder for better performance with larger files
$sb = [System.Text.StringBuilder]::new()
$null = $sb.AppendLine('{\rtf1\ansi\deff0')
$null = $sb.AppendLine('{\fonttbl')
$null = $sb.AppendLine('{\f0\fswiss Arial;}')
$null = $sb.AppendLine('{\f1\fmodern Courier New;}')
$null = $sb.AppendLine('}')
$null = $sb.AppendLine('\f0\fs24')

function Escape-RtfText {
    param([string]$text)
    return $text.Replace('\', '\\').Replace('{', '\{').Replace('}', '\}')
}

function ConvertTo-InlineRtf {
    # Applies inline Markdown (bold/italic/code/links) to already-escaped
    # text. Shared by body, bullet, and header lines so every path formats
    # the same way (single source of truth for inline rules).
    param([string]$escaped)

    # Bold **text**
    $escaped = [regex]::Replace(
        $escaped,
        '\*\*(.+?)\*\*',
        '\b $1\b0'
    )

    # Italic *text* / _text_ (the (?<!)/(?!) [\*_] guards keep **bold** and
    # __underscore__ constructs out of this rule)
    $escaped = [regex]::Replace(
        $escaped,
        '(?<![\*_])([\*_])(?![\*_])(.+?)(?<![\*_])\1(?![\*_])',
        '\i $2\i0'
    )

    # Inline code `text`
    $escaped = [regex]::Replace(
        $escaped,
        '`([^`]+)`',
        '\f1 $1\f0'
    )

    # Links [text](url) -> text (url)
    $escaped = [regex]::Replace(
        $escaped,
        '\[([^\]]+)\]\(([^)]+)\)',
        '$1 ($2)'
    )

    return $escaped
}

function ConvertTo-RtfUnicode {
    # \ansi RTF is byte-based; every non-ASCII character is emitted as a
    # \uN escape (N = signed 16-bit code unit) with a '?' fallback glyph.
    param([string]$text)
    $out = [System.Text.StringBuilder]::new($text.Length)
    foreach ($ch in $text.ToCharArray()) {
        $code = [int]$ch
        if ($code -lt 128) {
            $null = $out.Append($ch)
        }
        else {
            if ($code -gt 0x7FFF) { $code -= 0x10000 }
            $null = $out.Append("\u$code?")
        }
    }
    return $out.ToString()
}

foreach ($line in $lines) {
    $escaped = Escape-RtfText $line

    # Horizontal rule: ---, ***, ___, ===
    if ($escaped -match '^\s*([-*_=])\1{2,}\s*$') {
        $null = $sb.AppendLine("\par\brdrb\brdrs\brdrw10\brsp20\par")
        continue
    }

    # Headers (#, ##, ###, ...)
    if ($escaped -match '^(#+)\s+(.*)$') {
        $level = $matches[1].Length
        switch ($level) {
            1 { $size = 40 } # 20 pt
            2 { $size = 34 } # 17 pt
            default { $size = 30 } # 15 pt
        }
        $null = $sb.AppendLine("\b\fs$size $(ConvertTo-InlineRtf $matches[2])\b0\fs24\par")
        continue
    }

    # Bullet list: - , * , +
    if ($escaped -match '^\s*[-*+]\s+(.*)$') {
        $escaped = "\bullet`t$(ConvertTo-InlineRtf $matches[1])"
    }
    else {
        $escaped = ConvertTo-InlineRtf $escaped
    }

    $null = $sb.AppendLine("$escaped\par")
}

$null = $sb.AppendLine('}')

# Ensure output directory exists
$rtfPath = "backend\windows\LICENSE.rtf"
$rtfDir = Split-Path $rtfPath -Parent
if (-not (Test-Path $rtfDir)) {
    New-Item -ItemType Directory -Path $rtfDir -Force | Out-Null
}

[System.IO.File]::WriteAllText(
    (Join-Path (Get-Location) $rtfPath),
    (ConvertTo-RtfUnicode $sb.ToString())
)

Write-Output "Generated $rtfPath ($((Get-Item $rtfPath).Length) bytes)"
