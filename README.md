# Melon Client Launcher

A cross-platform launcher for Minecraft Java Edition that sets up Minecraft with
Fabric, Quilt, Forge, or NeoForge and lets players choose their own mods. The
launcher supports Microsoft account sign-in, a Modrinth browser,
background customization, and a local skin library.

## Launching the game

Choose a Minecraft version and mod loader on the Play page, then press
**Launch**. The launcher resolves and installs the selected Minecraft/loader
version as needed. Forge and NeoForge are installed through their official
client installers. There is no separate Install button and the launcher does
not bundle or automatically add a custom client mod.

Use the **Game Profiles** tab or the quick selector on Play to create and switch
between isolated instances. New profiles can select their Minecraft version,
loader, and profile image when created. The existing Minecraft directory is kept as the **Default** profile;
new profiles are created beneath its `profiles/` folder, with separate game
files and `mods/` directories. Each profile remembers its own Minecraft version
and mod loader. Profiles use the Melon Client logo by default; a custom PNG icon
can be uploaded. Selecting a profile from this tab opens its Mods view, where
the installed mods for that profile are listed. Existing files are not moved or
overwritten.

The **Mods & Packs** tab lists content in the active profile and automatically
previews compatible Modrinth projects when opened. Choose Mods, Resource Packs,
or Shaders to search and install into that profile's `mods`, `resourcepacks`,
or `shaderpacks` folder. Search text, category (for mods), and sort order are
available as filters; results are cached during the launcher session. Mods for
different loaders are not interchangeable; select the loader that matches the
mods you install.

Profiles other than **Default** can be deleted from the Game Profiles tab.
Deleting a profile permanently removes its managed profile folder, including
its mods and game files. The Default profile and the original Minecraft
directory cannot be deleted from the launcher. Profiles use the Melon Client
logo by default and can use a custom PNG (up to 512×512 and 1 MB). The profile
creator is at the top of the tab, with saved profiles listed below it.

Multiple profiles can run at once. On the Play page, choose the running
instance from the dropdown before pressing **Stop selected**. If the game exits
unsuccessfully, the launcher opens a crash help dialog with likely causes,
suggested next steps, and recent relevant console errors. The console is
bounded and redacts user-directory names; its source dropdown switches between
launcher output and each game's output. Minecraft instances are restored on
launcher startup, including instances left running after the launcher exits.
Download/install progress is dismissed when setup finishes.

Settings can hide the launcher to the system tray after a game starts. Closing
the launcher window while a game is running also hides it to the tray so the
game and its console monitoring continue. Use the tray menu to reopen or quit
the launcher. **Reset settings** restores launcher preferences while preserving
profiles, accounts, and game files.

## Launcher news

The Play page displays news below the Launch card. Edit [`launcher/news.json`](launcher/news.json)
in GitHub and publish that JSON at
`https://www.melonfr.dev/mod/news.json`. The response must be an array
with up to 20 entries:

```json
[
  {
    "title": "Launcher update",
    "date": "2026-10-18",
    "body": "News text shown in the launcher."
  }
]
```

Only plain text is rendered. Titles are limited to 120 characters, bodies to
2,000 characters, and the response to 256 KB. If the feed is unavailable or
invalid, the launcher shows the error and provides a Refresh button. News can
be disabled from Settings without affecting the rest of the launcher.

## Local skin library

The Profile tab can save imported 64x64 or 64x32 PNG skin files locally and
download public skins by Minecraft username. Saved skins can be previewed with
Classic or Slim arms. The library is stored under
`config/melonclient/skins/` in the selected Minecraft directory. Selecting a
skin only changes the launcher preview; it does not change the Minecraft player
skin or upload anything to a Microsoft or Mojang account.
The character preview also has Elytra/Flying, Swimming, and Idle animations.

## Install a prebuilt launcher

Every push builds platform-specific packages in GitHub Actions. To download
them, open the repository's **Actions** tab, select a successful **Build
launcher** run, and download the artifact for your operating system. Extract
the downloaded artifact ZIP before installing.

- **Windows x64:** Run `MelonClientLauncher_0.1.0_x64-setup.exe` or open
  `MelonClientLauncher_0.1.0_x64_en-US.msi` and follow the installer.
