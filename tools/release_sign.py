"""Sign ACECM release binaries so the self-updater can trust them.

    python tools/release_sign.py keygen [KEYFILE]
    python tools/release_sign.py sign FILE --version 1.0.20 [--asset NAME] [--key KEYFILE]
    python tools/release_sign.py verify FILE --version 1.0.20 [--asset NAME]

Why: the updater used to trust ACECM.exe.sha256, which comes from the SAME
GitHub release as the exe - it catches a corrupted download, not a swapped
release (a hijacked account or token could upload both). The updater now
also needs FILE.sig: an Ed25519 signature made with a private key that never
touches GitHub, checked against public keys pinned in acecm/version.py.

What is signed is version.release_message(): asset name + version + sha256.
Binding the version stops an OLD signed build being served as an "update";
binding the asset name stops the Windows and Linux binaries being swapped.

⚠ Development only. Never shipped: build.py ships an explicit whitelist of
runtime tools, and this is not on it. The private key file must never be
committed, uploaded or synced - keep it out of the repo and OneDrive, and keep
an OFFLINE backup (USB). Losing it means existing installs can only be moved
to a new key by a release signed with the old one.
"""
import argparse
import base64
import hashlib
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from acecm import version  # noqa: E402

from cryptography.hazmat.primitives import serialization  # noqa: E402
from cryptography.hazmat.primitives.asymmetric.ed25519 import (  # noqa: E402
    Ed25519PrivateKey)

DEFAULT_KEY = os.path.join(os.path.expanduser("~"), ".acecm-release",
                           "release_ed25519.pem")


def _sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def keygen(keyfile):
    if os.path.exists(keyfile):
        sys.exit(f"refusing to overwrite an existing key: {keyfile}")
    os.makedirs(os.path.dirname(keyfile), exist_ok=True)
    key = Ed25519PrivateKey.generate()
    pem = key.private_bytes(serialization.Encoding.PEM,
                            serialization.PrivateFormat.PKCS8,
                            serialization.NoEncryption())
    with open(keyfile, "wb") as f:
        f.write(pem)
    pub = key.public_key().public_bytes(serialization.Encoding.Raw,
                                        serialization.PublicFormat.Raw)
    print(f"private key: {keyfile}  (back it up OFFLINE; never commit it)")
    print("public key for version.RELEASE_PUBKEYS:")
    print("    " + base64.b64encode(pub).decode())


def sign(path, ver, asset, keyfile):
    with open(keyfile, "rb") as f:
        key = serialization.load_pem_private_key(f.read(), password=None)
    sha = _sha256(path)
    sig = key.sign(version.release_message(asset, ver, sha))
    out = path + ".sig"
    with open(out, "w", encoding="ascii") as f:
        f.write(base64.b64encode(sig).decode() + "\n")
    print(f"signed {asset} v{ver} sha256 {sha} -> {out}")
    # never hand back a signature the shipped verifier would reject
    ok, why = version.verify_release(asset, ver, sha,
                                     open(out, encoding="ascii").read())
    if not ok:
        sys.exit(f"!! the app would REJECT this signature: {why}")
    print("verified against the pinned public key(s)")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    k = sub.add_parser("keygen")
    k.add_argument("keyfile", nargs="?", default=DEFAULT_KEY)
    for name in ("sign", "verify"):
        s = sub.add_parser(name)
        s.add_argument("file")
        s.add_argument("--version", required=True)
        s.add_argument("--asset", default=None,
                       help="asset name (default: the file's own name)")
        if name == "sign":
            s.add_argument("--key", default=DEFAULT_KEY)
    a = ap.parse_args()
    if a.cmd == "keygen":
        return keygen(a.keyfile)
    asset = a.asset or os.path.basename(a.file)
    if a.cmd == "sign":
        return sign(a.file, a.version, asset, a.key)
    sig = open(a.file + ".sig", encoding="ascii").read()
    ok, why = version.verify_release(asset, a.version, _sha256(a.file), sig)
    print("OK" if ok else f"REJECTED: {why}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
