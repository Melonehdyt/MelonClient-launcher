# Melon Client Launcher

A free, open-source launcher for **Minecraft: Java Edition**. It installs
Minecraft for you, lets you sign in with your Microsoft account, and makes it
easy to add mods, modpacks, resource packs, and shaders — all from one app.

## Install

You do not need any programming tools to use the launcher. Every change is built
automatically on GitHub, so you only ever download the finished app.

> **Tip:** GitHub asks you to sign in to your (free) GitHub account before it
> will let you download files.

### Windows 10 / 11

1. At the top of this page, click **Actions**.
2. In the list on the left, click **Build launcher**, then open the newest run
   at the top (it will have a green tick).
3. Scroll to the bottom of that page to the **Artifacts** section.
4. Click **MelonClientLauncher-Windows x64** to download a `.zip` file.
5. Open the `.zip` and copy the contents somewhere, such as your Desktop.
6. Double-click **MelonClientLauncher_0.1.0_x64-setup.exe** (or the `.msi` file)
   and follow the installer.
7. Start **Melon Client Launcher** from your Start menu and sign in with the
   Microsoft account that owns Minecraft: Java Edition.

Windows 10/11 already includes the WebView2 component the launcher needs. If it
is somehow missing, install the Microsoft Edge WebView2 Runtime first.

### macOS

1. Under **Artifacts**, download **MelonClientLauncher-macOS Apple Silicon**
   (most Macs from 2020 on) or **MelonClientLauncher-macOS Intel** (older Macs).
2. Open the `.dmg` and drag the app into **Applications**. If there is no `.dmg`,
   move the `.app` out of the zip into Applications instead.
3. These builds are not signed. The first time you open it, **right-click the app
   and choose Open**, otherwise macOS will block it.

### Linux

1. Download **MelonClientLauncher-Linux x64** (most computers) or
   **MelonClientLauncher-Linux ARM64** (Raspberry Pi and similar).
2. Install the package that matches your system: `.deb` (Debian/Ubuntu), `.rpm`
   (Fedora), or run the `.AppImage`. For an AppImage, make it executable first
   (right-click → Properties → Permissions → "Allow executing file as program",
   or run `chmod +x` on it).

Do not install a build meant for a different operating system or CPU — for
example, do not put the ARM64 build on a normal PC.

## Getting started

1. Sign in with your Microsoft account.
2. On the **Play** page, pick a Minecraft version and a mod loader (Fabric,
   Quilt, Forge, or NeoForge), then press **Launch**. The launcher downloads
   whatever is needed the first time.
3. Add mods, modpacks, resource packs, or shaders from the **Mods & Packs** tab.

## Launching the game

Choose a Minecraft version and mod loader on the Play page, then press
**Launch**. The launcher resolves and installs the selected Minecraft/loader
version as needed. Forge and NeoForge are installed through their official
client installers. There is no separate Install button, and the launcher does
not bundle or automatically add a custom client mod.

Use the **Game Profiles** tab, or the quick selector on Play, to create and
switch between isolated instances. A new profile can pick its own Minecraft
version, mod loader, and profile image. Your existing Minecraft folder stays as
the **Default** profile; new profiles are created inside its `profiles/` folder
with their own game files and `mods/` folder. Each profile remembers its own
version and loader, and can use a custom PNG icon (up to 512×512 and 1 MB).
Selecting a profile in this tab opens its Mods view. Your existing files are
never moved or overwritten.

You can run several profiles at once. On the Play page, pick the running
instance from the dropdown before pressing **Stop selected**. If the game closes
with an error, the launcher opens a help dialog listing likely causes, what to
try next, and the most relevant recent log lines. The in-app console has a
switchable source (the launcher, or any running game) and hides your
user-folder name. Instances still running after you close the launcher are
picked up again the next time it starts.

In Settings you can make the launcher hide to the system tray once a game
starts. Closing the window while a game is running also hides it to the tray, so
the game and its log monitoring keep going. Use the tray menu to reopen or quit
the launcher. **Reset settings** restores your preferences while keeping your
profiles, accounts, and game files.

## Mods & Packs

The **Mods & Packs** tab lists the content in the active profile and shows
compatible Modrinth projects automatically when opened. Choose Mods, Modpacks,
Resource Packs, or Shaders to search and install straight into that profile's
`mods`, `resourcepacks`, or `shaderpacks` folder.

Modpacks use Modrinth's `.mrpack` format: the launcher downloads the files the
pack lists and unpacks its `overrides/` into the profile. If you already have a
`.mrpack` file, drag it onto the launcher window, or press **Import .mrpack** —
it is installed into the active profile.

Search text, category (for mods), and sort order are available as filters.
Mods for different loaders are not interchangeable, so pick the loader that
matches the mods you install.

Profiles other than **Default** can be deleted from the Game Profiles tab.
Deleting one permanently removes its managed folder, including its mods and game
files. The Default profile and your original Minecraft folder cannot be deleted
from the launcher.

## Skin library

