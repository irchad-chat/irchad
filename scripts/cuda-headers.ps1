# Fetches cuda.h (NVIDIA's cuda_cudart redist, 3.5 MB, hash-pinned) so the
# Windows build compiles NVENC into webrtc-sys (vendor/README.md). No CUDA
# toolkit needed. Prints the directory to use as CUDA_HOME.
#
#   $env:CUDA_HOME = powershell -ExecutionPolicy Bypass -File scripts\cuda-headers.ps1
#   (into target\cuda; pass a directory to put it elsewhere). Plain
#   `scripts\cuda-headers.ps1` works where local scripts are allowed
#   (`Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`, or CI's pwsh).
param([string]$Dest = "$PSScriptRoot\..\target\cuda")
$ErrorActionPreference = 'Stop'

$version = '12.9.79'
$sha256 = '179e9c43b0735ffe67207b3da556eb5a0c50f3047961882b7657d3b822d34ef8'
$name = "cuda_cudart-windows-x86_64-$version-archive"
$url = "https://developer.download.nvidia.com/compute/cuda/redist/cuda_cudart/windows-x86_64/$name.zip"

$home_ = Join-Path $Dest $name
if (-not (Test-Path "$home_\include\cuda.h")) {
  New-Item -ItemType Directory -Force $Dest | Out-Null
  $zip = Join-Path $Dest "$name.zip"
  Invoke-WebRequest -UseBasicParsing $url -OutFile $zip
  $got = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
  if ($got -ne $sha256) {
    Remove-Item $zip
    throw "cuda_cudart sha256 mismatch: $got"
  }
  Expand-Archive $zip -DestinationPath $Dest -Force
  Remove-Item $zip
}
(Resolve-Path $home_).Path
