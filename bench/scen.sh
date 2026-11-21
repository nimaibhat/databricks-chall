#!/bin/bash
set -u
ref=$(grep -v '^#' bench/variants.txt | tail -1 | cut -d'|' -f2)
rm -rf /tmp/t && git worktree prune && git worktree add -f /tmp/t "$ref" -q && cd /tmp/t
RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --release -q 2>&1 | tail -2
for i in 1 2; do BENCH=2 NCPU=1 DATA_DIR=$HOME/data OUTPUT_DIR=/tmp/ob ./target/release/sessions 2>&1 | grep -E "ns/line" | sed 's/^/MB /'; echo "MB --"; done
echo "CALLS in parse_events closure:"
objdump -d --no-show-raw-insn -C target/release/sessions | awk '/^[0-9a-f]+ <.*>:$/{cur=$0} /call/{ if (cur ~ /parse_events|parse_clicks/) { sub(/.*call +/,""); c[cur" -> "$0]++ } } END{for(k in c) print "CALL", c[k], k}' | sort | cut -c1-230 | head -60
