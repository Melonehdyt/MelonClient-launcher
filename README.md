# Melon Client Launcher

A cross-platform launcher for Minecraft Java Edition that sets up Minecraft with
Fabric, Quilt, Forge, or NeoForge and lets players choose their own mods. The
launcher includes Microsoft and offline account management, a Modrinth browser,
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
launcher output and each game's output. Download/install progress is dismissed
when setup finishes.

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

## Building the launcher

The Tauri launcher project is in [`launcher/`](launcher/). Its frontend uses
the scripts declared in `launcher/package.json`; Windows and macOS installers
and Linux packages are built from `launcher/src-tauri/`.
The GitHub Actions workflow builds installers/packages on Windows, macOS, and
Linux and publishes them as workflow artifacts.

On Linux, install the Tauri system build dependencies before building: GTK 3,
WebKitGTK 4.1, Ayatana AppIndicator, librsvg, and the compiler/build tools.
The workflow builds x86_64 and ARM64 Linux/macOS variants and Windows x64.
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
