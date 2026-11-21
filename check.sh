#!/bin/sh
# Local regression: fixtures, dev answer, and big-set baseline (if present).
# usage: ./check.sh [BIN]   (env S = dir holding dev/, ans/dev-answer, big/, ref_big/)
set -u
BIN=${1:-./target/release/sessions}
S=${S:-/private/tmp/claude-501/-Users-nimaibhat-Documents-databricks-chall/9a86209f-9a0f-40b2-8184-ee4c81fa51e7/scratchpad}
python3 scripts/run_fixtures.py fixtures -- "$BIN" 2>&1 | grep -v "^ok" | tail -5
rm -rf "$S/out"; DATA_DIR=$S/dev OUTPUT_DIR=$S/out "$BIN" 2>/dev/null && python3 scripts/compare.py "$S/out" "$S/ans/dev-answer" | tail -1
if [ -d "$S/ref_big" ]; then
  rm -rf "$S/outb"; DATA_DIR=$S/big OUTPUT_DIR=$S/outb "$BIN" 2>&1 | tr '\n' ' '; echo
  python3 scripts/compare.py "$S/outb" "$S/ref_big" | tail -1
fi
