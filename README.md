# Assetto Corsa EVO Content Manager

ACECM is a desktop manager for [Assetto Corsa EVO](https://store.steampowered.com/app/3058630): install cars and tracks, run a dedicated server, join it from Drive, and share a live map. It is built for the same job Content Manager does for the original Assetto Corsa — content, hosting, and getting you on track without fighting the stock launcher.

Current release: **0.10.5**. Windows 10/11, Edge WebView2 (included with Windows 11).

## What it is

AC EVO does not expose a usable direct-connect. Joining a session goes through the game’s online lobby. ACECM runs that lobby on your machine (proxying Kunos so your account, garage, and public list stay real), installs mods where the dedicated server can actually load them, and launches the game with the session you picked.

It does **not** ship the game, the dedicated server, or Kunos content. You already own those on Steam.

## Install

1. Download **[ACECM.exe](https://github.com/WhoaThatCombo/AC-evo-content-manager-/releases/latest)** from the latest GitHub Release.
2. Run it. There is no installer.
3. On first launch it locates the Steam game folder and `AssettoCorsaEVOServer.exe`. If either is missed, set it under **Settings**.

Settings, server profiles, logs, and thumbnails live in `%LOCALAPPDATA%\ACECM`. Override the data folder with the `ACECM_DATA` environment variable if you need a clean profile.

**Settings → check for updates** downloads the newest release, verifies the SHA-256 published with the asset, and swaps the exe on the next restart. The previous build is kept as `ACECM.exe.old`.

```
ACECM.exe              native window (default)
ACECM.exe --browser    open in your default browser
ACECM.exe --headless   API only (for Studio or another client)
```

## Quick start

1. Open ACECM. Confirm **Settings** shows a game exe and a dedicated-server exe.
2. **Lobby** (Backend): start **proxy** and apply the redirect, then launch the game from ACECM. Steam “Verify integrity of game files” restores the original lobby URL; launch from ACECM again after that.
3. **Drive**: pick a car and a track, then Drive for single-player, or start **My server** and join it from Drive.
4. Drop a car or track archive onto any page to install. The same files again ask before overwrite.

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

The library can delete items and export them, including tracks in the same multiplayer pack format you can drop back in. AI splines (`.ideal_line` / `.pitlane`) are copied next to the server when the client or the import has them.

### Garage and tracks

- **Cars** is the list the *dedicated server* can load. That is the list that matters for a join. Open a car in **evoview** (shipped inside `ACECM.exe`); it reads the `.kspkg` in place and does not extract the package.
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

### Live

`/live` is a read-only map and board anyone can open. It is not an admin surface. Share the link from the Live page.

### Other

- **Game settings** — read, backup, export, and restore client settings files.
- **Share** — other ACECM users can pull the cars and tracks a server needs.
- **Logs** — ACECM and server output, with the noisy server time spam filtered.
- **Updates** — GitHub Releases, checksum-verified.

## Configuration

| | |
|---|---|
| Data | `%LOCALAPPDATA%\ACECM` (or `ACECM_DATA`) |
| Config | `config.json` in that folder — local paths, never part of the git repo |
| UI / API port | `ui_port` (default 8092) |
| Telemetry | `telemetry_port` (default 8091) |
| Lobby | `backend_port` (default 448) |
| Dedicated TCP / HTTP | `default_tcp_port` 9700, `default_http_port` 8080 |

Detect walks Steam libraries for `AssettoCorsaEVO.exe` and `AssettoCorsaEVOServer.exe`. Settings overrides always win.

## Companion UI — ACECM Studio

Studio is a separate Drive-first UI that uses this same local API: cached car lists and thumbnails, and an in-app first-run guide. It does not live in this repository and does not modify ACECM. Run ACECM as usual, or `ACECM.exe --headless` if Studio should own the window, then start Studio (`python studio.py` → http://127.0.0.1:8765).

## Build from source

```bash
pip install pyinstaller protobuf numpy pywebview cryptography websockets capstone texture2ddecoder pillow
python -m acecm           # run from this checkout
python build.py           # -> dist/ACECM.exe
```

Releases are built by GitHub Actions on a version tag. The tag must match `VERSION` in `acecm/version.py` (for example `git tag v0.10.5 && git push origin v0.10.5`).

Contributor rules so a feature works on a **fresh `ACECM.exe`**, not only the author’s PC: [PORTABILITY.md](PORTABILITY.md).

## What this repository does not contain

- The game, dedicated server, or any Kunos assets
- Protobuf schemas extracted from the game binary (generated per machine on first use)
- TLS keys (the lobby proxy creates a self-signed pair locally)
- `config.json` and `data/` — those stay on the machine that runs ACECM

ACECM is an independent tool. It is not affiliated with Kunos Simulazioni or 505 Games.
