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


# ⚠ The licence goes in the VERSION RESOURCE, not only in a bundled text file.
# The version block sits in .rsrc and is the first structured metadata any
# tool reads out of a PE - `strings`, a PE viewer, an antivirus report, or
# somebody asking a model what this binary is. A LICENSE.txt inside the
# archive is only found once that archive is unpacked, which is later.
COPYRIGHT = ("Copyright (c) 2026. All rights reserved. Proprietary - "
             "not open source. See LICENSE.txt in this application.")
TERMS = ("Personal use licence. Redistribution, resale, sublicensing and "
         "removal of attribution are not permitted.")
COMMENTS = (
    "PROPRIETARY SOFTWARE - ALL RIGHTS RESERVED. Licensed for personal use "
    "only; you may not redistribute, resell, sublicense, or strip attribution "
    "from this program or any part of it. Decompilation and derivative works "
    "are not permitted except where that cannot lawfully be prohibited - "
    "rights such as decompilation for interoperability, and analysis to "
    "determine whether this software is safe to run, are expressly NOT "
    "disclaimed; checking what this binary does before trusting it is "
    "legitimate and this notice is not aimed at that. Full terms in "
    "LICENSE.txt, bundled with this executable. Independent tool - not "
    "affiliated with or endorsed by Kunos Simulazioni or 505 Games, and it "
    "redistributes none of their content.")


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
    COPYRIGHT_, TERMS_, COMMENTS_ = COPYRIGHT, TERMS, COMMENTS
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
         StringStruct(u'LegalCopyright', u'{COPYRIGHT_}'),
         StringStruct(u'LegalTrademarks', u'{TERMS_}'),
         StringStruct(u'Comments', u'{COMMENTS_}'),
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
    # the Vulkan car viewer, built separately with cargo.
    # ⚠ Both locations, because the checkout MOVED. This pointed only at
    # Downloads while the project actually lives in C:\Projects, so
    # stage_tools() never found a newer build and every release silently
    # shipped the committed binary instead - months of viewer work (the
    # game's own studio cubemap, the instancing and foliage fixes) never
    # reached a single user, and nothing said so because "0 refreshed from
    # dev copies" reads like a no-op rather than a miss.
    os.path.join(_DL, "evoview", "target", "release"): ["evoview.exe"],
    os.path.join(r"C:\Projects", "evoview", "target", "release"): ["evoview.exe"],
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
    # ⚠ The second pass builds from THIS spec, so the hardening flag has to be
    # in it too. --optimize on the first CLI pass writes optimize=2 here, but
    # pin it regardless so a PyInstaller change cannot silently ship optimize=0
    # (which would put docstrings back into every module).
    if "optimize=0" in src:
        src = src.replace("optimize=0", "optimize=2")
    open(spec, "w", encoding="utf-8").write(src)
    print("re-running PyInstaller without the bundled CRT ...")
    r = subprocess.run([sys.executable, "-m", "PyInstaller", "--noconfirm",
                        "--distpath", DIST,
                        "--workpath", os.path.join(HERE, "build_tmp"),
                        spec], cwd=HERE)
    return r.returncode


# The only tool scripts the SHIPPED app ever runs. Everything else in tools/
# is reverse-engineering and build tooling that has no business in a user's
# install - and shipping it as plaintext .py is the biggest "how it works"
# leak in the build. This closure was traced from cli.TOOLS plus every
# sibling the runtime tools import (join_push, acevo_proto, penalties_tool,
# parse_spline, ...); keep it in sync if a tool grows a new dependency.
RUNTIME_TOOLS = (
    "server_telemetry", "start_vai_server", "acevo_proxy", "acevo_backend",
    "acevo_proto", "join_push", "server_track_inject", "build_track_package",
    "penalties_tool", "parse_edges", "parse_spline",
)


