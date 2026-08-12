# Assetto Corsa EVO Content Manager

WARNING THIS IS CURRENTLY FOR TESTING PURPOSES AND IS VERY UNFINISHED

A desktop app for hosting modded AC EVO multiplayer: content, server profiles,
live telemetry, and the self-hosted lobby that makes joining possible at all.

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

### Updating

**Settings -> check for updates** reads this repo's latest Release, verifies the
download against the SHA-256 GitHub publishes for the asset, and swaps the exe
on next restart. The previous build is kept as `ACECM.exe.old`.

## Build from source

```bash
pip install pyinstaller protobuf numpy pywebview cryptography websockets capstone
python build.py            # -> dist/ACECM.exe
python -m acecm            # or just run it from source
```

Releases are built by GitHub Actions on a tag (`git tag v0.4.0 && git push
origin v0.4.0`). The workflow refuses to build if the tag does not match
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

Proven working — the client's own log ends with
`Established connection to server 127.0.0.1:9700 (yay, this is good)`.

The protocol work lives in `acevo_localconnect/` (see its README); this app
supervises it and gives it a UI.

---

## Pages

**Dashboard** — content counts, backend state, start/stop, launch the game.

**Servers** — profiles you can save, edit and run: track, AI count, slots,
ports, time of day. Starting one shells out to `start_vai_server.py`, which
encodes the base64 `serverconfig` / `seasondefinition` blobs the exe expects.
Live status shows the client count from the server's own HTTP port, and logs
are readable inline (with the vAI `ServerWorldTime` spam filtered out).

**Cars** — what the *dedicated server* can actually load, which is the thing
that matters: a car the server cannot resolve is a broken join no matter what
the client has. Kunos presets are `<code>_mech_<n>`; anything else came from a
modded client and is flagged.

**Tracks** — the 36 layouts, indexed exactly as the launcher's `EVENT_IDX`, so
the number shown *is* the launch index. (Nordschleife Touristenfahrten = 18;
the default of 0 is Brands Hatch, which has caught us out before.)

**Content** — install and remove car mods, and check track readiness.

Car mods go to the **user profile**, not the server's install folder:
`%USERPROFILE%\Saved Games\ACE-Server\mods\`. Each needs **both**
`<mod>.kspkg` and `<mod>.json`; the `.json` is what makes the car selectable.
A mod missing it installs "fine" and then never appears in the car list — the
page flags exactly which file is absent, because the two failures look
identical from in-game but are fixed in different places.

Tracks are listed per layout with an **AI-ready** check: the AI needs both
`.ideal_line.aisplinedata` and `.pitlane.aisplinedata`, and the server's own
package ships neither. They must be loose on disk — content added to a `.kspkg`
is invisible, since the engine resolves paths by hash against the archive index.

The same page deploys **custom tracks**, which work completely differently from
cars. A track cannot be dropped in as loose files: the dedicated server has no
loose path for track logic, and the engine resolves content by hash against the
archive index, so a brand-new path cannot be found at all. A package therefore
installs by **borrowing an existing track's slots** inside `content.kspkg`.

Three safety rules are enforced rather than assumed:

1. **The server must be stopped** — it holds the 300 MB archive open.
2. **The archive is backed up** before the first write (`.bak_pretrack`), with a
   Restore button, because a failure part-way through leaves it inconsistent.
3. **The path is passed explicitly.** `penalties_tool.find_server_kspkg()`
   resolves to the *Steam* install, which is not the portable server we run —
   auto-detection would patch a server you are not using and leave the real one
   untouched, which looks exactly like "nothing happened".

Packages missing `containers.bin` (built before that file existed) are detected
and refused with that reason, rather than failing obscurely mid-write.

**Backend** — start/stop the lobby, TLS state, live log.

**Settings** — paths and ports, written to `config.json`.

---

## Known limits, stated plainly

**Installed mods were not selectable, and now are.** Cars shipped by a mod are
not in `cars.json` (that file is a client dump), so the launcher's allowed-cars
list excluded them entirely — the mod installed correctly and no player could
ever pick it. ACECM merges mod-declared car ids into both the inventory and the
server's allowed list.

**Car ids use two different schemes and they are NOT mapped.** `cars.json` is
mostly `preset_<code>_mech_<n>` (93 of 107), while the server log records full
model names (`ks_bmw_m4_gt3`). The short codes cannot be expanded into model
names without a lookup we do not have, so the Cars page shows the codes
honestly and lists real model names separately under "Models seen on this
server", harvested from actual join records. Guessing a mapping would put wrong
names on cars. Mods are the exception: a mod's `.json` declares
`display_name` for the ids it ships, so those get real names
(`preset_mazda_rx_s_mech_1` → "Mazda RX-8 drift").

**Per-profile process tracking is approximate.** `status()` finds *a* running
dedicated server rather than proving it is this profile's; with one server at a
time that is fine, and it is the honest limit of matching on a base64 blob.

**Client redirect is not automated yet.** Pointing the client at our lobby still
means running `client/patch_backend_url.py` (and `--restore` to undo). The
Backend page reports the state it can see but does not yet drive it.

**Track deploy is built but not yet run end-to-end.** Validation is proven
against four real packages on disk (one ready, three missing `containers.bin`),
and every guard fires correctly; the actual archive write has not been executed
yet.

**Telemetry is not wired in yet.** `server_telemetry.py` and `track_viewer.py`
work standalone (see `acevo-server-side-telemetry` notes); folding them in as a
Telemetry page is the next obvious step.

---

## Official shared memory (new, not yet used)

Kunos documents three mapped segments the **client** exposes:
`Local\acevo_pmf_physics`, `_graphics`, `_static` — the graphics page carries
coordinates for up to 60 cars plus full physics for the local car.

Verified against the binaries: **the client has all three, the dedicated server
has none.** So this is a much cleaner replacement for the client-side telemetry
hack (DevTools + a hand-fitted calibration), but it does *not* provide
server-side telemetry — that still needs the memory-scan approach.

---

## Layout

```
acecm/
  app.py        HTTP server + JSON API
  config.py     paths and ports (override with config.json)
  content.py    car and track inventory
  servers.py    profiles, start/stop, status, logs
  backend.py    own lobby supervision, game launch
  web/          UI (index.html, app.js, style.css)
data/           profiles.json and logs, created on first run
```
