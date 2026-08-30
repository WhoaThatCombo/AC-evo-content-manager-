"""Remote administration, for an ACECM that runs on a box you do not sit at.

Everything a headless server needs already exists in this app - it shares
content over HTTP, deploys tracks, starts and stops servers, and takes uploads
by streaming them to a staging folder. The only reason none of that could be
driven from another machine is that admin routes refuse any peer that is not
loopback, which is exactly the right default for a desktop app and exactly
wrong for a server.

So this is the missing half, and it is deliberately small:

  * a token, generated once and kept in the data folder
  * a check that a request carries it
  * an OFF switch that is on by default only in headless mode

⚠ Loopback stays unauthenticated. The desktop app on the machine itself must
keep working with no token and no prompt - the moment sharing a server means
logging into your own PC, people stop using it. The token guards the network,
not the console.

⚠ The token is a bearer credential over plain HTTP. Anyone who can read the
traffic can replay it, so this is meant for a LAN or a private overlay
(Tailscale, WireGuard), not the open internet. Say so rather than implying a
safety that is not there.
"""
import hmac
import os
import secrets
import stat

from . import config, logs

TOKEN_FILE = os.path.join(config.DATA, "admin_token.txt")
_HEADER = "X-ACECM-Token"


def token(create=True):
    """The admin token, made on first use.

    Kept in a file rather than the config so that config.json stays something
    a user can paste into a bug report.
    """
    try:
        got = open(TOKEN_FILE, encoding="ascii").read().strip()
        if got:
            return got
    except OSError:
        pass
    if not create:
        return ""
    new = secrets.token_urlsafe(24)
    try:
        os.makedirs(config.DATA, exist_ok=True)
        with open(TOKEN_FILE, "w", encoding="ascii") as f:
            f.write(new + "\n")
        # best effort on Windows; the ACL is what really matters and the file
        # already sits under the user's own AppData
        os.chmod(TOKEN_FILE, stat.S_IRUSR | stat.S_IWUSR)
    except OSError as ex:
        logs.LOG.warning("could not save the admin token: %s", ex)
    return new


def rotate():
    """Throw the current token away and issue another."""
    try:
        os.remove(TOKEN_FILE)
    except OSError:
        pass
    return token()


def enabled():
    """Is remote administration allowed at all?"""
    return bool(config.CFG.get("remote_admin"))


def presented(headers, query=None):
    """The token a request carries, from a header, a query or a cookie.

    Three places because three callers: a script uses the header, a link
    someone pastes carries the query, and the web UI stores a cookie so the
    page keeps working across navigations.
    """
    got = (headers.get(_HEADER) or "").strip()
    if got:
        return got
    auth = (headers.get("Authorization") or "").strip()
    if auth.lower().startswith("bearer "):
        return auth[7:].strip()
    if query:
        vals = query.get("token") or []
        if vals and vals[0]:
            return vals[0].strip()
    for part in (headers.get("Cookie") or "").split(";"):
        k, _, v = part.strip().partition("=")
        if k == "acecm_token" and v:
            return v.strip()
    return ""


def ok(headers, query=None):
    """Does this request carry the right token?

    ⚠ Compared with compare_digest. A plain `==` on a secret leaks its length
    and, in principle, its content through timing; there is no reason to write
    the version that does.
    """
    if not enabled():
        return False
    want = token(create=False)
    got = presented(headers, query)
    if not want or not got:
        return False
    return hmac.compare_digest(want, got)


def banner(url, addresses=()):
    """What to print when a headless server comes up.

    `addresses` is [(label, url)] in the order they are worth trying. The
    token is shown ONCE per start, on the console of the machine running it -
    the same trust boundary as the file it came from.
    """
    t = token()
    lines = ["", "  Remote administration is ON.", f"  token: {t}", ""]
    shown = list(addresses) or [("local", url)]
    width = max(len(lbl) for lbl, _ in shown)
    for lbl, u in shown:
        lines.append(f"  {lbl:<{width}}  {u}/?token={t}")
    lines += [
        "",
        # ⚠ ASCII only. This goes to a Windows console, which is cp1252 by
        # default, and a bare print of a non-ASCII character raises
        # UnicodeEncodeError - which killed the whole headless start.
        "  ! Plain HTTP. Fine over Tailscale or a LAN; do not expose this",
        "    straight to the internet.",
        f"  the token is also in {TOKEN_FILE}",
        "",
    ]
    return "\n".join(lines)
