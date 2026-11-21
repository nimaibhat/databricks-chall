#!/bin/bash
set -u
source bench/config.env
[ -z "${PERF_REF:-}" ] && exit 0
N=$(nproc); DS=$HOME/${PERF_DATA:-d20}
sudo apt-get install -y -qq linux-tools-common linux-tools-$(uname -r) >/dev/null 2>&1 || sudo apt-get install -y -qq linux-tools-generic >/dev/null 2>&1
PERF=$(ls /usr/lib/linux-tools/*/perf 2>/dev/null | head -1); [ -z "$PERF" ] && PERF=perf
sudo sysctl -w kernel.perf_event_paranoid=-1 >/dev/null; sudo sysctl -w kernel.kptr_restrict=0 >/dev/null
rm -rf /tmp/t && git worktree prune && git worktree add -f /tmp/t "$PERF_REF" -q && cd /tmp/t
RUSTFLAGS="-C target-cpu=x86-64-v3 -C target-feature=+crt-static" CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release -q 2>&1 | tail -2
B=./target/release/sessions
for n in $N 1; do
  for i in 1 2; do rm -rf /tmp/o; mkdir /tmp/o; /usr/bin/time -f "PF ncpu=$n wall %e user %U sys %S maxrss %MKB minflt %R majflt %F" env NCPU=$n DATA_DIR=$DS OUTPUT_DIR=/tmp/o $B 2>&1 | grep -E "^(links|phase|done|PF)" | sed "s/^/PH$n /"; done
done
rm -rf /tmp/o; mkdir /tmp/o
$PERF stat -e task-clock,cycles,instructions,branches,branch-misses,cache-misses,L1-dcache-load-misses,dTLB-load-misses,page-faults,context-switches env NCPU=$N DATA_DIR=$DS OUTPUT_DIR=/tmp/o $B 2>&1 | grep -vE "^(links|phase|done|rusage)" | sed 's/^/PS /'
rm -rf /tmp/o; mkdir /tmp/o
$PERF record -q -F 3000 -o /tmp/perf.data env NCPU=$N DATA_DIR=$DS OUTPUT_DIR=/tmp/o $B >/dev/null 2>&1
echo "--- perf symbols"
$PERF report -i /tmp/perf.data --no-children --stdio --sort dso,sym 2>/dev/null | grep -E "^ +[0-9]" | head -40 | sed 's/^/PY /'
echo "--- perf srclines"
$PERF report -i /tmp/perf.data --no-children --stdio --sort srcline 2>/dev/null | grep -E "^ +[0-9]" | head -90 | sed 's/^/PL /'
