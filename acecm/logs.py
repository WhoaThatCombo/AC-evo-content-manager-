"""Logging: make failures explain themselves.

A shipped build has no console to watch, and the interesting failures happen in
child processes (a dedicated server that will not start, a tracker that dies on
attach) or inside an HTTP handler where an exception used to become a 500 with
a one-line message and no traceback.

So everything lands in one rotating file per run:

    <data>/logs/acecm.log        current
    <data>/logs/acecm.log.1..5   previous runs / rotations

Captured here:
  * every API error, with the full traceback and the request that caused it
  * uncaught exceptions on ANY thread (threading.excepthook), which otherwise
    print to a console nobody sees and vanish
  * process launches and exits, with argv and exit code
  * the resolved paths at startup - the single most useful line when someone
    reports "it lost my profiles"

⚠ The startup banner records the DATA directory it actually resolved. Windows
MSIX/Store virtualization can redirect %LOCALAPPDATA% into a per-package
LocalCache, so two copies of the app can silently use different folders. That
cost an hour once; the banner makes it obvious.
"""
import logging
import logging.handlers
import os
import platform
import sys
import threading
import traceback

LOG = logging.getLogger("acecm")
_ready = False


def log_dir():
    from . import config
    d = os.path.join(config.DATA, "logs")
    os.makedirs(d, exist_ok=True)
    return d


def setup(level=logging.INFO):
    """Install file + console logging. Safe to call more than once."""
    global _ready
    if _ready:
        return LOG
    from . import config, version

    LOG.setLevel(level)
    fmt = logging.Formatter(
        "%(asctime)s %(levelname)-7s %(threadName)-12s %(name)s: %(message)s",
        "%Y-%m-%d %H:%M:%S")

    path = os.path.join(log_dir(), "acecm.log")
    fh = logging.handlers.RotatingFileHandler(
        path, maxBytes=4 << 20, backupCount=5, encoding="utf-8")
    fh.setFormatter(fmt)
    LOG.addHandler(fh)

    ch = logging.StreamHandler(sys.stderr)
    ch.setFormatter(fmt)
    LOG.addHandler(ch)

    # Uncaught exceptions, on the main thread and every other one. Without the
    # threading hook a crashed background thread just disappears.
    def _hook(exc_type, exc, tb):
        LOG.critical("UNCAUGHT %s: %s\n%s", exc_type.__name__, exc,
                     "".join(traceback.format_exception(exc_type, exc, tb)))
    sys.excepthook = _hook

    def _thook(args):
        LOG.critical("UNCAUGHT in thread %s: %s: %s\n%s",
                     getattr(args.thread, "name", "?"),
                     args.exc_type.__name__, args.exc_value,
                     "".join(traceback.format_exception(
                         args.exc_type, args.exc_value, args.exc_traceback)))
    threading.excepthook = _thook

    LOG.info("=" * 72)
    LOG.info("%s v%s starting", version.NAME, version.VERSION)
    LOG.info("frozen     : %s", config.FROZEN)
    LOG.info("executable : %s", sys.executable)
    LOG.info("python     : %s", sys.version.split()[0])
    LOG.info("windows    : %s", platform.platform())
    LOG.info("data dir   : %s", config.DATA)
    LOG.info("log file   : %s", path)
    LOG.info("server dir : %s", config.server_dir() or "(not found)")
    LOG.info("tools dir  : %s", config.tools_dir())
    # See the module docstring: this is how a virtualized %LOCALAPPDATA% gets
    # noticed instead of looking like lost data.
    if "\\Packages\\" in config.DATA and "LocalCache" in config.DATA:
        LOG.warning("DATA is inside an MSIX/Store LocalCache - this app is "
                    "running in a packaged container and its %%LOCALAPPDATA%% "
                    "is redirected. Launched from Explorer it would use %s",
                    os.path.join(os.environ.get("LOCALAPPDATA", ""), "ACECM"))
    _ready = True
    return LOG


def exception(where, exc, **context):
    """Log a failure with its traceback and whatever context we have."""
    LOG.error("%s failed: %s: %s%s\n%s", where, type(exc).__name__, exc,
              ("  " + repr(context)) if context else "",
              "".join(traceback.format_exception(type(exc), exc,
                                                 exc.__traceback__)))


def launched(what, argv, pid=None, **extra):
    LOG.info("launched %s pid=%s%s\n    argv: %s", what, pid,
             ("  " + repr(extra)) if extra else "", " ".join(map(str, argv)))


def tail(lines=200, which="acecm.log"):
    """Recent log lines, for the UI."""
    p = os.path.join(log_dir(), os.path.basename(which))
    if not os.path.isfile(p):
        return {"ok": False, "error": f"no log at {p}"}
    try:
        with open(p, encoding="utf-8", errors="replace") as f:
            got = f.readlines()[-int(lines):]
    except OSError as ex:
        return {"ok": False, "error": str(ex)}
    return {"ok": True, "file": p, "lines": [l.rstrip("\n") for l in got]}


def files():
    d = log_dir()
    out = []
    for f in sorted(os.listdir(d)):
        p = os.path.join(d, f)
        out.append({"name": f, "size": os.path.getsize(p),
                    "mtime": int(os.path.getmtime(p))})
    return {"dir": d, "files": out}
