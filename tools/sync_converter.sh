#!/usr/bin/env bash
# Copy the converter (aceconv) into converter/ from its development
# checkout. Only the files tracked at that checkout's HEAD are copied, so
# build output and local scratch never come along; uncommitted changes there
# are refused.
#
#   tools/sync_converter.sh [path to the converter checkout]
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
src="${1:-${ACECONV_SRC:-C:/Projects/aceconv-open}}"
dst="$here/converter"
git -C "$src" rev-parse --verify HEAD >/dev/null
if [ -n "$(git -C "$src" status --porcelain --untracked-files=no)" ]; then
    echo "sync_converter: $src has uncommitted changes - commit them there first" >&2
    exit 1
fi
rev="$(git -C "$src" rev-parse --short=12 HEAD)"
tmp="$(mktemp -d)"
git -C "$src" archive --format=tar HEAD | tar -x -C "$tmp"
# the copied tree must pass this repository's word check before it lands
if ! sh "$here/.githooks/check-tree.sh" "$tmp"; then
    rm -rf "$tmp"
    exit 1
fi
rm -rf "$dst"
mv "$tmp" "$dst"
printf '%s\n' "$rev" > "$dst/UPSTREAM_REV"
echo "converter/ <- $src @ $rev ($(find "$dst" -type f | wc -l | tr -d ' ') files)"
