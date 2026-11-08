# Orbit build script.
#   .\build.ps1                 debug build
#   .\build.ps1 -Release        optimized build + dist\Orbit-Setup.exe + dist\Orbit-portable.zip
#   .\build.ps1 -Cargo <args>   run any cargo command with the right environment (e.g. -Cargo check -p orbit-ring)
param([switch]$Release, [Parameter(ValueFromRemainingArguments = $true)][string[]]$Cargo)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

# Toolchain: rustup's x86_64-pc-windows-gnu (no Visual Studio needed). dlltool lives in self-contained\.
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (-not (Get-Command cargo -ErrorAction SilentlyContinue) -and -not (Test-Path "$cargoBin\cargo.exe")) {
    throw 'Rust not found. Install: https://rustup.rs  (rustup-init -y --default-host x86_64-pc-windows-gnu)'
}
$sc = Resolve-Path "$env:USERPROFILE\.rustup\toolchains\*\lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained" -ErrorAction SilentlyContinue | Select-Object -First 1
$env:PATH = "$cargoBin;$sc;$env:PATH"

if ($Cargo) { & cargo @Cargo; exit $LASTEXITCODE }

if (-not $Release) { & cargo build; exit $LASTEXITCODE }

& cargo build --release
if ($LASTEXITCODE) { exit $LASTEXITCODE }
New-Item -ItemType Directory -Force dist | Out-Null
# The exe is its own installer: run it from anywhere and it offers to install per-user.
Copy-Item target\release\orbit.exe dist\Orbit-Setup.exe -Force
Compress-Archive -Path target\release\orbit.exe -DestinationPath dist\Orbit-portable.zip -Force
Get-Item dist\* | Format-Table Name, @{n = 'KB'; e = { [math]::Round($_.Length / 1KB) } }
