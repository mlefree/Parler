# Build Instructions

This guide covers Parler desktop builds. The customized 1.0.0 installation is validated on macOS; platform instructions inherited from Handy remain for development on Windows and Linux. See README.md for current features and offline fallback behavior.

## Prerequisites

### All Platforms

- [Rust](https://rustup.rs/) (latest stable)
- [Bun](https://bun.sh/) package manager
- [Tauri Prerequisites](https://tauri.app/start/prerequisites/)

### Platform-Specific Requirements

#### macOS

- Xcode Command Line Tools
- Install with: `xcode-select --install`

#### Windows

- Microsoft C++ Build Tools
- Visual Studio 2019/2022 with C++ development tools
- Or Visual Studio Build Tools 2019/2022

#### Linux

- Build essentials
- ALSA development libraries
- Install with:

  ```bash
  # Ubuntu/Debian
  sudo apt update
  sudo apt install build-essential libasound2-dev pkg-config libssl-dev libvulkan-dev vulkan-tools glslc libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libgtk-layer-shell0 libgtk-layer-shell-dev patchelf cmake

  # Fedora/RHEL
  sudo dnf groupinstall "Development Tools"
  sudo dnf install alsa-lib-devel pkgconf openssl-devel vulkan-devel \
    gtk3-devel webkit2gtk4.1-devel libappindicator-gtk3-devel librsvg2-devel \
    gtk-layer-shell gtk-layer-shell-devel \
    cmake

  # Arch Linux
  sudo pacman -S base-devel alsa-lib pkgconf openssl vulkan-devel \
    gtk3 webkit2gtk-4.1 libappindicator-gtk3 librsvg gtk-layer-shell \
    cmake
  ```

## Setup Instructions

### Local macOS installation and testing

Run `bun run deploy:local` to build, install and launch `/Applications/Parler.app`.
The script uses a persistent local code-signing identity instead of ad-hoc signing,
so subsequent builds keep the same macOS code identity. Only the installed app is
launched; the generated bundle is kept under the ignored `.local-build` directory
and renamed after deployment to avoid duplicate app entries.

On the first run, the script creates a local code-signing certificate in the login
keychain. macOS may ask you to authorize `codesign` to use that key; choose Always
Allow for this dedicated key if you want unattended builds. The certificate is
trusted for code signing only. Its private key is imported as non-extractable and
the temporary private-key files are removed. The public certificate is kept in
`~/Library/Application Support/Parler Build Signing/certificate.pem`.

The first transition from ad-hoc signing to this identity may require granting
Microphone and Accessibility once more. Keep the signing identity and app bundle
identifier unchanged afterward; do not reset permissions during normal updates.
Settings, API keys and models remain in the existing application data directory.
This local signing workflow is for this Mac, not notarized public distribution.

The local deployment strips executable symbols by default to reduce installed size.
Set `CARGO_PROFILE_RELEASE_STRIP=false` when a diagnostic build needs symbols.
Audio feedback playback and its Rodio dependency have been removed; WAV sound
themes are excluded from the bundle. General and advanced settings show the
core dictation controls, microphone, history and learning corpus. Legacy
settings remain readable for compatibility. Transcription engines and downloaded
models remain available for bilingual model comparisons; this cleanup does not
delete model files, history or learning recordings.

The identity can also be initialized separately with `bun run setup:local-signing`.
If the certificate exists but its keychain identity is missing, deployment stops
instead of silently generating a different identity.

### 1. Clone the Repository

```bash
git clone git@github.com:cjpais/Handy.git
cd Handy
```

### 2. Install Dependencies

```bash
bun install
```

### 3. Start Dev Server

```bash
bun tauri dev
```

### 4. Build for Production

```bash
bun run tauri build
```

This compiles a release binary and generates platform-specific bundles (deb, rpm, AppImage on Linux; dmg on macOS; msi on Windows).

## Linux Install (from source)

The raw binary (`src-tauri/target/release/handy`) cannot run standalone — it needs Tauri resource files (tray icons, sounds, VAD model) to be co-located at the expected path.

**Install from the deb bundle** (works on any Linux distro):

```bash
cd /tmp
ar x /path/to/Handy/src-tauri/target/release/bundle/deb/Handy_*_amd64.deb data.tar.gz
tar xzf data.tar.gz
sudo cp usr/bin/handy /usr/bin/
sudo cp -r usr/lib/Handy /usr/lib/
sudo cp -r usr/share/icons/hicolor/* /usr/share/icons/hicolor/
sudo cp usr/share/applications/Handy.desktop /usr/share/applications/
```

After subsequent rebuilds, only the binary needs re-copying:

```bash
sudo cp src-tauri/target/release/handy /usr/bin/
```

Resources only need re-copying if they change upstream (new icons, sounds, etc.).

## Troubleshooting

### AppImage build fails on Arch / rolling-release distros

`linuxdeploy` bundles its own `strip` binary which is too old to process system libraries built with newer toolchains on rolling-release distros (Arch, CachyOS, Manjaro, EndeavourOS).

The error from Tauri:

```
Bundling Handy_*_amd64.AppImage
failed to bundle project `failed to run linuxdeploy`
```

Tauri swallows the real linuxdeploy error. To see it, run linuxdeploy manually:

```bash
cd src-tauri/target/release/bundle/appimage
~/.cache/tauri/linuxdeploy-x86_64.AppImage --appimage-extract-and-run \
  --appdir Handy.AppDir --plugin gtk --output appimage
```

**Workaround:** The binary, deb, and rpm bundles all build fine — only the AppImage step fails. To skip it:

```bash
bun run tauri build -- --bundles deb
```

Then install using the deb extraction method above.
