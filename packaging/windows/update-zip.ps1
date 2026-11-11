# Builds irchad-windows-x86_64.zip for the in-app updater (self_update):
# irchad.exe and a VERSION line, signed like packaging/sign.sh (zipsign,
# context = the asset name) and checked against packaging/update.pub.
# The installer stays the download for new users; this is only for updates.
#   $env:UPDATE_SIGNING_KEY = <base64 private key>
#   packaging\windows\update-zip.ps1 path\to\irchad.exe "0.1.25"   # or "nightly <sha>"
param(
  [Parameter(Mandatory)] [string] $Exe,
  [Parameter(Mandatory)] [string] $Version
)
$ErrorActionPreference = 'Stop'
if (-not $env:UPDATE_SIGNING_KEY) { throw "UPDATE_SIGNING_KEY secret missing" }
if (-not (Get-Command zipsign -ErrorAction SilentlyContinue)) {
  cargo install -q --locked zipsign@0.2.2
  if ($LASTEXITCODE) { exit 1 }
}

$zip = 'irchad-windows-x86_64.zip'
$tmp = Join-Path ([IO.Path]::GetTempPath()) "irchad-update-$PID"
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
  Copy-Item $Exe (Join-Path $tmp 'irchad.exe')
  [IO.File]::WriteAllText((Join-Path $tmp 'VERSION'), "$Version`n")
  Remove-Item $zip -ErrorAction SilentlyContinue
  # Windows' own bsdtar by full path: a Git for Windows GNU tar earlier on
  # PATH would quietly write a tar.
  & "$env:SystemRoot\System32\tar.exe" --format zip -cf $zip -C $tmp irchad.exe VERSION
  if ($LASTEXITCODE) { exit 1 }
  $key = Join-Path $tmp 'update.key'
  [IO.File]::WriteAllBytes($key, [Convert]::FromBase64String($env:UPDATE_SIGNING_KEY))
  zipsign sign zip $zip $key
  if ($LASTEXITCODE) { exit 1 }
} finally {
  Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
zipsign verify zip -q $zip (Join-Path $PSScriptRoot '..\update.pub')
if ($LASTEXITCODE) { exit 1 }
"signed $zip"
