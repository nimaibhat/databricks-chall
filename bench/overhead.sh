#!/bin/bash
# Splits wall time: docker start/stop overhead, process exit, and an x86 perf profile.
set -u
N=$(nproc)
docker build -q -t sut:head . >/dev/null
flags="--rm --network none --read-only --tmpfs /tmp:rw,size=64m,mode=1777 --cap-drop ALL --security-opt no-new-privileges --pids-limit 4096 --cpus $N --memory 14g --memory-swap 14g --user 1000:1000"
echo "--- empty container (binary exits at once: missing manifest)"
for i in 1 2 3 4 5; do t0=$(date +%s.%N); docker run $flags -e DATA_DIR=/nonexistent sut:head 2>/dev/null; t1=$(date +%s.%N); echo "empty wall $(echo "$t1 - $t0" | bc)"; done
echo "--- native binary (no docker), /usr/bin/time"
id=$(docker create sut:head); docker cp $id:/sessions ./sessions-bin >/dev/null; docker rm $id >/dev/null
for i in 1 2 3 4; do rm -rf /tmp/o; mkdir /tmp/o; /usr/bin/time -f "native wall %e user %U sys %S maxrss %MKB" env NCPU=$N DATA_DIR=$HOME/data OUTPUT_DIR=/tmp/o ./sessions-bin 2>&1 | grep -E "^done|native"; done
echo "--- perf"
sudo apt-get install -y -qq linux-tools-common linux-tools-$(uname -r) >/dev/null 2>&1 || sudo apt-get install -y -qq linux-tools-generic >/dev/null 2>&1
PERF=$(ls /usr/lib/linux-tools/*/perf 2>/dev/null | head -1); [ -z "$PERF" ] && PERF=perf
sudo sysctl -w kernel.perf_event_paranoid=-1 >/dev/null; sudo sysctl -w kernel.kptr_restrict=0 >/dev/null
RUSTFLAGS="-C target-cpu=x86-64-v3" CARGO_PROFILE_RELEASE_DEBUG=true cargo build --release -q 2>&1 | tail -2
rm -rf /tmp/o; mkdir /tmp/o
$PERF stat -e task-clock,cycles,instructions,branch-misses,cache-misses,page-faults,context-switches,cpu-migrations env NCPU=$N DATA_DIR=$HOME/data OUTPUT_DIR=/tmp/o ./target/release/sessions 2>&1 | grep -vE "^(links|phase|sect)" | tail -20
rm -rf /tmp/o; mkdir /tmp/o
$PERF record -q -F 2000 -g -o /tmp/perf.data env NCPU=$N DATA_DIR=$HOME/data OUTPUT_DIR=/tmp/o ./target/release/sessions >/dev/null 2>&1
echo "--- perf top symbols (self)"
$PERF report -i /tmp/perf.data --no-children --stdio --sort dso,sym 2>/dev/null | grep -E "^ +[0-9]" | head -45
echo "--- kernel symbols"
$PERF report -i /tmp/perf.data --no-children --stdio --sort sym --dsos '[kernel.kallsyms]' 2>/dev/null | grep -E "^ +[0-9]" | head -20
