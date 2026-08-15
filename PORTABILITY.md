# Portability — this must work on any Windows box

ACECM is shipped as **one `ACECM.exe`**. The author checkout, `C:\Users\joshu\…`,
`ACE_server_portable`, `acevo_localconnect`, and a local Python install **do
not exist** on a fresh machine. If a feature only works here, it is not done.

The test is: **download the release exe onto a PC that has Steam, the game, and
a dedicated server — nothing else from this repo.** Settings may be filled in
if detect misses a folder. Everything we added must still run.

---

## Hard rules

1. **No machine paths in source or defaults.** Not `C:\Users\…`, not
   `Downloads\acecm`, not a specific `AssettoCorsaEVOServer.stock.exe` name.
   `config.DEFAULTS` stay empty for `server_dir`, `server_exe`, `tools_dir`,
   `game_exe`. Detect, then Settings.

2. **`config.json` is gitignored on purpose.** It is this PC’s paths. Never
   commit it. Frozen builds read `%LOCALAPPDATA%\ACECM\config.json` (created
   empty from `DEFAULTS`).

3. **User data is `%LOCALAPPDATA%\ACECM` when frozen.** Never write next to
   the exe (Program Files / one-file temp). Use `config.DATA`. Set
   `ACECM_DATA` only for a clean test dir.

4. **Helpers ACECM owns come from the bundle.** `start_vai_server.py`,
   `acevo_proxy.py`, `acevo_backend.py`, `server_telemetry.py`, catalogues,
   `evoview.exe` live in `tools/` and are packed into the exe. Do not prefer a
   leftover copy next to the dedicated server — that is how a fix never
   reached other machines.

5. **Frozen has no `python`.** `sys.executable` is `ACECM.exe`. Child jobs are
   `ACECM.exe --tool <name>`. `ACECM.exe -u some.py` starts a second app
   window. `config.tool_cmd` already does this; do not invent a second way.

6. **`build.py` may copy from the author’s Downloads at build time only.**
   Those folders are optional. A clone that only has committed `tools/` must
   still produce a working exe. Never let an older portable
   `start_vai_server.py` overwrite the copy in this repo.

---

## Things that already broke “works on my PC”

Treat these as regression tests, not folklore.

| What we did here | What happened elsewhere / next start |
|---|---|
| Default `server_exe` = `AssettoCorsaEVOServer.stock.exe` | Stock Steam install has `AssettoCorsaEVOServer.exe` → “exe not found” |
| `pythonw` parent + launcher exits | Dedicated server dies; 9700 never stays open |
| `CREATE_NO_WINDOW` or `stdout=DEVNULL` on the server exe | Prints `Start Server` then dies (`0xC0000409`); no bind |
| Redirect server stdout to `last_start.log` and close it | Same death |
| In-game year outside 2020–2035 | Same death before 9700 |
| Require `cars.json` / `events_*.json` next to the server | Stock dedicated server has neither |
| Frozen telemetry as `python -u server_telemetry.py` | No Python; or `ACECM.exe -u` opens another ACECM |
| `runtime.json` pid still “running” | Windows reused the PID; Drive skipped Start |
| QuitGame JS only, then assume process gone if inspector died | Game window stayed up |
| Pull launchers from `ACE_server_portable` if newer | Author’s old script overwrote the ship fix |

The dedicated server is a **console-subsystem** binary. Give it
`CREATE_NEW_CONSOLE` (hidden is fine) and a **log file this process keeps
open** until the exe exits. Do not use `CREATE_NO_WINDOW`.

---

## How to resolve things

| Need | How |
|---|---|
| Game / server folder | `detect.py` (Steam libraries + generic names), then Settings |
| Server executable | Any `AssettoCorsaEVOServer*.exe` that is not a `.percar` / `.test` / `.bak` copy |
| Helper script | `config.tool_script` / `tool_cmd` (bundle first for owned tools) |
| Stock car/track list | `config.catalog_path` — server folder, else bundled `tools/*.json` |
| evoview | `tools/evoview.exe` inside the bundle; Settings `viewer_exe` only as override |
| Live telemetry UI | `acecm/web/live.html` packed with the other web files |
| Close the game | JS quit, then `WM_CLOSE`, then `taskkill /IM AssettoCorsaEVO.exe /T /F`. A dead inspector is not “game closed” |
| Protobufs / TLS | Extract / generate on **that** machine. Never ship keys or `.desc` from here |

Steam drops launch argv (`Arguments: 1`). Launch via `steam://` after rewriting
the lobby URL **in the exe**. First write under Program Files may need
**Run as administrator once**. That is every Steam install, not a quirk of
this PC.

---

## Checklist before a change is “done”

Walk this as if the author folders do not exist.

- [ ] No new absolute path, username, or one-off exe filename.
- [ ] Defaults still empty; detect + Settings cover a Steam-only layout.
- [ ] Frozen path uses `--tool` / bundled files, not `python` or `__file__`
      next to the server.
- [ ] Starting a server still uses a hidden **console** + a log that stays open.
- [ ] Year is clamped 2020–2035 on start **and** save.
- [ ] Missing `cars.json` / `events_*.json` on the server still works (bundle).
- [ ] A leftover pid or a dead inspector cannot fake “running” / “closed”.
- [ ] `config.json` and `data/` are not in the commit.
- [ ] You could explain the feature to someone who only has `ACECM.exe`.

A run that succeeds only with `pythonw -m acecm` from this repo is **not**
validated. Prefer `ACECM_DATA` pointing at an empty folder, or the installed
`ACECM.exe` after a real build.

Windows 10/11 + Steam + WebView2 is the product. We do not need Linux. We do
need “any user’s Steam library letter and folder names.”
