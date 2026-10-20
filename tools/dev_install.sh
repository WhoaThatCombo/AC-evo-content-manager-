#!/usr/bin/env bash
# Build "ACECM Dev" from this checkout and (re)install it BESIDE the normal
# ACECM: its own folder (Programs\ACECM-Dev), data (%LOCALAPPDATA%\ACECM-Dev),
# port (8292) and WebView2 identity, and no self-updater. It is the same
# code as the release build plus the dev-edition feature, so a release
# candidate can be tried on a real machine first.
#
# The converter (converter/) and the 3D viewer (tools/evoview.exe) are
# bundled inside the exe by native/build.rs, so the install is one exe.
#
#   tools/dev_install.sh          build, install, start
#   DEV_NO_START=1 tools/dev_install.sh
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-L:/build/acecm-dev}"
# a WebView2 identity of its own: two apps sharing one profile folder
# fight over it when both run
export TAURI_CONFIG='{"identifier":"com.whoathatcombo.acecm.dev","productName":"Assetto Corsa EVO Content Manager (dev)"}'
# idle priority: the PC stays usable while it compiles
(cd "$here/native" && cmd //c "start /low /b /wait cargo build --release --features dev-edition")
R="$CARGO_TARGET_DIR/release"
curl -s -m 3 -X POST -d '{}' http://127.0.0.1:8292/api/app/quit >/dev/null || true
sleep 4
"$R/acecm.exe" --install
inst="$LOCALAPPDATA/Programs/ACECM-Dev"
# converter copies installed beside the app by older dev builds: the
# bundled one is the only one now
rm -f "$inst/aceconv.exe"
rm -rf "$inst/fmod_template"
if [ -z "${DEV_NO_START:-}" ]; then
    cmd //c start "" "$(cygpath -w "$inst/ACECM-Dev.exe")"
fi
