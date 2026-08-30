"""Build a single shippable ACECM.exe.

    python build.py            build
    python build.py --clean    rebuild from scratch

What goes in: our own code only - the acecm package, its web assets, and the
helper scripts we wrote (telemetry tracker, server launcher, track injector,
lobby proxy).

⚠ What deliberately does NOT go in:

  * Any Kunos binary or content - the dedicated server exe, content.kspkg,
    track or car assets. The build manages the user's own install; it does not
    redistribute the game.
  * The vAI-patched server binaries. We do not ship AI bots (and a modified
    server binary is exactly the thing not to hand out).
  * The protobuf descriptors extracted from the game exe. They are derived from
    Kunos' binary, so they are read from the user's own install at runtime
    rather than redistributed. Game-settings editing needs a local game
    install for that reason.
  * TLS keys. Generated on the user's machine on first use, never shared.
"""
import argparse
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TOOLS = os.path.join(HERE, "tools")
DIST = os.path.join(HERE, "dist")


def _version_file():
    """A real VERSIONINFO resource. Blank file info + UPX is what Defender
    treats as a packed dropper. Do not UPX this exe."""
    sys.path.insert(0, HERE)
    from acecm.version import VERSION
    parts = []
    for chunk in str(VERSION).split("."):
        digits = "".join(c for c in chunk if c.isdigit())
        parts.append(int(digits) if digits else 0)
    while len(parts) < 4:
        parts.append(0)
    vers = tuple(parts[:4])
    comma = ",".join(str(n) for n in vers)
    path = os.path.join(HERE, "ACECM.version")
    path = os.path.abspath(path)
    body = f"""# UTF-8
VSVersionInfo(
  ffi=FixedFileInfo(
    filevers=({comma}),
    prodvers=({comma}),
    mask=0x3f,
    flags=0x0,
    OS=0x40004,
    fileType=0x1,
    subtype=0x0,
    date=(0, 0)
  ),
  kids=[
    StringFileInfo([
      StringTable(
        u'040904B0',
        [StringStruct(u'CompanyName', u'ACECM'),
         StringStruct(u'FileDescription', u'Assetto Corsa EVO Content Manager'),
         StringStruct(u'FileVersion', u'{VERSION}'),
         StringStruct(u'InternalName', u'ACECM'),
         StringStruct(u'LegalCopyright', u'ACECM'),
         StringStruct(u'OriginalFilename', u'ACECM.exe'),
         StringStruct(u'ProductName', u'Assetto Corsa EVO Content Manager'),
         StringStruct(u'ProductVersion', u'{VERSION}')])
    ]),
    VarFileInfo([VarStruct(u'Translation', [1033, 1200])])
  ]
)
"""
    open(path, "w", encoding="utf-8").write(body)
    return path, VERSION

# Where these scripts live on a DEV machine, so a build here picks up edits
# automatically. These folders do not exist anywhere else, and that is fine:
# the committed tools/ is then used as-is, which is what makes a fresh clone
# buildable.
_DL = os.path.join(os.path.expanduser("~"), "Downloads")
SOURCES = {
    os.path.join(_DL, "ACE_server_portable"): [
        # start_vai_server.py and server_telemetry.py live in this repo's
        # tools/. Pulling them from the portable folder overwrote ship
        # fixes whenever that copy was newer.
        "parse_spline.py",
        "parse_edges.py", "server_track_inject.py", "build_track_package.py",
        "penalties_tool.py",
    ],
    os.path.join(_DL, "acevo_localconnect", "backend"): [
        # acevo_proxy.py is owned by this repo. A newer copy next to the
        # author's other checkout used to overwrite the ship fix.
        "acevo_backend.py", "join_push.py", "gencert.sh",
        # the proxy imports this; without it a shipped backend cannot start
        "acevo_proto.py", "dump_protos.py",
    ],
    HERE: ["acecm_sync.py"],
    # the Vulkan car viewer, built separately with cargo
    os.path.join(_DL, "evoview", "target", "release"): ["evoview.exe"],
}