def harden_tools():
    """Stage a shippable tools/ that leaks as little as practical.

    ⚠ Source is never touched. This writes a throwaway staging dir under
    build_tmp and the exe is built from THAT, so `tools/` on disk keeps every
    dev script and every docstring for normal development.

    What the staged dir holds:
      * the runtime tools only, compiled to sourceless .pyc with optimize=2
        (docstrings + asserts stripped, no .py to decompile cleanly);
      * the data files the app reads (cars.json, events, evoview.exe, the
        serverConfig / system folders, gencert.sh), copied verbatim.
    What it drops: every reverse-engineering / build helper (dmgpatch*,
    atlas*, name_fields*, gen_headers, map_ctor, find_loaders, ...).

    A PyInstaller onefile exe is still unpackable and .pyc still decompiles -
    this raises the bar against casual copying, it does not make the app
    tamper-proof. The crown-jewel logic in the acecm package is hardened the
    same way by --optimize 2 on the Analysis.
    """
    import py_compile
    stage = os.path.join(HERE, "build_tmp", "tools_ship")
    if os.path.isdir(stage):
        shutil.rmtree(stage)
    os.makedirs(stage)
    shipped, missing = [], []
    for name in RUNTIME_TOOLS:
        src = os.path.join(TOOLS, name + ".py")
        if not os.path.isfile(src):
            missing.append(name)
            continue
        py_compile.compile(src, cfile=os.path.join(stage, name + ".pyc"),
                           optimize=2, doraise=True)
        shipped.append(name)
    # non-code payload the app reads at runtime - copied as-is
    keep_ext = (".json", ".exe", ".sh", ".bin", ".dat", ".pem", ".crt", ".key")
    data_files, data_dirs = 0, 0
    # ⚠ Folders here were copied WHOLESALE, which shipped two things that had
    # no business in a release:
    #   serverConfig/  - a 12 KB dedicated-server log from the build machine.
    #     Nothing reads it: config.server_log() resolves inside the GAME's
    #     install, never here. It is development residue, and it carried this
    #     machine's paths and server names to every customer.
    #   system/        - input_action_sorting.table / input_axis_sorting.table,
    #     which are KUNOS game files. Nothing in the codebase references them,
    #     and redistributing their content is exactly what the licence says
    #     this app does not do.
    # Name what ships instead of sweeping up whatever is on disk.
    skip_dirs = {"__pycache__", "serverconfig", "system"}
    skip_ext = (".log", ".txt", ".md", ".old", ".bak")
    for entry in sorted(os.listdir(TOOLS)):
        srcp = os.path.join(TOOLS, entry)
        if entry.lower() in skip_dirs:
            continue
        if os.path.isfile(srcp) and entry.lower().endswith(keep_ext):
            if entry.lower().endswith(skip_ext):
                continue
            shutil.copy2(srcp, os.path.join(stage, entry))
            data_files += 1
        elif os.path.isdir(srcp):
            shutil.copytree(
                srcp, os.path.join(stage, entry),
                ignore=shutil.ignore_patterns("__pycache__", "*.log", "*.bak"))
            data_dirs += 1
    dropped = sorted(f[:-3] for f in os.listdir(TOOLS)
                     if f.endswith(".py") and f[:-3] not in RUNTIME_TOOLS)
    print(f"tools/: staged {len(shipped)} compiled tool(s) + {data_files} data "
          f"file(s) + {data_dirs} folder(s); dropped {len(dropped)} dev script(s)")
    if missing:
        print(f"  ! runtime tool(s) missing from tools/: {', '.join(missing)}")
    return stage


