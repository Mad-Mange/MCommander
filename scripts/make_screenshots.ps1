<#
.SYNOPSIS
  Regenerates the README screenshots in docs/images from a neutral demo tree.

.DESCRIPTION
  Creates demo folders under %TEMP%\mcommander-demo, maps them to a temporary
  drive letter with SUBST (so screenshots show short, anonymous paths), runs
  MCommander with scripted input (--keys) and removes the mapping again.
  Requires a release build: cargo build --release
#>
param([string]$Drive = "X:", [string]$Exe = "")

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$exe = if ($Exe) { $Exe } else { Join-Path $root "target\release\mcommander.exe" }
$out = Join-Path $root "docs\images"
if (-not (Test-Path $exe)) { throw "Build first: cargo build --release" }
New-Item -ItemType Directory -Force $out | Out-Null

$demo = Join-Path $env:TEMP "mcommander-demo"
if (Test-Path $demo) { Remove-Item -Recurse -Force $demo }
$p = Join-Path $demo "Projects"
$b = Join-Path $demo "Backup"
$dirs = "Apollo\src", "Billing-Service", "Docs\guides", "Game-Prototype\assets", "Photos-2026", "Scripts", "Website\css"
foreach ($d in $dirs) { New-Item -ItemType Directory -Force (Join-Path $p $d) | Out-Null }
foreach ($d in "Apollo", "Docs", "Photos") { New-Item -ItemType Directory -Force (Join-Path $b $d) | Out-Null }

function Write-Text($path, $text) { Set-Content -Path $path -Value $text -Encoding utf8 }
function Write-Bytes($path, $size) {
  $bytes = New-Object byte[] $size
  (New-Object System.Random 42).NextBytes($bytes)
  [IO.File]::WriteAllBytes($path, $bytes)
}
Write-Text "$p\README.md" "# Projects`n`nWorkspace for this year's projects.`n`n- Apollo: launch planning`n- Billing-Service: invoicing API`n- Website: public site`n"
Write-Text "$p\meeting-notes.txt" "Monday sync`n- ship the beta on Friday`n- review the budget`n- book the venue`n"
Write-Text "$p\release-notes.md" "## 1.4.0`n* Faster search`n* Dark mode`n"
Write-Text "$p\install.cmd" "@echo off`necho Installing...`n"
Write-Text "$p\Apollo\src\main.rs" "fn main() {`n    println!(""Hello, Apollo!"");`n}`n"
Write-Text "$p\Apollo\README.md" "# Apollo"
Write-Text "$p\Docs\guides\getting-started.md" "# Getting started"
Write-Bytes "$p\budget-2026.xlsx" 24576
Write-Bytes "$p\report-q3.pdf" 182733
Write-Bytes "$p\Game-Prototype\assets\level1.dat" 90000
Copy-Item (Join-Path $root "res\mcommander-256.png") "$p\design-mockup.png"
foreach ($i in 1..6) { Copy-Item (Join-Path $root "res\mcommander-256.png") ("$p\Photos-2026\IMG_{0}.png" -f (4100 + $i * 7)) }
Compress-Archive -Path "$p\Docs\*", "$p\README.md" -DestinationPath "$p\archive-2025.zip" -Force
Write-Text "$b\README.md" "# Projects (old)"
Write-Text "$b\old-notes.txt" "notes from last year"
Copy-Item "$p\meeting-notes.txt" "$b\meeting-notes.txt"
Copy-Item "$p\Apollo\README.md" "$b\Apollo\README.md"
Compress-Archive -Path "$p\Photos-2026\*" -DestinationPath "$b\photos.zip" -Force
# Give the files a believable spread of dates.
$i = 0
Get-ChildItem -Recurse $demo | Sort-Object FullName | ForEach-Object { $_.LastWriteTime = (Get-Date "2026-10-24 17:42").AddHours(-7 * $i); $i++ }

subst $Drive $demo | Out-Null
try {
  $ini = Join-Path $demo "shots.ini"
  function Shot($theme, $keys, $size = "1100x700") {
    Set-Content -Path $ini -Encoding utf8 -Value "[Configuration]`nTheme=$theme`nQuickSearch=0`nShowHidden=0`n[Left]`ntab0=$Drive\Projects`n[Right]`ntab0=$Drive\Backup`n"
    # Windows PowerShell does not quote arguments with spaces itself.
    $proc = Start-Process -FilePath $exe -ArgumentList @("""/I=$ini""", "--size=$size", """--keys=$keys""") -PassThru
    if (-not $proc.WaitForExit(60000)) { $proc.Kill(); throw "timeout: $keys" }
  }
  # 1. Light theme: selection (red) and cursor.
  Shot "light" "Wait:1500,End,Up,Up,Insert,Insert,Up,Up,Up,Up,Wait:400,Shot:$out\main-light.png,Exit"
  # 2. Dark theme with the quick view panel showing README.md.
  Shot "dark" "Wait:1500,End,Up,Up,Up,Up,Up,Ctrl+Q,Wait:800,Shot:$out\main-dark.png,Exit"
  # 3. Multi-rename tool on the photos.
  Shot "light" "Wait:1500,Down,Down,Down,Down,Down,Enter,Wait:400,Ctrl+A,Ctrl+M,Wait:500,Text:Holiday_[C:3],Wait:300,Shot:$out\multi-rename.png,Exit"
  # 4. Synchronize dirs (opened from the toolbar, Compare clicked).
  Shot "light" "Wait:1500,Alt+C,Wait:300,Down,Down,Enter,Wait:600,Click:128;210,Wait:300,Click:262;210,Wait:1200,Shot:$out\sync.png,Exit"
  Get-ChildItem $out | Select-Object Name, Length
}
finally {
  subst $Drive /D | Out-Null
}