def stage_tools():
    """Refresh tools/ from the dev checkouts, if they exist on this machine.

    ⚠ tools/ is COMMITTED, so a fresh clone can build without the author's
    folder layout. On the dev machine the scripts still live in their original
    projects, so newer copies there win; anywhere else the committed copies are
    used untouched.
    """
    os.makedirs(TOOLS, exist_ok=True)
    updated, missing = 0, []
    for base, names in SOURCES.items():
        for n in names:
            src = os.path.join(base, n)
            dst = os.path.join(TOOLS, n)
            if os.path.isfile(src):
                if (not os.path.isfile(dst)
                        or os.path.getmtime(src) > os.path.getmtime(dst)):
                    shutil.copy2(src, dst)
                    updated += 1
            elif not os.path.isfile(dst):
                missing.append(n)
    have = len([f for f in os.listdir(TOOLS) if f.endswith((".py", ".sh"))])
    print(f"tools/: {have} helper script(s) ({updated} refreshed from dev copies)")
    # ⚠ The viewer is a real dependency of the Cars page, not an optional
    # extra: without it the picker can list cars but not open any of them.
    viewer = os.path.join(TOOLS, "evoview.exe")
    if os.path.isfile(viewer):
        print(f"  viewer: evoview.exe ({os.path.getsize(viewer)/1e6:.1f} MB)")
    else:
        print("  ! evoview.exe missing - the 3D viewer will not work in this "
              "build (cargo build --release in the evoview checkout)")
    for m in missing:
        print(f"  ! missing entirely: {m}")
    check_catalogue()
    return have, missing


def check_catalogue():
    """Refuse to ship a cars.json that has MODS in it.

    ⚠ tools/cars.json is bundled into the exe and is the list every fresh
    install reads before it has a game to read from. It was captured on a dev
    machine that had car mods installed, so ten mod cars - a Bugatti Bolide, a
    Tesla Model S Plaid, Ferraris - shipped to everyone as real entries. On a
    clean install they showed up in Drive and could not be driven, because no
    such car exists in the game.

    A mod car is easy to tell apart: it lives in its own package, so it has no
    car folder inside the game's content.kspkg. That is what is checked here -
    on a machine with no game installed the check simply says so and passes.
    """
    import json
    cat = os.path.join(TOOLS, "cars.json")
    if not os.path.isfile(cat):
        return
    sys.path.insert(0, HERE)
    try:
        from acecm import kspkg, viewer as vw
        pkg = vw.package()
    except Exception as ex:
        print(f"  catalogue: not checked ({ex})")
        return
    if not pkg or not os.path.isfile(pkg):
        print("  catalogue: not checked (no game install here)")
        return
    sep = chr(92)
    folders = set()
    for path, _s, _o in kspkg.iter_entries(pkg):
        low = path.lower().replace("/", sep)
        parts = low.split(sep)
        if len(parts) > 2 and parts[0] == "content" and parts[1] == "cars":
            folders.add(parts[2])
    try:
        cars = json.load(open(cat, encoding="utf-8"))["cars"]
    except Exception as ex:
        raise SystemExit(f"tools/cars.json is unreadable: {ex}")
    from acecm import carsmap
    presets = set(carsmap.table().get("presets") or {})
    bad = [c for c in cars
           if c.get("name") not in presets and c.get("name") not in folders]
    if bad:
        names = ", ".join(f"{c.get('name')} ({c.get('display_name')})"
                          for c in bad[:12])
        raise SystemExit(
            f"tools/cars.json holds {len(bad)} car(s) the game does not have "
            f"- these are mods and must not ship: {names}")
    print(f"  catalogue: {len(cars)} cars, all present in the game")


_CRT_FILTER = '''
# --- injected by build.py: never ship the Visual C++ runtime -------------
# See the note in build.py. A bundled CRT is inherited by every child
# process ACECM starts and crashed a user's dedicated server.
_CRT = ("vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll",
        "concrt140.dll")
a.binaries = TOC([(n, p, t) for (n, p, t) in a.binaries
                  if os.path.basename(n).lower() not in _CRT])
'''


def _rebuild_without_crt(args):
    """Strip the CRT out of the spec PyInstaller wrote, then build from it."""
    spec = os.path.join(HERE, "ACECM.spec")
    if not os.path.isfile(spec):
        print("  ! no spec to patch - CRT may still be bundled")
        return 0
    src = open(spec, encoding="utf-8").read()
    if "injected by build.py" in src:
        return 0
    if "pyz = PYZ(" not in src:
        print("  ! spec shape unexpected - not patching")
        return 0
    if "import os" not in src.split("\n")[0:6]:
        src = "import os\n" + src
    src = src.replace("pyz = PYZ(", _CRT_FILTER + "\npyz = PYZ(", 1)
    open(spec, "w", encoding="utf-8").write(src)
    print("re-running PyInstaller without the bundled CRT ...")
    r = subprocess.run([sys.executable, "-m", "PyInstaller", "--noconfirm",
                        "--distpath", DIST,
                        "--workpath", os.path.join(HERE, "build_tmp"),
                        spec], cwd=HERE)
    return r.returncode


