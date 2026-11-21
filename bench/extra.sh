#!/bin/bash
# Stage timing: parse cut off after stage n (diag feature), native, 1 and N threads.
set -u
source bench/config.env
N=$(nproc)
if [ -n "${DIAG_REF:-}" ]; then
rm -rf /tmp/t && git worktree prune && git worktree add -f /tmp/t "$DIAG_REF" -q && cd /tmp/t
RUSTFLAGS="-C target-cpu=x86-64-v3 -C target-feature=+crt-static" cargo build --release -q --features diag 2>&1 | tail -2
for n in 1 $N; do
  for rep in 1 2; do
    for st in ${DIAG_STAGES:-1 2 3 4 5 6 0}; do
      rm -rf /tmp/o; mkdir /tmp/o
      echo -n "DG ncpu=$n stage=$st ${DIAG_ENV:-} : "
      env ${DIAG_ENV:-} STAGE=$st NCPU=$n DATA_DIR=$HOME/d20 OUTPUT_DIR=/tmp/o ./target/release/sessions 2>&1 | grep "^phase2" | sed -E 's/phase2 parse [^ ]+ //'
    done
  done
done
fi
# Cachegrind on the 1M set (instruction counts per line), normal build with line tables
if [ -n "${CG_REF:-}" ]; then
  sudo apt-get install -y -qq valgrind >/dev/null 2>&1
  G=$(dirname $(find ~/gen -name Cargo.toml | head -1))
  $G/target/release/generate --seed 1 --events 1000000 --clicks 250000 --output ~/dev >/dev/null 2>&1
  echo "CG lines: $(cat ~/dev/events/*.csv | wc -l) events $(cat ~/dev/clicks/*.csv | wc -l) clicks"
  cd "$GITHUB_WORKSPACE"; rm -rf /tmp/t2; git worktree prune; git worktree add -f /tmp/t2 "$CG_REF" 2>&1 | tail -2; cd /tmp/t2 || echo "CG cd failed"
  pwd; ls | head -5
  RUSTFLAGS="-C target-cpu=x86-64-v3" CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release --features diag 2>&1 | tail -3
  which valgrind cg_annotate || sudo apt-get install -y valgrind 2>&1 | tail -2
  for st in ${CG_STAGES:-0}; do
    rm -rf /tmp/o; mkdir /tmp/o
    STAGE=$st NCPU=1 DATA_DIR=$HOME/dev OUTPUT_DIR=/tmp/o valgrind --tool=cachegrind --branch-sim=yes --cache-sim=no --cachegrind-out-file=/tmp/cg.out ./target/release/sessions 2>&1 | grep -E "I +refs|Branches|Mispred|rror|valgrind:" | sed "s/^/CG$st /"
    (cg_annotate /tmp/cg.out --show=Ir,Bcm --annotate=yes > /tmp/ann.txt 2>/tmp/ann.err || cg_annotate /tmp/cg.out > /tmp/ann.txt 2>>/tmp/ann.err); head -5 /tmp/ann.err | sed 's/^/CGE /'; wc -l /tmp/ann.txt | sed 's/^/CGE /'
    grep -nE "^-- (Auto|User)-annotated|annotated source" /tmp/ann.txt | head -8 | sed "s/^/CGR$st /"
    awk '/annotated source/{f++} f>=1 && NR%1==0' /tmp/ann.txt | grep -E "^ *[0-9,]{9,}" | sort -t'(' -k1,1 -r | head -5 | cut -c1-200 | sed "s/^/CGS$st /"
    awk '/^-- Annotated source file: /{f=$NF; sub(/.*\//,"",f); next} /^-- /{ if ($0 !~ /Annotated source file/) f="" } /^ *[0-9,]+ \(/{n=$1; gsub(/,/,"",n); if (n+0>=1200000 && f!="") {l=$0; gsub(/ +/," ",l); printf "%d %s | %s\n", n, f, substr(l,1,170)}}' /tmp/ann.txt | sort -rn | head -140 | sed "s/^/CGL$st /"
  done
fi

# I/O mode comparison, native, N threads: phase2 line + rusage
if [ -n "${IO_REF:-}" ]; then
  cd "$GITHUB_WORKSPACE"; rm -rf /tmp/t3; git worktree prune; git worktree add -f /tmp/t3 "$IO_REF" 2>&1 | tail -1; cd /tmp/t3
  RUSTFLAGS="-C target-cpu=x86-64-v3 -C target-feature=+crt-static" cargo build --release 2>&1 | tail -1
  run() { rm -rf /tmp/o; mkdir /tmp/o; env "$@" NCPU=$N DATA_DIR=$HOME/d20 OUTPUT_DIR=/tmp/o ./target/release/sessions 2>&1 | grep -E "^(phase2|rusage)" | sed -E 's/\(parse only//; s/rusage: //' | tr '\n' ' ' | sed "s/^/IO [$*] /"; echo; }
  for rep in 1 2; do
    run IOMODE=3; run IOMODE=4; run IOMODE=4 CHUNKKB=512; run IOMODE=5; run IOMODE=5 CHUNKKB=64; run IOMODE=5 IO5MAP=1; run IOMODE=5 IO5MAP=1 IO5RA=1
  done
  PERF=$(ls /usr/lib/linux-tools/*/perf 2>/dev/null | head -1); [ -z "$PERF" ] && PERF=perf
  sudo sysctl -w kernel.perf_event_paranoid=-1 >/dev/null; sudo sysctl -w kernel.kptr_restrict=0 >/dev/null
  rm -rf /tmp/o; mkdir /tmp/o
  sudo $PERF record -q -F 2000 -g -o /tmp/perf5.data env IOMODE=5 NCPU=$N DATA_DIR=$HOME/d20 OUTPUT_DIR=/tmp/o ./target/release/sessions >/dev/null 2>&1
  sudo $PERF report -i /tmp/perf5.data --no-children --stdio --sort sym 2>/dev/null | grep -E "^ +[0-9]" | head -30 | sed 's/^/IOP /'
  sudo $PERF report -i /tmp/perf5.data --no-children --stdio --sort sym -G 2>/dev/null | grep -A14 -E "^ +[0-9.]+%.*\[k\]" | head -90 | sed 's/^/IOG /'
fi