The Profile tab can save skin files (64×64 or 64×32 PNG) locally and download
public skins by Minecraft username. Saved skins can be previewed with Classic or
Slim arms, and the character preview has Idle, Elytra/Flying, and Swimming
animations. The library is stored under `config/melonclient/skins/` inside your
selected Minecraft directory.

While signed in with a Microsoft account, selecting a saved skin uploads it as
that account's Minecraft skin; the launcher's face icons update to the new skin
right away. Without a Microsoft account, selecting a skin only changes the
preview.

## UI design

The interface is inspired by the classic **Bedrock Edition** look — the clean,
flat panels and simple controls that have become a familiar, almost classic
Minecraft style nowadays. Even though this launcher runs Java Edition, we
borrowed that Bedrock-flavoured design rather than the older Java GUI. One
portfolio we admired while building this was Titus "SnaveSutit"
([snavesutit.github.io](https://snavesutit.github.io/)), whose
Minecraft-flavoured interface is close to the look we were aiming for.

## Privacy and customization notes

Local file paths are blurred until you focus their input, and your user-folder
name is redacted from the in-app game log. The Background settings include a
live-preview transparency slider for launcher panels, a saved background-video
volume slider, a mute toggle, and a loop button. Loop works for local videos and
for YouTube video or playlist backgrounds.

Username skin downloads accept Mojang's legacy HTTP texture URLs only when they
use the official `textures.minecraft.net/texture/` path, and are upgraded to
HTTPS before downloading.

## Launcher news (for maintainers)

The Play page can show news below the Launch card. Edit
[`launcher/news.json`](launcher/news.json) in GitHub and publish that JSON at
`https://www.melonfr.dev/mod/news.json`. The response must be an array with up
to 20 entries:

```json
[
  {
    "title": "Launcher update",
    "date": "2026-10-18",
    "body": "News text shown in the launcher."
  }
]
```

Only plain text is shown. Titles are limited to 120 characters, bodies to 2,000
characters, and the whole response to 256 KB. If the feed is unavailable or
invalid, the launcher shows the error and a Refresh button. News can be turned
off in Settings without affecting anything else.

## Build from source (for developers)

The Tauri launcher project lives in [`launcher/`](launcher/). Java is downloaded
automatically when a compatible runtime is not found.

### Windows

Install [Node.js 22](https://nodejs.org/) (includes npm), [Rust with
rustup](https://rustup.rs/), and Visual Studio Build Tools with the **Desktop
development with C++** workload. Windows 10/11 normally has WebView2 already.

In PowerShell, from the repository root:

```powershell
Set-Location launcher
npm ci
npm run dev
```

`npm run dev` builds and opens the launcher for development. To produce
installers instead, run `npm run build`. The MSI and NSIS installers are written
to `launcher/src-tauri/target/release/bundle/`.

### macOS

Install Node.js 22, Rust with rustup, and the Xcode Command Line Tools:

```sh
xcode-select --install
```

Then, from the repository root:

```sh
cd launcher
npm ci
npm run dev
```

Run `npm run build` to create a `.app` bundle and, if generated, a `.dmg`, both
written to `launcher/src-tauri/target/release/bundle/`. Builds made this way are
unsigned unless you separately configure Apple Developer ID signing and
notarization, which is recommended before sharing with others.

### Linux

Running from source needs Node.js/npm, Rust/Cargo, and the GTK 3, WebKitGTK 4.1,
Ayatana AppIndicator, librsvg, OpenSSL, and compiler development packages.

On Bazzite or another immutable Fedora system, use Distrobox so those build
dependencies stay out of your host system:

```sh
distrobox create --name melonclient-dev --image registry.fedoraproject.org/fedora:44 --yes
distrobox enter melonclient-dev -- sudo dnf install -y nodejs npm rust cargo pkgconf-pkg-config gtk3-devel webkit2gtk4.1-devel libayatana-appindicator-gtk3-devel librsvg2-devel openssl-devel gcc gcc-c++ make
```

Then enter the container and build:

```sh
distrobox enter melonclient-dev
cd /path/to/MelonClient-launcher/launcher
npm ci
npm run dev
```

Replace `/path/to/MelonClient-launcher` with this repository's location as seen
inside the container (Distrobox shares your home directory). On a normal
distribution, install the equivalent packages with your package manager, then run
`npm ci` and `npm run dev` from `launcher/`.

## Continuous integration

The GitHub Actions workflow in
[`.github/workflows/build-launcher.yml`](.github/workflows/build-launcher.yml)
builds Windows x64, Intel and Apple Silicon macOS, and x86_64 and ARM64 Linux
packages on every push and pull request, and uploads them as workflow artifacts
(the **Install** section above explains how to download them).

macOS builds from that workflow are unsigned. They can be signed and notarized in
a separately configured release workflow. Keep signing certificates and
credentials in GitHub Actions secrets; never commit certificates, passwords,
account files, or local settings to the repository.

## License

MIT — see [LICENSE](LICENSE).
