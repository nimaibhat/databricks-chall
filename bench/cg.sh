#!/bin/bash
# Cachegrind (instructions, simulated cache misses, branch mispredictions) for ours and two public builds, 1M set.
set -u
sudo apt-get install -y -qq valgrind >/dev/null 2>&1
G=$(dirname $(find ~/gen -name Cargo.toml | head -1))
$G/target/release/generate --seed 1 --events 1000000 --clicks 250000 --output ~/dev >/dev/null 2>&1
echo "CG lines: $(cat ~/dev/events/*.csv | wc -l) events $(cat ~/dev/clicks/*.csv | wc -l) clicks"
one() { # label dir bin extra-cargo-args
  label=$1; dir=$2; bin=$3; shift 3
  cd "$dir" || { echo "CG $label: no dir"; return; }
  RUSTFLAGS="-C target-cpu=x86-64-v3" CARGO_PROFILE_RELEASE_DEBUG=line-tables-only CARGO_PROFILE_RELEASE_STRIP=none cargo build --release "$@" 2>&1 | tail -1 | sed "s/^/CG $label build: /"
  rm -rf /tmp/o; mkdir /tmp/o
  NCPU=1 DATA_DIR=$HOME/dev OUTPUT_DIR=/tmp/o valgrind --tool=cachegrind --branch-sim=yes --cache-sim=yes --cachegrind-out-file=/tmp/cg_$label.out ./target/release/$bin 2>&1 | grep -E "I +refs|D +refs|D1 +misses|LLd misses|LL misses|Branches|Mispred|rror|valgrind:" | sed "s/^/CG $label /"
  ls /tmp/o/*/ | head -3 | sed "s/^/CG $label out: /"
  cg_annotate /tmp/cg_$label.out --show=Ir,D1mr,D1mw,DLmr,DLmw,Bcm,Bim 2>/dev/null | grep -vE "^$" | head -75 | cut -c1-230 | sed "s/^/CGF $label /"
  (cg_annotate /tmp/cg_$label.out --show=Ir,Bcm --annotate=yes > /tmp/ann_$label.txt 2>/tmp/ann.err || cg_annotate /tmp/cg_$label.out --show=Ir,Bcm > /tmp/ann_$label.txt 2>>/tmp/ann.err); head -3 /tmp/ann.err | sed "s/^/CGE $label /"; wc -l < /tmp/ann_$label.txt | sed "s/^/CGE $label lines /"
  grep -n "nnotated source" /tmp/ann_$label.txt | head -6 | cut -c1-140 | sed "s/^/CGE $label /"
  ln=$(grep -n "nnotated source" /tmp/ann_$label.txt | head -1 | cut -d: -f1); [ -n "$ln" ] && sed -n "$((ln)),$((ln+14))p" /tmp/ann_$label.txt | cut -c1-160 | sed "s/^/CGE $label /"
  awk '/^-- .*[Aa]nnotated source( file)?: /{f=$NF; sub(/.*\//,"",f); ln=0; next} /^-- line /{ln=$3+0-1; next} f!="" && !/^--/ {ln++} /^ *[0-9,]+ \(/{n=$1; gsub(/,/,"",n); if (n+0>=1000000 && f!="") {line=$0; sub(/^ *[0-9,]+ +\([^)]*\) +[0-9,.]+( +\([^)]*\))? */,"",line); printf "%d %s:%d | %s\n", n, f, ln, substr(line,1,120)}}' /tmp/ann_$label.txt | sort -rn | head -170 | sed "s/^/CGL $label /"
}
W=$GITHUB_WORKSPACE
rm -rf /tmp/t2; git -C $W worktree prune; git -C $W worktree add -f /tmp/t2 origin/main -q
one ours /tmp/t2 sessions
rm -rf /tmp/c1; git clone -q https://github.com/cherylaurelia/builders-hackathon /tmp/c1 && git -C /tmp/c1 checkout -q 2e4261f
one cheryl /tmp/c1 sessions
rm -rf /tmp/c2; git clone -q https://github.com/Vibhor7-7/shopping-sessions /tmp/c2 && git -C /tmp/c2 checkout -q 765c6e2
one vibhor /tmp/c2 pipeline --bin pipeline