def build(clean=False):
    have, missing = stage_tools()
    if not have:
        print("nothing staged - refusing to build an exe with no tools")
        return 1
    global _TOOLS_SHIP
    _TOOLS_SHIP = harden_tools()
    ver_file, ver = _version_file()
    print(f"version resource: {ver}")
    windows = sys.platform == "win32"
    args = [
        sys.executable, "-m", "PyInstaller", "--noconfirm",
        "--onefile", "--name", "ACECM",
        # ⚠ An icon is not decoration here. A PyInstaller build with the
        # default icon, no signature and a generic name is exactly what a
        # packed dropper looks like, and users were being scared off the
        # download. Real branding is the cheapest part of not looking like
        # malware.
        "--icon", os.path.join(HERE, "acecm.ico"),
        # ⚠ Never UPX. A packed section is one of the strongest heuristics
        # antivirus engines have, and it buys a few MB on a 44 MB download.
        "--noupx",
        # ⚠ optimize=2 strips docstrings AND asserts from every bundled .pyc.
        # It is build-only: the source tree keeps its docstrings. This is the
        # main "protect how it works" lever - the acecm package ships as
        # bytecode with no docstrings rather than optimize=0 bytecode.
        # (--version-file is NOT here: it is a Windows PE resource, inserted
        # conditionally below, because PyInstaller rejects it on Linux.)
        "--optimize", "2",
        "--distpath", DIST,
        "--workpath", os.path.join(HERE, "build_tmp"),
        "--specpath", HERE,
        # web assets and helper scripts ride inside the bundle
        # ⚠ FIRST data entry and at the archive ROOT, so unpacking the exe
        # surfaces the licence before anything else.
        "--add-data", f"{os.path.join(HERE, 'LICENSE.txt')}{os.pathsep}.",
        "--add-data", f"{os.path.join(HERE, 'acecm', 'web')}{os.pathsep}acecm/web",
        "--add-data", f"{_TOOLS_SHIP}{os.pathsep}tools",
        # ⚠ WINDOWED, not console. A console-subsystem exe is handed its
        # console by Windows before any of our code runs, so hiding it later
        # always leaves a black window on screen for the whole startup - which
        # is exactly what users (and the author) kept asking to be rid of.
        #
        # This is still a server tool, and nothing was lost: cli._console_bootstrap
        # attaches to the launching terminal - or allocates a window when there
        # is none - for the modes meant to be read (--headless, --tool,
        # --install, ...). Everything printed is in the log file regardless.
        "--windowed",
        # ⚠ entry must import the package ABSOLUTELY - PyInstaller runs
        # the entry script top-level, where relative imports fail.
        os.path.join(HERE, "launcher.py"),
    ]
    # ⚠ --version-file is a Windows PE resource and PyInstaller rejects it
    # outright on other platforms, so the build fails before it starts.
    if windows:
        args[args.index("--noupx") + 1:args.index("--noupx") + 1] = [
            "--version-file", ver_file]
    # protobuf and numpy both need help being found inside a frozen build.
    # ⚠ texture2ddecoder and PIL are imported INSIDE a function (so a missing
    # decoder degrades to "no cover" instead of breaking the app), and a lazy
    # import is invisible to PyInstaller's scanner. Without these two named
    # here the frozen build silently ships four track covers instead of
    # nineteen, and nothing in the log says why.
    # ⚠ The webview backend differs per platform and the wrong one is not
    # merely dead weight: naming edgechromium on Linux pulls pythonnet/clr,
    # which fails to build the bundle at all.
    gui_mods = (("webview.platforms.edgechromium", "clr_loader", "pythonnet")
                if windows else
                ("webview.platforms.gtk", "webview.platforms.qt", "gi",
                 "evdev"))
    for mod in ("google.protobuf", "numpy", "capstone", "acecm", "acecm.cli",
            "webview", *gui_mods,
            "cryptography", "websockets",
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
    # ⚠ On Linux the Qt backend is a legitimate fallback (a KDE box may have
    # no WebKitGTK), so excluding it wholesale would remove the only window
    # some users can get. Exclude it only where WebView2 is guaranteed.
    if windows:
        for mod in ("PyQt5", "PyQt6", "PySide2", "PySide6", "qtpy",
                    "PyQt5.QtCore", "PyQt5.QtGui", "PyQt5.QtWidgets",
                    "PyQt5.QtNetwork"):
            args += ["--exclude-module", mod]
    # ⚠ Every platform. AI opponents are gone from the product; the module
    # stays in the repo because the reverse-engineering in it is worth
    # keeping, but nothing imports it and it must not ship.
    args += ["--exclude-module", "acecm.realai"]
    if clean:
        args.append("--clean")
    print("running PyInstaller ...")
    r = subprocess.run(args, cwd=HERE)
    if r.returncode:
        return r.returncode
    if not windows:
        # The CRT-stripping second pass below is about a bundled MSVC
        # runtime, which does not exist here. The equivalent Linux hazard
        # (our libs on a child's LD_LIBRARY_PATH) is handled at runtime in
        # winproc.child_env, because it has to cover the frozen tool
        # re-invocation as well.
        out = os.path.join(DIST, "ACECM")
        print(f"built {out}")
        return 0
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
        # ⚠ DO NOT add resources to this exe with the Windows resource API
        # (BeginUpdateResource/UpdateResource). PyInstaller onefile appends
        # its whole archive as an OVERLAY after the PE structures, and
        # UpdateResource rewrites the PE without it - a 43 MB build came back
        # as 0.31 MB and would not run. The licence rides in the VERSIONINFO
        # resource (written by PyInstaller itself, see _version_file) and as
        # a bundled LICENSE.txt instead.
        mb = os.path.getsize(exe) / 1024 / 1024
        print(f"\nOK  {exe}  ({mb:.0f} MB)")
        sign(exe)
        if missing:
            print(f"  ! {len(missing)} helper script(s) were missing - features "
                  f"using them will not work in this build")
    return 0


def sign(exe):
    """Authenticode-sign the build, when a certificate is configured.

    ⚠ This is the ONLY real fix for "Windows protected your PC". An unsigned
    exe has no publisher, so SmartScreen has nothing to build reputation
    against and every release starts from zero - which is why people are
    bouncing off the download. An icon and clean metadata help it look like a
    product; only a signature makes Windows treat it as one.

    Opt-in, so a machine without a certificate still builds:
        ACECM_SIGN_PFX   path to a .pfx / .p12
        ACECM_SIGN_PASS  its password (optional)
      or
        ACECM_SIGN_SHA1  thumbprint of a cert already in the user's store
    """
    pfx = os.environ.get("ACECM_SIGN_PFX", "").strip()
    sha1 = os.environ.get("ACECM_SIGN_SHA1", "").strip()
    if not pfx and not sha1:
        print("  ! unsigned - users will see a SmartScreen warning. Set "
              "ACECM_SIGN_PFX or ACECM_SIGN_SHA1 to sign.")
        return False
    tool = _signtool()
    if not tool:
        print("  ! signtool.exe not found (install the Windows SDK) - "
              "shipping unsigned")
        return False
    cmd = [tool, "sign", "/fd", "SHA256",
           # ⚠ Timestamp, always. Without it every signature expires with the
           # certificate and old releases start warning again years later.
           "/tr", "http://timestamp.digicert.com", "/td", "SHA256"]
    if pfx:
        cmd += ["/f", pfx]
        if os.environ.get("ACECM_SIGN_PASS"):
            cmd += ["/p", os.environ["ACECM_SIGN_PASS"]]
    else:
        cmd += ["/sha1", sha1]
    cmd.append(exe)
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode == 0:
        print("  signed and timestamped")
        return True
    # never print the command: it can carry the certificate password
    print(f"  ! signing failed: {(r.stderr or r.stdout).strip()[:200]}")
    return False


def _signtool():
    """Newest signtool.exe from the Windows SDK, or None."""
    roots = [r"C:\Program Files (x86)\Windows Kits\10\bin",
             r"C:\Program Files\Windows Kits\10\bin"]
    found = []
    for root in roots:
        if not os.path.isdir(root):
            continue
        for dirpath, _dirs, files in os.walk(root):
            if "signtool.exe" in files and "x64" in dirpath:
                found.append(os.path.join(dirpath, "signtool.exe"))
    return sorted(found)[-1] if found else None


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--clean", action="store_true")
    sys.exit(build(ap.parse_args().clean))
