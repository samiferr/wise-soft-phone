<#
.SYNOPSIS
  Builds the Windows 10 / 11 distribution of Wise Softphone.

.DESCRIPTION
  Produces, in dist/:
    wise-softphone-<version>-windows-x64.zip          portable executable
    wise-softphone-<version>-windows-x64-setup.exe    installer (needs Inno Setup 6.3+)
    SHA256SUMS-windows.txt                            checksums for both files

  Requires Rust 1.95+ with the MSVC toolchain (Visual Studio Build Tools with the
  "Desktop development with C++" workload, which supplies the linker and rc.exe).
  Run from any directory in PowerShell 5.1 or PowerShell 7:  ./windows/package-windows.ps1
#>
[CmdletBinding()]
param(
    [string]$Target = 'x86_64-pc-windows-msvc',
    [switch]$SkipInstaller
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $version) { throw 'Could not read the version from Cargo.toml.' }

& rustup target add $Target
if ($LASTEXITCODE -ne 0) { throw 'rustup target add failed.' }
& cargo build --locked --release --target $Target
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed.' }

$exe = Join-Path $root "target/$Target/release/softphone.exe"
if (-not (Test-Path $exe)) { throw "Build output not found: $exe" }

$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$arch = if ($Target -like 'aarch64*') { 'arm64' } else { 'x64' }

# Portable zip: the executable plus a short note.
$stage = Join-Path $root "target/windows-stage"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item $exe (Join-Path $stage 'softphone.exe')
@"
Wise Softphone $version for Windows 10 and 11 ($arch)

Run softphone.exe. No installation or administrator rights are needed.
Allow microphone access for desktop apps under Settings > Privacy & security > Microphone.
If Windows Defender Firewall asks, allow the app on private networks (and public
networks if you connect over one) so SIP and voice traffic can pass.
Logs: %LOCALAPPDATA%\com.softphone.desktop\logs
"@ | Set-Content -Encoding UTF8 (Join-Path $stage 'README.txt')

$zip = Join-Path $dist "wise-softphone-$version-windows-$arch.zip"
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
$outputs = @($zip)

if (-not $SkipInstaller) {
    $iscc = (Get-Command iscc.exe -ErrorAction SilentlyContinue).Source
    if (-not $iscc) {
        $iscc = @(
            "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
            "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
            "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
        ) | Where-Object { Test-Path $_ } | Select-Object -First 1
    }
    if (-not $iscc) {
        Write-Warning 'Inno Setup 6 not found; skipping the installer. Install it (winget install JRSoftware.InnoSetup) or pass -SkipInstaller.'
    }
    elseif ($arch -ne 'x64') {
        Write-Warning 'The installer script targets x64 only; skipping the installer.'
    }
    else {
        & $iscc "/DAppVersion=$version" "/DExePath=$exe" (Join-Path $root 'windows/wise-softphone.iss')
        if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed.' }
        $outputs += Join-Path $dist "wise-softphone-$version-windows-$arch-setup.exe"
    }
}

$sums = foreach ($file in $outputs) {
    $hash = (Get-FileHash -Algorithm SHA256 $file).Hash.ToLower()
    "$hash  $(Split-Path -Leaf $file)"
}
$sums | Set-Content -Encoding ASCII (Join-Path $dist 'SHA256SUMS-windows.txt')
$outputs | ForEach-Object { Write-Host $_ }
