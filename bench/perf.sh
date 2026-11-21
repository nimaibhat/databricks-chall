#!/bin/bash
set -u
N=$(nproc)
sudo apt-get install -y -qq linux-tools-common linux-tools-$(uname -r) >/dev/null 2>&1 || sudo apt-get install -y -qq linux-tools-generic >/dev/null 2>&1
PERF=$(ls /usr/lib/linux-tools/*/perf 2>/dev/null | head -1); [ -z "$PERF" ] && PERF=perf
sudo sysctl -w kernel.perf_event_paranoid=-1 >/dev/null; sudo sysctl -w kernel.kptr_restrict=0 >/dev/null
RUSTFLAGS="-C target-cpu=x86-64-v3" CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release -q 2>&1 | tail -2
rm -rf /tmp/o; mkdir /tmp/o
$PERF record -q -F 4000 -o /tmp/perf.data env NCPU=$N DATA_DIR=$HOME/data OUTPUT_DIR=/tmp/o ./target/release/sessions >/dev/null 2>&1
echo "--- perf symbols"
$PERF report -i /tmp/perf.data --no-children --stdio --sort sym 2>/dev/null | grep -E "^ +[0-9]" | head -25
echo "--- perf srclines"
$PERF report -i /tmp/perf.data --no-children --stdio --sort srcline 2>/dev/null | grep -E "^ +[0-9]" | head -70
