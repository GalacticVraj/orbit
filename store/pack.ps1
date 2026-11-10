# Builds dist\Orbit.msix for the Microsoft Store.
#   .\store\pack.ps1                                   test package (identity GalacticVraj.Orbit / CN=GalacticVraj)
#   .\store\pack.ps1 -Name <Package/Identity/Name> -Publisher <Package/Identity/Publisher> -PublisherName <display name> -DisplayName <reserved app name>
#                                                      Store package: copy the values from Partner Center >
#                                                      your app > Product identity
#   .\store\pack.ps1 -Register                         also install it loose for testing (needs Developer Mode)
# The Store signs the package itself; nothing here needs a certificate.
param(
    [string]$Name = "GalacticVraj.Orbit",
    [string]$Publisher = "CN=GalacticVraj",
    [string]$PublisherName = "GalacticVraj",
    [string]$DisplayName = "Orbit",
    [string]$Version = "1.0.0.0",
    [switch]$Register
)
$ErrorActionPreference = "Continue"   # cargo writes progress to stderr; failures are checked through $LASTEXITCODE
$root = Split-Path $PSScriptRoot -Parent
$tools = Join-Path $PSScriptRoot ".tools"
$layout = Join-Path $root "dist\msix"
$out = Join-Path $root "dist\Orbit.msix"

# makeappx / makepri come from Microsoft's own Windows SDK build-tools NuGet package (no SDK install, no admin)
$makeappx = Get-ChildItem $tools -Recurse -Filter makeappx.exe -ErrorAction SilentlyContinue | ? { $_.FullName -match '\\x64\\' } | Select -First 1
if (-not $makeappx) {
    New-Item -ItemType Directory -Force $tools | Out-Null
    $v = (Invoke-RestMethod "https://api.nuget.org/v3-flatcontainer/microsoft.windows.sdk.buildtools/index.json").versions | ? { $_ -notmatch '-' } | Select -Last 1
    Invoke-WebRequest "https://api.nuget.org/v3-flatcontainer/microsoft.windows.sdk.buildtools/$v/microsoft.windows.sdk.buildtools.$v.nupkg" -OutFile "$tools\bt.zip"
    Expand-Archive "$tools\bt.zip" "$tools\bt" -Force; Remove-Item "$tools\bt.zip"
    $makeappx = Get-ChildItem $tools -Recurse -Filter makeappx.exe | ? { $_.FullName -match '\\x64\\' } | Select -First 1
}
$makepri = Join-Path $makeappx.DirectoryName "makepri.exe"

# 1. release build (and the Store logos, drawn by the same code as the app icon)
& "$root\build.ps1" -Cargo build --release | Out-Null
if ($LASTEXITCODE) { throw "build failed" }
if (Test-Path $layout) { Remove-Item $layout -Recurse -Force }
& "$root\build.ps1" -Cargo run --release -p orbit-setup --example store_assets -- "$layout\Assets" | Out-Null
if ($LASTEXITCODE) { throw "logos failed" }

# 2. layout: the exe and the manifest
Copy-Item "$root\target\release\orbit.exe" $layout
$x = { param($s) [Security.SecurityElement]::Escape($s) }
(Get-Content "$PSScriptRoot\AppxManifest.xml" -Raw).Replace('$NAME$', $Name).Replace('$PUBLISHER$', (& $x $Publisher)).Replace('$PUBLISHERNAME$', (& $x $PublisherName)).Replace('$DISPLAYNAME$', (& $x $DisplayName)).Replace('$VERSION$', $Version) |
    Set-Content "$layout\AppxManifest.xml" -Encoding UTF8

# 3. resources.pri (so Windows picks the right logo size), then the package
Push-Location $layout
& $makepri createconfig /cf "..\priconfig.xml" /dq en-US /pv 10.0.0 /o | Out-Null
& $makepri new /pr . /cf "..\priconfig.xml" /mn AppxManifest.xml /of resources.pri /o | Out-Null
Pop-Location
if ($LASTEXITCODE) { throw "makepri failed" }
& $makeappx.FullName pack /d $layout /p $out /o | Select-Object -Last 1
if ($LASTEXITCODE) { throw "makeappx failed" }
"{0}  ({1:N0} KB, identity {2})" -f $out, ((Get-Item $out).Length / 1KB), $Name

if ($Register) {
    Get-AppxPackage -Name $Name | Remove-AppxPackage
    Add-AppxPackage -Register "$layout\AppxManifest.xml"
    "registered: launch Orbit from Start"
}