- **macOS:** Open the `.dmg` and drag the app into Applications if a disk image
  is included. Otherwise, move the `.app` from the artifact into Applications.
  The automated macOS builds are unsigned and may be blocked by Gatekeeper;
  they are intended for testing until a trusted Developer ID signing and
  notarization setup is configured.
- **Linux:** Install the package for your distribution (`.deb`, `.rpm`, or
  `.AppImage`, depending on the artifact). AppImage files may need executable
  permission before they can be launched.

The Windows installer is also available from the local Windows build output
described below. Do not install packages built for a different operating
system or CPU architecture.

## Build on Windows

Install Node.js 22 (including npm), Rust using rustup, and Visual Studio Build
Tools with the **Desktop development with C++** workload. Windows 10/11
normally includes WebView2; install the Microsoft Edge WebView2 Runtime if it
is missing.

In PowerShell, from the repository root:

```powershell
Set-Location launcher
npm ci
npm run dev
```

`npm run dev` builds and opens the launcher for development. To create
installers instead, run:

```powershell
npm run build
```

The MSI and NSIS installers are written to
`launcher/src-tauri/target/release/bundle/`.

## Build on macOS

Install Node.js 22 (including npm), Rust using rustup, and the Xcode Command
Line Tools:

```sh
xcode-select --install
```

From the repository root, install dependencies and start the development
launcher:

```sh
cd launcher
npm ci
npm run dev
```

To create a macOS application bundle and disk image, run:

```sh
npm run build
```

The `.app` and any generated `.dmg` are written to
`launcher/src-tauri/target/release/bundle/`. Builds made this way are unsigned
unless you separately configure Apple Developer ID signing and notarization.
For distribution to other users, signed and notarized builds are recommended.

## Running on Linux

The launcher is a Tauri desktop application. Running it from source requires
Node.js/npm, Rust/Cargo, and the GTK 3, WebKitGTK 4.1, Ayatana AppIndicator,
librsvg, OpenSSL, and compiler development packages.

On Bazzite or another immutable Fedora system, use Distrobox so these build
dependencies stay out of the host operating system. From the repository root,
create a Fedora development container and install the prerequisites:

```sh
distrobox create --name melonclient-dev --image registry.fedoraproject.org/fedora:44 --yes
distrobox enter melonclient-dev -- sudo dnf install -y nodejs npm rust cargo pkgconf-pkg-config gtk3-devel webkit2gtk4.1-devel libayatana-appindicator-gtk3-devel librsvg2-devel openssl-devel gcc gcc-c++ make
```

Enter the container, change to this repository's `launcher/` directory, and
install the locked npm dependencies:

```sh
distrobox enter melonclient-dev
cd /path/to/MelonClient-launcher/launcher
npm ci
npm run dev
```

Replace `/path/to/MelonClient-launcher` with the checkout's location as seen
inside the container. Distrobox makes the home directory available there. The
last command builds and opens the desktop launcher; leave that terminal running
while using it. On a non-immutable distribution, install the equivalent
development packages with its package manager, then run `npm ci` and `npm run
dev` from `launcher/`.

## Building the launcher

The Tauri launcher project is in [`launcher/`](launcher/). Its frontend uses
the scripts declared in `launcher/package.json`; run `npm run build` from
`launcher/` on the target operating system to create its native packages. The
GitHub Actions workflow builds Windows x64, Intel and Apple Silicon macOS, and
x86_64 and ARM64 Linux packages and publishes them as workflow artifacts.
Java is downloaded automatically when a compatible runtime is not found.

macOS builds from the included workflow are unsigned. They can be signed and
notarized in a separately configured release workflow. Keep signing
certificates and credentials in GitHub Actions secrets; never commit
certificates, passwords, account files, or local settings to the repository.

Local filesystem paths in the launcher are visually blurred until their input
is focused. User-directory names are redacted from the in-launcher game log.
The Background settings include a live-preview transparency slider for launcher
content blocks such as the Launch card and profile panels, along with a saved
background-video volume slider, mute toggle, and loop button. Loop is supported
for local videos and YouTube video or playlist backgrounds.

Username skin downloads accept Mojang's legacy HTTP texture URLs only when they
use the official `textures.minecraft.net/texture/` path, then upgrade them to
HTTPS before downloading.

## License

MIT — see [LICENSE](LICENSE).
