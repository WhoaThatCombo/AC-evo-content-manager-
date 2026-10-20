#!/bin/sh
# Whole-tree word check (CI, and tools/sync_converter.sh on an incoming copy).
#   sh .githooks/check-tree.sh            the tracked files of this repository
#   sh .githooks/check-tree.sh <dir>      every file under <dir>
here="$(cd "$(dirname "$0")" && pwd)"
pat=$(sh "$here/forbidden.sh")
allow=$(grep -v '^#' "$here/allow.txt" | grep -v '^$')
if [ -n "$1" ]; then
    hits=$(grep -r -I -n -i -E "$pat" "$1" 2>/dev/null)
    names=$(cd "$1" && find . | grep -i -E "$pat")
else
    cd "$here/.."
    hits=$(git grep -I -n -i -E "$pat" -- . $(for f in $allow; do printf ':(exclude)%s ' "$f"; done) 2>/dev/null)
    names=$(git ls-files | grep -i -E "$pat")
fi
if [ -n "$hits$names" ]; then
    echo "forbidden words found:" >&2
    [ -n "$hits" ] && echo "$hits" >&2
    [ -n "$names" ] && echo "file names: $names" >&2
    exit 1
fi
echo "word check: clean"
exit 0
