# Assetto Corsa EVO Content Manager

**Testing only — unfinished.**

Host modded AC EVO multiplayer: install content, run a dedicated server, join
it from Drive, and show a public live map.

## Get it

1. Download **[ACECM.exe](https://github.com/WhoaThatCombo/AC-evo-content-manager-/releases/latest)** and run it (Windows 11 / WebView2).
2. You need the **game** and a **dedicated server** already installed. ACECM does not ship either.
3. First launch finds Steam’s game folder and `AssettoCorsaEVOServer.exe` in the usual places. If it misses them, set both in **Settings**.
4. No Python. Data lives in `%LOCALAPPDATA%\ACECM`.

**Settings → check for updates** pulls the latest release and swaps the exe on restart (`ACECM.exe.old` is the previous build).

```
ACECM.exe              window (default)
ACECM.exe --browser    your browser instead
ACECM.exe --headless   API only
```

## What you do in it

| Page | What it is |
|---|---|
| **Drive** | Pick a car. Start or join **My server**, or join a listed host. Join waits until TCP 9700 is actually open. |
| **Servers** | Save profiles (track, slots, weather, cars) and Start / Stop. |
| **Content** | Drop a car or track onto *any* page to install. Library: delete, export (MP tracks), clipboard. Same files again → confirm overwrite. |
| **Live** | `/live` — read-only map anyone can open. No admin. |
| **Backend** | The lobby ACECM runs so the game can list and join your server. Leave **proxy** on to keep your real Kunos account. |
| **Settings** | Game path, server path, ports. |

Launch the game **from ACECM**. Steam “Verify integrity” undoes the lobby redirect and joining stops working until you Launch again.

## Do not

- Set the in-game **year** outside **2020–2035**. Values like 1948 crash the dedicated server before it binds 9700.
- Expect a visible server console. It is hidden on purpose; the exe will not stay up without a console, so ACECM gives it a hidden one.
- Expect AI bots on a stock server. New profiles start with 0 AI.
- Put ACECM’s game, server, or TLS keys in this repo. They stay on your machine.

## Build from source

```bash
pip install pyinstaller protobuf numpy pywebview cryptography websockets capstone texture2ddecoder pillow
python build.py            # -> dist/ACECM.exe
python -m acecm            # run from this checkout
```

Tag must match `VERSION` in `acecm/version.py` (`git tag v0.7.6 && git push origin v0.7.6`).
