"""Release version and signature check, shared by release_sign.py,
publish_release.py and build.py.

The version is the one COMPILED INTO the exe (native/Cargo.toml) - the
in-app updater compares release tags against it.

What is signed (release_message) and the pinned key must stay exactly in
step with native/src/updater.rs, which verifies every download against them.
"""
import os
import re

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def _cargo_version():
    with open(os.path.join(HERE, "native", "Cargo.toml"), encoding="utf-8") as f:
        m = re.search(r'(?m)^version\s*=\s*"([^"]+)"', f.read())
    return m.group(1) if m else ""


VERSION = _cargo_version()

# ---- release signatures ---------------------------------------------------
# ⚠ The SHA-256 the updater checks comes from the SAME GitHub release as the
# exe, so on its own it only catches a corrupted download - whoever can
# replace the exe (a hijacked account or token) can replace its checksum too.
# Every release is also signed with an Ed25519 key that never touches GitHub
# (tools/release_sign.py), and the updater refuses anything whose <asset>.sig
# does not verify against one of these pinned public keys.
# More than one entry = key rotation: ship the new key alongside the old,
# sign with the new one, drop the old a few releases later.
RELEASE_PUBKEYS = (
    "ScPZslRHuUNMFcqqHc6TG3My2SgS/EsY4THCkv2gs6A=",
)


def release_message(asset, ver, sha256):
    """What a release signature covers.

    The version is in it so an OLD signed build cannot be served as an
    update (a downgrade to a known-bad version would otherwise verify); the
    asset name so the Windows and Linux binaries cannot be swapped.
    """
    return ("ACECM-release-v1\n" + str(asset).strip().lower() + "\n"
            + str(ver).strip().lstrip("vV") + "\n"
            + str(sha256).strip().lower() + "\n").encode("ascii")


def verify_release(asset, ver, sha256, sig_text):
    """(ok, reason). Never raises."""
    import base64
    try:
        from cryptography.exceptions import InvalidSignature
        from cryptography.hazmat.primitives.asymmetric.ed25519 import (
            Ed25519PublicKey)
    except Exception as ex:                        # noqa: BLE001
        return False, f"signature support missing ({ex})"
    try:
        sig = base64.b64decode((sig_text or "").strip(), validate=True)
    except Exception:                              # noqa: BLE001
        return False, "the signature file is not valid base64"
    if len(sig) != 64:
        return False, "no signature for this release"
    msg = release_message(asset, ver, sha256)
    for k in RELEASE_PUBKEYS:
        try:
            Ed25519PublicKey.from_public_bytes(base64.b64decode(k)).verify(
                sig, msg)
            return True, ""
        except InvalidSignature:
            continue
        except Exception as ex:                    # noqa: BLE001
            return False, f"bad pinned key ({ex})"
    return False, "the signature does not match ACECM's release key"
