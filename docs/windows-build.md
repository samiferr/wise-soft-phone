# Windows distribution guide

This guide covers building and installing Wise Softphone on **Windows 10 and Windows 11** (64-bit). Both versions run the same executable. Windows 11 on ARM runs it through the built-in x64 emulation.

## What you get

| File in `dist/` | Purpose |
| --- | --- |
| `wise-softphone-0.2.0-windows-x64-setup.exe` | Installer. Installs per user by default (no administrator rights); the wizard can install for all users. Adds a Start menu entry and an optional desktop shortcut, and an uninstaller. |
| `wise-softphone-0.2.0-windows-x64.zip` | Portable `softphone.exe` plus a short README. Unzip anywhere and run. |
| `SHA256SUMS-windows.txt` | SHA-256 checksums of the two files above. |

The executable links the C runtime statically, so no Visual C++ Redistributable is needed. It uses OpenGL (through the graphics driver), WASAPI audio, and the Windows Credential Manager for remembered passwords. There is no WebView2 or .NET dependency.

## Using the app on Windows

1. Run the installer (or `softphone.exe` from the zip). The installer is not code-signed, so Windows SmartScreen may show "Windows protected your PC": choose **More info → Run anyway**.
2. Allow microphone access: **Settings → Privacy & security → Microphone**, and turn on **Let desktop apps access your microphone**.
3. On first sign-in, Windows Defender Firewall may ask to allow the app. Allow it on the network type you use (private, and public if you connect over one) so SIP and voice traffic can pass.

Storage locations:

| Data | Location |
| --- | --- |
| Account, contacts, history | `%APPDATA%\com.softphone.desktop\` |
| Remembered password | Windows Credential Manager, service `com.softphone.desktop` |
| Diagnostic log | `%LOCALAPPDATA%\com.softphone.desktop\logs\softphone.log` (set `SOFTPHONE_LOG_DIR` to change it) |

If the window stays blank or the app reports an OpenGL error, install the graphics driver from your hardware vendor. Basic "Microsoft Basic Render Driver" sessions (some virtual machines and remote desktop sessions) do not provide the OpenGL support the interface needs.

## Rebuild from source

Build on Windows 10 or 11 (or a `windows-2022` CI runner):

1. Install Rust 1.95 or newer (<https://rustup.rs>) with the default MSVC toolchain.
2. Install **Visual Studio Build Tools** with the "Desktop development with C++" workload (provides the linker and `rc.exe`, used to embed the icon, version, and manifest).
3. Optional, for the installer: Inno Setup 6.3 or newer (`winget install JRSoftware.InnoSetup`).
4. From the repository root, in PowerShell:

```powershell
./windows/package-windows.ps1
```

The script reads the version from `Cargo.toml`, runs `cargo build --locked --release --target x86_64-pc-windows-msvc`, and writes the files above to `dist/`. Use `-SkipInstaller` to produce only the zip. For a plain developer build, `cargo run --locked` works on Windows as on Linux.

### Automated build

[`.github/workflows/windows.yml`](../.github/workflows/windows.yml) runs the same script on a `windows-2022` runner. Start it manually (**Actions → Windows distribution → Run workflow**) or push a tag such as `v0.2.0`. The files are uploaded as a workflow artifact; tag builds also attach them to a GitHub release.

### Cross-compiling check from Linux

Compilation can be checked without Windows (this does not produce the official MSVC build):

```sh
sudo apt install gcc-mingw-w64-x86-64
rustup target add x86_64-pc-windows-gnu
cargo check --locked --target x86_64-pc-windows-gnu
```

## Files involved

| File | Role |
| --- | --- |
| `build.rs` | Embeds icon, version info, and manifest into the Windows executable |
| `windows/softphone.manifest` | Declares Windows 10/11 support, per-monitor DPI awareness, long-path support |
| `windows/wise-softphone.ico` | Application icon (same artwork as the Android icon) |
| `windows/wise-softphone.iss` | Inno Setup installer script |
| `windows/package-windows.ps1` | Build and packaging script |
| `.cargo/config.toml` | Static C runtime for the Windows MSVC targets |

## Known limits

- The Windows build has been type-checked and cross-compiled on Linux. Running it, including audio devices, the Credential Manager, and the installer, still needs acceptance testing on real Windows 10 and Windows 11 machines.
- The installer and executable are not code-signed. Signing requires a code-signing certificate (`signtool sign` on the exe and setup file).
- The automated test suite (`cargo test`) has been run only on Linux.
