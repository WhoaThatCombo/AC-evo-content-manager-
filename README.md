# Assetto Corsa EVO Content Manager

WARNING THIS IS CURRENTLY FOR TESTING PURPOSES AND IS VERY UNFINISHED

A desktop app for hosting modded AC EVO multiplayer: content, server profiles,
Drive (start / list / join), live telemetry, and the self-hosted lobby that
makes joining possible at all.

## Install

Grab **ACECM.exe** from the [latest release][rel] and run it. Nothing to
install: it opens its own window (Edge WebView2, which ships with Windows 11).

[rel]: https://github.com/WhoaThatCombo/AC-evo-content-manager-/releases/latest

```
ACECM.exe              native window (default)
ACECM.exe --browser    open in your default browser instead
ACECM.exe --headless   API only, for a machine with no desktop
```

Settings, profiles and logs live in `%LOCALAPPDATA%\ACECM`. Set `ACECM_DATA`
to put them somewhere else.

On first run it looks for the game (Steam libraries) and a dedicated-server
folder (`AssettoCorsaEVOServer.exe` in the usual Steam / Downloads places).
If either is missing, set it in **Settings**. You do not need Python, and you
do not need this repo's folder layout.

Stock `cars.json` / `events_*.json` catalogues ride inside the exe. A Steam
dedicated server that does not have those files next to it still starts.

### Updating

**Settings -> check for updates** reads this repo's latest Release, verifies the
download against the SHA-256 GitHub publishes for the asset, and swaps the exe
on next restart. The previous build is kept as `ACECM.exe.old`.

## Build from source

```bash
pip install pyinstaller protobuf numpy pywebview cryptography websockets capstone texture2ddecoder pillow
python build.py            # -> dist/ACECM.exe
python -m acecm            # or just run it from source
```

Releases are built by GitHub Actions on a tag (`git tag v0.7.6 && git push
origin v0.7.6`). The workflow refuses to build if the tag does not match
`VERSION` in `acecm/version.py`, because a mismatch makes the updater offer the
same update forever.

## What is NOT in this repository

* **The game, the dedicated server, or any Kunos content.** This manages an
  install you already have.
* **The game's protobuf schemas.** They are extracted from *your own* game
  binary on first use and cached in your data folder - they are derived from
  Kunos' executable, so they are not ours to redistribute.
* **TLS keys.** The lobby proxy generates a self-signed keypair per machine. A
  private key in a public repo would be the same key for every user.
* **AI bots.** Virtual-AI cars need a patched server binary; new profiles
  default to 0 AI and the stock server.

---

## Why a custom lobby is needed

AC EVO has no direct connect. `-direct` exists as a launch flag but aborts at
startup, and `connectToDirectServer()` sits in the binary with nothing calling
it. Joining is **backend-mediated by design**: the client asks Kunos's lobby to
resolve a server and authorise the join.

So the way in is not to bypass the backend but to **be** the backend. Two modes,
both driven from the Backend page:

| | proxy | standalone |
|---|---|---|
| upstream | relays to real Kunos | none, fully offline |
| your account, garage, progression | **real** | empty |
| public server list | **yes** | no |
| your servers | injected into the list | the only entries |

The protocol work lives in `acevo_localconnect/` (see its README); this app
supervises it and gives it a UI.

---

## Pages

**Dashboard** — content counts, backend state, start/stop, launch the game.

**Drive** — pick a car, start or join **My server** (a local ACECM host), or
join a listed multiplayer server. Join waits until TCP 9700 is actually
listening so the client does not get "socket did not respond" on a still-booting
host.

**Servers** — profiles you can save, edit and run: track, AI count, slots,
ports, time of day. Starting one shells out to `start_vai_server.py`, which
encodes the base64 `serverconfig` / `seasondefinition` blobs the exe expects.
The dedicated server is a console-subsystem binary: ACECM gives it a hidden
console and a log file that stays open for the life of the process. The
in-game **year must be 2020–2035** (1948 and similar values abort the exe
before it binds 9700).

**Cars** — what the *dedicated server* can actually load, which is the thing
that matters: a car the server cannot resolve is a broken join no matter what
the client has. Kunos presets are `<code>_mech_<n>`; anything else came from a
modded client and is flagged.

**Tracks** — the stock layouts, indexed exactly as the launcher's `EVENT_IDX`,
so the number shown *is* the launch index. (Nordschleife Touristenfahrten = 18;
the default of 0 is Brands Hatch.)

**Content** — drop a car or track archive onto any page to install it. The
library lists installed mods: delete, export (including MP-format tracks),
copy to clipboard. Installing the same files again asks before overwrite.
Custom tracks deploy into the server archive (native or borrowed slot) with
the server stopped and a backup taken first.

**Live** — `/live` is a public read-only map of the running server (no start /
stop / admin). AI splines are discovered from the client package and shipped
next to the dedicated server so every stock layout, plus Barber / Highlands
when those files exist, has a line.

**Backend** — start/stop the lobby, TLS state, live log.

**Settings** — paths and ports, written to `config.json`.

---

## Known limits, stated plainly

**Installed mods are selectable.** Cars shipped by a mod are not in `cars.json`
(that file is a client dump), so ACECM merges mod-declared car ids into both
the inventory and the server's allowed list.

**Car ids use two different schemes and they are NOT mapped.** `cars.json` is
mostly `preset_<code>_mech_<n>`, while the server log records full model names
(`ks_bmw_m4_gt3`). The Cars page shows the codes honestly and lists real model
names separately under "Models seen on this server". Mods are the exception: a
mod's `.json` declares `display_name`.

**Per-profile process tracking needs a live PID.** A leftover pid in
`runtime.json` is not enough (Windows reuses PIDs). Status also requires that
process to still be a dedicated server.

**Client redirect is required, and Launch does it.** Steam relaunches the
game with `Arguments: 1`, so `-backend=` never arrives. Launch from ACECM
rewrites the lobby URL in `AssettoCorsaEVO.exe` first. Steam "Verify integrity"
undoes that rewrite.

**The dedicated server window is hidden on purpose.** `CREATE_NO_WINDOW` and
redirecting its stdout to a file ACECM then closes both abort the exe. A
hidden console plus a log the launcher keeps open is the combination that
binds 9700 on any machine.

---

## Official shared memory (new, not yet used)

Kunos documents three mapped segments the **client** exposes:
`Local\acevo_pmf_physics`, `_graphics`, `_static` — the graphics page carries
coordinates for up to 60 cars plus full physics for the local car.

Verified against the binaries: **the client has all three, the dedicated server
has none.** So this is a cleaner replacement for client-side HUD scraping, but
it does *not* provide server-side telemetry — that still needs the memory-scan
approach behind `/live`.

---

## Layout

```
acecm/
  app.py        HTTP server + JSON API
  config.py     paths and ports (override with config.json)
  content.py    car and track inventory
  install.py    drop / library / overwrite
  servers.py    profiles, start/stop, status, logs
  drive.py      My server / join
  splines.py    discover and ship AI lines
  telemetry.py  server-side tracker + public /live
  backend.py    own lobby supervision, game launch
  web/          UI (index.html, app.js, live.html, style.css)
tools/          start_vai_server.py, lobby proxy, stock catalogues
```

A frozen build keeps user data in `%LOCALAPPDATA%\ACECM`, not next to the exe.
