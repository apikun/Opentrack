# Builds an MSIX from a release build (Windows, Windows SDK required).
# Usage: .\make-msix.ps1 -BinDir target\x86_64-pc-windows-msvc\release -Out dist
param(
    [string]$BinDir = "target/release",
    [string]$Out = "dist"
)
$ErrorActionPreference = "Stop"

$stage = Join-Path $env:TEMP "opentrack-msix"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null

Copy-Item "$BinDir/opentrack.exe" $stage
Copy-Item "$BinDir/opentrack-app.exe" $stage

@"
<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
         xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10">
  <Identity Name="OpenTrack.OpenTrack" Version="0.4.0.0"
            Publisher="CN=OpenTrack contributors"/>
  <Properties>
    <DisplayName>OpenTrack</DisplayName>
    <PublisherDisplayName>OpenTrack contributors</PublisherDisplayName>
    <Logo>assets/logo.png</Logo>
  </Properties>
  <Dependencies>
    <TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.17763.0" MaxVersionTested="10.0.22621.0"/>
  </Dependencies>
  <Resources><Resource Language="en-us"/></Resources>
  <Applications>
    <Application Id="OpenTrackApp" Executable="opentrack-app.exe" EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements DisplayName="OpenTrack" Description="Clean-room Rust NLE"
                          BackgroundColor="#14141c" Square150x150Logo="assets/logo.png"
                          Square44x44Logo="assets/logo.png"/>
    </Application>
  </Applications>
</Package>
"@ | Set-Content (Join-Path $stage "AppxManifest.xml")

New-Item -ItemType Directory -Force (Join-Path $stage "assets") | Out-Null
# A 1-color placeholder logo; replace with a real asset before store upload.
Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap 256, 256
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::FromArgb(0x14, 0x14, 0x1c))
$g.Dispose()
$bmp.Save((Join-Path $stage "assets/logo.png"), [System.Drawing.Imaging.ImageFormat]::Png)

$dest = Join-Path $Out "opentrack-msix"
New-Item -ItemType Directory -Force $dest | Out-Null
& makeappx.exe pack /d $stage /p (Join-Path $dest "OpenTrack.msix") /o
if ($LASTEXITCODE -ne 0) { throw "makeappx failed" }
Write-Host "MSIX written to $dest (unsigned - sign with signtool + a code-signing identity)"