def build(clean=False):
    have, missing = stage_tools()
    if not have:
        print("nothing staged - refusing to build an exe with no tools")
        return 1
    ver_file, ver = _version_file()
    print(f"version resource: {ver}")
    args = [
        sys.executable, "-m", "PyInstaller", "--noconfirm",
        "--onefile", "--name", "ACECM",
        "--noupx",
        "--version-file", ver_file,
        "--distpath", DIST,
        "--workpath", os.path.join(HERE, "build_tmp"),
        "--specpath", HERE,
        # web assets and helper scripts ride inside the bundle
        "--add-data", f"{os.path.join(HERE, 'acecm', 'web')}{os.pathsep}acecm/web",
        "--add-data", f"{TOOLS}{os.pathsep}tools",
        # ⚠ Console stays ON. This is a server tool: when a dedicated
        # server refuses to start, the console output is how anyone finds
        # out why. The native window is the UI; the console is the log.
        "--console",
        # ⚠ entry must import the package ABSOLUTELY - PyInstaller runs
        # the entry script top-level, where relative imports fail.
        os.path.join(HERE, "launcher.py"),
    ]
    # protobuf and numpy both need help being found inside a frozen build.
    # ⚠ texture2ddecoder and PIL are imported INSIDE a function (so a missing
    # decoder degrades to "no cover" instead of breaking the app), and a lazy
    # import is invisible to PyInstaller's scanner. Without these two named
    # here the frozen build silently ships four track covers instead of
    # nineteen, and nothing in the log says why.
    for mod in ("google.protobuf", "numpy", "capstone", "acecm", "acecm.cli",
            "webview", "webview.platforms.edgechromium", "clr_loader",
            "pythonnet", "cryptography", "websockets",
            "texture2ddecoder", "PIL", "PIL.Image",
            # ⚠ certifi is imported inside version._ssl_context, so the
            # scanner never sees it - and without its cacert.pem a machine
            # whose Windows store cannot build the chain to github.com fails
            # every update check with CERTIFICATE_VERIFY_FAILED. PyInstaller's
            # certifi hook ships the .pem once the module is named here.
            "certifi"):
        args += ["--hidden-import", mod]
    # ⚠ protobuf's well-known types (any_pb2, timestamp_pb2, ...) are GENERATED
    # modules imported at runtime, so PyInstaller's scanner never sees them and
    # naming the package alone does not bring them. Without this the frozen
    # backend dies instantly with
    #   ImportError: cannot import name 'any_pb2' from 'google.protobuf'
    # which never showed up in development because the proxy runs from source
    # there - so the shipped build was broken for everyone but the author.
    args += ["--collect-submodules", "google.protobuf"]
    # ⚠ pywebview's packer hook scans EVERY GUI backend. PyQt5 happens to
    # be installed on the build machine, so 0.6.7 shipped ~84 MB of Qt
    # DLLs the Windows app never loads (it uses Edge WebView2). That is
    # why the exe jumped from 40 MB to 73 MB. Exclude the unused
    # backends; do not uninstall PyQt5 from the machine.
    for mod in ("PyQt5", "PyQt6", "PySide2", "PySide6", "qtpy",
                "PyQt5.QtCore", "PyQt5.QtGui", "PyQt5.QtWidgets",
                "PyQt5.QtNetwork"):
        args += ["--exclude-module", mod]
    if clean:
        args.append("--clean")
    print("running PyInstaller ...")
    r = subprocess.run(args, cwd=HERE)
    if r.returncode:
        return r.returncode
    # ⚠ Second pass, and it is not optional. PyInstaller bundles the Visual
    # C++ runtime and unpacks it into %TEMP%\_MEIxxxxxx, so every program
    # ACECM starts resolves its C runtime out of OUR folder instead of its
    # own. A crash dump of somebody's dedicated server showed it running
    # _MEI133842\VCRUNTIME140.dll and dying with an unhandled C++ exception;
    # the same server from Kunos's own launcher, same machine, same ports,
    # was fine. We bundle 14.42 while a current Windows has 14.51, so the
    # server ran MSVCP140 14.51 against VCRUNTIME140 14.42.
    #
    # There is no command-line switch to drop a bundled DLL, and PyInstaller
    # rewrites the spec on every CLI run - so edit the spec it just wrote and
    # build once more from it.
    #
    # ⚠ Safe HERE specifically: the game and its server both link MSVCP140
    # from System32, which only the VC++ redistributable installs, so anyone
    # who can run AC EVO already has the matched runtime. Not a safe default
    # for an app whose users might not.
    rc = _rebuild_without_crt(args)
    if rc:
        return rc
    exe = os.path.join(DIST, "ACECM.exe")
    if os.path.isfile(exe):
        mb = os.path.getsize(exe) / 1024 / 1024
        print(f"\nOK  {exe}  ({mb:.0f} MB)")
        if missing:
            print(f"  ! {len(missing)} helper script(s) were missing - features "
                  f"using them will not work in this build")
    return 0


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--clean", action="store_true")
    sys.exit(build(ap.parse_args().clean))
