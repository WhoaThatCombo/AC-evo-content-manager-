# ACECM → Rust + Tauri port

Goal: the same app, as a small native binary — ~5–15 MB instead of 41 MB,
sub-second start (no onefile unpack), less RAM. The UI keeps running on
WebView2 (it already did, via pywebview) and keeps talking to the same
HTTP API, so it moves over unchanged.

## How the port stays shippable at every step (strangler)

```
WebView2 window ──► Rust shell :8092 (public)
                     ├─ ported routes ─► answered in Rust
                     └─ everything else ─► Python backend (hidden, 127.0.0.1:<random>)
```

* `native/` is the Rust/Tauri app. `cargo run` starts the window, binds the
  public port, and starts `python -m acecm --headless` behind it with
  `ACECM_ROOT` (real %LOCALAPPDATA%\ACECM data), `ACECM_UI_PORT`/`ACECM_LISTEN`
  (private loopback port), `ACECM_PROXY_SECRET`, `ACECM_PARENT_PID`.
* ⚠ Peer identity: through the proxy every request arrives from loopback, so
  the shell sends `X-ACECM-Peer` + `X-ACECM-Proxy: <secret>` and the backend
  (`app.Handler._peer_ip`) believes the peer only with the secret. Client
  copies of those headers are stripped. Without this a LAN client would pass
  as "this machine" and skip the token check.
* ⚠ `Host` is passed through untouched — the CSRF check compares Origin to it.
* Bodies stream both ways (SSE `/api/events`, downloads).
* A route moves to Rust only with an **equivalence check**: the same request
  against Rust and Python, JSON diffed (`native/tools/diff_api.py`, to write).
* Ship natively only when **no route is proxied** and the helper tools are
  ported too — until then the download would still carry Python.

## Order (dependencies first, highest-traffic next)

| # | Area | Python | Notes / crates |
|---|------|--------|----------------|
| 1 | Shell, window, proxy, static UI | ui.py, app.py (serving) | tauri 2, axum, reqwest. Theme injection `__ACECM_THEME__`. |
| 2 | config, paths, logs, detect | config.py, detect.py, logs.py | Steam library VDF parsing. |
| 3 | process utils | _proc_win.py, winproc.py | windows-sys: toolhelp snapshot, ports, hidden spawn, child_env (strip our dirs). |
| 4 | live feed | events.py | SSE from axum; one builder task. |
| 5 | content scan, cars, tracks | content.py, carspecs.py, carsmap.py, tracks.py, tracktables.py | |
| 6 | kspkg read/write | kspkg.py, kspkg_write.py | FNV-1a UTF-16 table, encryption check (memories). |
| 7 | protobuf | protos.py, tools/acevo_proto.py | prost-reflect: descriptors extracted from the game exe. |
| 8 | servers | servers.py, lobby.py, penalties.py, tools/penalties_tool.py | never ship vAI / patched server. |
| 9 | drive / join / game UI | drive.py, gameui.py, backend.py, patching.py, hooking.py | capstone-rs for pattern scans; cohtml inspector WS. |
| 10 | lobby proxy + backend | tools/acevo_proxy.py, acevo_backend.py, join_push.py | tokio-tungstenite + rustls server certs. |
| 11 | direct lobby fetch | tools/lobby_fetch.py | libloading steam_api64; MUST stay a short-lived child process. |
| 12 | install / packs / share / registry | install.py, pack.py, contentsync.py, evoshare.py, registry.py, autoshare.py, push.py | zip crate; archive_budget limits. |
| 13 | thumbs / livery / textures | thumbs.py, livery.py | texture2ddecoder (Rust crate), image. |
| 14 | telemetry, splines | telemetry.py, splines.py, tools/server_telemetry.py | |
| 15 | settings, assists, hud, hotkey, damage | settings.py, hotkey.py, damage.py | |
| 16 | updater, installer, signing | version.py, installer.py | ed25519-dalek, same signed-release format (asset+version+sha256). |
| 17 | Linux | _proc_posix.py, proton.py, _input_posix.py | Tauri on WebKitGTK; glibc floor. |

Sizes today: acecm/ 27.8k lines Python, runtime tools 4.4k, web UI 8.8k
(stays), 163 API routes.

## Progress (2026-10-14)

Answered in Rust, each equal to the Python answer (`tools/diff_api.py`,
`tools/diff_thumbs.py`):

* UI files (theme + cache-tag injection), `/live`
* `GET/POST /api/config`, `/api/detect`, `/api/auth`, `/api/version`
* `/api/logs`, `/api/logs/files`
* `/api/mods`, `/api/mods/audit`, `/api/cars/map`, `/api/viewer/cars`
* `/api/cars`, `/api/tracks` (133 cars / 68 tracks)
* `/api/thumb/car`, `/api/thumb/track` - cached pictures; a request that
  would render/decode one still goes to the backend (221 rows identical)
* `/api/gamesettings`, `/read`, `/backups` (180 settings files identical)
* `/api/assists`, `/api/drive/fav`, `/api/tracks/installed`

Core libraries ported: kspkg reader (59 ms vs 2.05 s on the 69 GB
content.kspkg), protobuf schema extraction + prost-reflect pool, garage
(livery) reader, Windows process/port helpers, Steam detection.

⚠ Shared state rules while both run:
* config.json: Python re-reads on mtime change (`config.refresh`).
* Caches shared by file (paths.json, preset_map.json, viewer/index.json,
  track_map.json, protos/): same format and stamps on both sides. The
  backend DELETES some on a content change (`forget_lists`), so Rust treats
  a missing file as a miss even with a warm memory copy.
* Anything owned by a running process in Python memory (servers, drive job,
  proxy, telemetry) moves as a WHOLE subsystem, never route by route.

Next: servers (profiles + start/stop + status), then the lobby proxy and
drive/join, then events (needs both), install/share/registry, telemetry.

## Build

```
cd native
cargo run            # debug; Python backend from this repo
cargo build --release
```

Target dir is `J:/build/acecm-native` (`.cargo/config.toml`) — C: is full.
Toolchain pinned to MSVC (`rust-toolchain.toml`).
