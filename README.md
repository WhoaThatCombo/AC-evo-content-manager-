# Assetto Corsa EVO Content Manager

ACECM is a desktop manager for [Assetto Corsa EVO](https://store.steampowered.com/app/3058630): install cars and tracks, convert cars and tracks from the original Assetto Corsa, run a dedicated server, and join it from Drive. It is built for the same job Content Manager does for the original Assetto Corsa — content, hosting, and getting you on track without fighting the stock launcher.

Runs on Windows 10/11 with Edge WebView2 (included with Windows 11). The
latest version is on the [Releases](https://github.com/WhoaThatCombo/AC-evo-content-manager-/releases/latest)
page. Linux is on an older build for now - see [Linux](#linux).

## What it is

AC EVO does not expose a usable direct-connect. Joining a session goes through the game’s online lobby. ACECM runs that lobby on your machine (proxying Kunos so your account, garage, and public list stay real), installs mods where the dedicated server can actually load them, and launches the game with the session you picked.

It does **not** ship the game, the dedicated server, or Kunos content. You already own those on Steam.

## Install

1. Download **[ACECM.zip](https://github.com/WhoaThatCombo/AC-evo-content-manager-/releases/latest)**
   from the latest release and unzip it (or take `ACECM.exe` directly - some
   browsers warn about a bare `.exe`, the zip avoids that).
2. Run `ACECM.exe`. The first run offers to install it to
   `%LOCALAPPDATA%\Programs\ACECM` with Start Menu and Desktop shortcuts. Say
   no to just run it from where it is.
3. On first launch it locates the Steam game folder and `AssettoCorsaEVOServer.exe`. If either is missed, set it under **Settings**.

ACECM is not code-signed, so Windows SmartScreen shows an "unknown publisher"
warning. Each release lists the file's SHA-256 so you can check what you
downloaded. ACECM updates itself from inside the app; every update is checked
against a signature made with the project's release key before it is
installed.

Settings, server profiles, logs, and thumbnails live in `%LOCALAPPDATA%\ACECM`
(`~/.local/share/ACECM` on Linux). Override the data folder with the
`ACECM_DATA` environment variable if you need a clean profile.

### Linux

> **Linux is paused for now.** Windows moved to a rewritten native app in
> 1.0.22. The Linux build is still the older Python app, last released as
> [1.0.21](https://github.com/WhoaThatCombo/AC-evo-content-manager-/releases/tag/v1.0.21),
> and does not have the fixes since. Everything below describes that build.

Download **ACECM** (no extension) from the 1.0.21 release, make it executable,
run it. It is one self-contained file — no Python, no pip, nothing to install.

```
chmod +x ACECM
./ACECM
```

To get it in your app launcher, put it on your PATH and add a desktop entry:

```
mkdir -p ~/.local/bin ~/.local/share/applications
mv ACECM ~/.local/bin/ && chmod +x ~/.local/bin/ACECM
cat > ~/.local/share/applications/ACECM.desktop <<EOF
[Desktop Entry]
Type=Application
Name=AC EVO Content Manager
Exec=$HOME/.local/bin/ACECM
Terminal=false
Categories=Game;Utility;
EOF
```

Settings and profiles live in `~/.local/share/ACECM`.

#### Two things to set up once

**1. The game needs a launch option.** In Steam, right-click Assetto Corsa EVO
→ Properties → Launch Options:

```
VKD3D_CONFIG=force_raw_va_cbv %command%
```

Without it the client dies at renderer init with `Failed to create command
signature` — vkd3d-proton cannot build a command signature that updates a root
descriptor unless root CBVs are raw VAs. ACECM sets this itself when it
launches the game, but Steam launches need it too.

**2. The lobby proxy needs port 448, which Linux reserves for root.** The game
connects there, so it cannot be moved:

```
sudo sysctl -w net.ipv4.ip_unprivileged_port_start=448
echo 'net.ipv4.ip_unprivileged_port_start=448' | sudo tee /etc/sysctl.d/50-acecm.conf
```

Everything except the lobby (Drive, Content, servers, thumbnails) works
without it.

#### How it runs the game

The game and the dedicated server are Windows binaries. ACECM runs them
through the **Proton prefix Steam already uses for AC EVO**, so launch the game
from Steam once before first use — that is what creates the prefix. The 3D car
viewer (`evoview.exe`) goes through the same prefix.

#### Window

ACECM uses a native window when the desktop can provide one:

1. A WebKitGTK window, hosted by your system Python. Needs `python3-gobject`
   and `webkit2gtk4.1`, which most desktops already have.
2. A Chromium-family browser in `--app` mode.
3. Otherwise your default browser.

The single binary carries its own Python, so it cannot import the distro's
PyGObject directly (different ABI) — it asks the system Python to draw the
window instead. Nothing to install either way.

#### Optional

* `xdotool` or `wmctrl` — lets ACECM raise the game window, and enables the
  keyboard pit hotkey. Without focus detection that binding stays **off** on
  purpose: `ctrl+y` is redo in most editors and firing it there would teleport
  a car in a session running behind. Wheel and pad buttons need none of this.
* Reading wheel buttons needs membership of the `input` group:
  `sudo usermod -aG input $USER` (log out and in).

## Quick start

1. Open ACECM. Confirm **Settings** shows a game exe and a dedicated-server exe.
2. **Lobby** (Backend): start **proxy** and apply the redirect, then launch the game from ACECM. Steam “Verify integrity of game files” restores the original lobby URL; launch from ACECM again after that.
3. **Drive**: pick a car and a track, then Drive for single-player, or start **My server** and join it from Drive.
4. Drop a car or track archive onto any page to install. The same files again ask before overwrite.
5. **Convert**: pick an Assetto Corsa car or track from the grid and press Convert. It is checked and put in the game, ready to drive.

## Features

### Drive

Home screen, same idea as Content Manager’s Drive:

- Pick a car (stock presets and installed mods, with thumbnails).
- Pick a stock layout or a custom track you installed.
- Single-player writes a session the client already understands and launches into it. EVO has no `-car` / `-track` launch flags; ACECM does not pretend otherwise.
- **My server** starts a saved dedicated-server profile and waits until TCP 9700 is actually open before joining.
- Join a listed host, a favourite, or a pasted address. Join does not fire until the server port responds.

Search filters the car grid in place. Thumbnails are cached; rendering is not kicked off by typing.

### Content

Drop a car mod or a track pack onto **any** page.

**Cars** install to the user profile (`Saved Games\ACE` / `ACE-Server\mods`), not into the Steam server folder. A selectable car needs both the `.kspkg` and its `.json`. The Content library flags a missing sidecar so that failure is not confused with a bad package.

**Tracks** cannot be loose files on the dedicated server: the engine resolves content by hash against `content.kspkg`. ACECM writes the track into that archive (native folder or a borrowed stock slot), takes a `.bak_pretrack` backup before the first write, and lists the result in Drive. **Stop the dedicated server first** — it keeps the archive open.

The library can delete items and export them. Export writes an `.acemod` by default: the same compressed package sharing uses (files identical to the game's own are left out and rebuilt from the installer's `content.kspkg`, so a ~540 MB converted car is a few tens of MB). Dropping an `.acemod` on any page shows what it holds, rebuilds it, checks every byte and installs it like the plain files; if it was made with a different game version, ask for the plain export. The `.zip` / `.tar` button saves the plain files (tracks in the same multiplayer pack format you can drop back in) for people without ACECM. AI splines (`.ideal_line` / `.pitlane`) are copied next to the server when the client or the import has them.

### Convert (Assetto Corsa → EVO)

Turns cars and tracks from the original Assetto Corsa installed on your PC into AC EVO mods. The converter is built into ACECM and its source is in [`converter/`](converter/) (GPL-3.0-or-later).

- **Cars**: body, LODs, materials and liveries, wheels and tyres, physics (engine, turbo, gearbox, RWD / FWD / AWD drivetrains, suspension, tyres, aero, setup ranges), lights, shift lights, dash and gauges, cameras, exhaust effects, the seated driver, car animations and sound (AC1 FMOD banks rebuilt as EVO banks).
- **Tracks**: every layout, surfaces and collision, materials, AI lines, pits and the pit limiter, grid / pit / hotlap spots, timing, TV cameras, DRS zones, the dynamic (rubbering) track, baked lighting, 3D trees and grass, CSP lights, and the track pictures and maps.
- Every conversion is checked before anything is installed, and the result (errors / warnings) is shown. Nothing that fails the check is put in the game.
- Nothing from either game is shipped: the base car and the material / sound templates are built once from **your own** AC EVO install, the first time you convert.
- Converting again replaces the earlier version, and earlier conversions of the same car or track are offered for replacement. Replacing a car also resets the game's saved cameras and mirrors for it (the settings files are backed up first).
- A track is added to the game's track list only when you ask, with the game closed, using the same backed-up and verified writer as track deploy.
- Cars and tracks protected by their authors (CSP encryption) are marked and not converted.
- Runs in the background at low priority: half your CPU cores by default, fewer when memory is short (both adjustable).

The converter is new: tracks have had less in-game testing than cars, and the result depends on the source car / track. Please report anything that looks or drives wrong, with the name and the newest log from `Saved Games\ACE\Logs`.

### Garage and tracks

- **Cars** is the list the *dedicated server* can load. That is the list that matters for a join. Open a car in **evoview** (shipped inside `ACECM.exe`); it reads the `.kspkg` in place and does not extract the package. The viewer switches liveries in place (also from the car page), shows body kits and the seated driver, and has a drive / demo mode with suspension, lights, live dash and effects.
- **Tracks** use the same index as the launcher’s `EVENT_IDX` (for example Touristenfahrten is 18, not 0). Custom tracks appear alongside stock layouts after install.

### Dedicated servers

Save profiles (name, track or custom track, slots, weather, time, allowed cars) and Start / Stop them.

The dedicated server is a console-subsystem binary. ACECM starts it with a hidden console and keeps a log open so the process stays up and binds its ports. There is no extra CMD window by design.

New profiles start with **0 AI**. Stock servers do not run AI bots. Status is based on a live process, not a leftover PID in a file.

### Lobby

| | Proxy | Standalone |
|---|---|---|
| Upstream | Relays to Kunos | None — offline |
| Account, garage, progression | Real | Empty |
| Public server list | Yes | No |
| Your servers | Injected into the list | The only entries |

Leave **proxy** on unless you need a fully offline lobby.

### Other

- **Game settings** — read, backup, export, and restore client settings files.
- **Share** — other ACECM users can pull the cars and tracks a server needs. Hosts also offer a compressed copy of each: files identical to the game's own are not sent and are rebuilt from the joiner's game, so a converted car of ~580 MB downloads as ~45 MB. Every rebuilt file is checked against the host's checksums; if it cannot be rebuilt (a different game version), the normal files are downloaded instead.
- **Logs** — ACECM and server output, with the noisy server time spam filtered.
- **Updates** — from GitHub Releases, inside the app; each download is checked against the release signature and checksum before it replaces anything.

## Configuration

| | |
|---|---|
| Data | `%LOCALAPPDATA%\ACECM` (or `ACECM_DATA`) |
| Config | `config.json` in that folder — local paths, never part of the git repo |
| UI / API port | `ui_port` (default 8092) |
| Lobby | `backend_port` (default 448) |
| Dedicated TCP / HTTP | `default_tcp_port` 9700, `default_http_port` 8080 |

Detect walks Steam libraries for `AssettoCorsaEVO.exe` and `AssettoCorsaEVOServer.exe`. Settings overrides always win.

## Build from source

The Windows app is Rust + [Tauri](https://tauri.app) in `native/`; the web UI
it shows lives in `acecm/web`. You need the Rust MSVC toolchain and Python 3
(the build script is Python).

```bash
python build.py --native   # -> dist/ACECM.exe (one file: UI, viewer and data embedded)
cd native && cargo run     # development build from this checkout
```

The version lives in `native/Cargo.toml` and `native/tauri.conf.json`, which
must agree; `build.py` refuses to build when they differ.

The converter (`converter/`) is its own Rust crate; `native/build.rs` builds
it and embeds `aceconv.exe` in the app. Its formats and rules are documented
in [`converter/docs/FORMATS.md`](converter/docs/FORMATS.md), and it can be run
on its own (`aceconv car <ac1 car> <out>`, `aceconv track <ac1 track> <out>`,
`aceconv check-car` / `check-track`). Third-party crates and their licences
are listed in [CREDITS.md](CREDITS.md).

The older Python app that Linux 1.0.21 was built from is no longer in this
repository; it returns when Linux is ported.

Windows releases are built and signed locally, then published with
`tools/publish_release.py`, which refuses to upload without a signature that
verifies.

Contributor rules so a feature works on a **fresh `ACECM.exe`**, not only the author’s PC: [PORTABILITY.md](PORTABILITY.md).

## What this repository does not contain

- The game, dedicated server, or any Kunos assets (the converter builds what it needs from your own install, on your machine)
- Protobuf schemas extracted from the game binary (generated per machine on first use)
- TLS keys (the lobby proxy creates a self-signed pair locally)
- `config.json` and `data/` — those stay on the machine that runs ACECM

ACECM is an independent tool. It is not affiliated with Kunos Simulazioni or 505 Games.
