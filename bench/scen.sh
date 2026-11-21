#!/bin/bash
set -u
sudo apt-get install -y -qq valgrind >/dev/null 2>&1
G=$(dirname $(find ~/gen -name Cargo.toml | head -1))
$G/target/release/generate --seed 1 --events 1000000 --clicks 250000 --output ~/dev >/dev/null 2>&1
wc -l ~/dev/events/* | tail -1
rm -rf /tmp/t && git worktree prune && git worktree add -f /tmp/t main -q && cd /tmp/t
RUSTFLAGS="-C target-cpu=x86-64-v3" CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release -q 2>&1 | tail -2
rm -rf /tmp/o; mkdir /tmp/o
NCPU=1 DATA_DIR=$HOME/dev OUTPUT_DIR=/tmp/o valgrind --tool=cachegrind --branch-sim=yes --cache-sim=no --cachegrind-out-file=/tmp/cg.out ./target/release/sessions 2>&1 | grep -E "I +refs|Branches|Mispred" | sed 's/^/CG /'
cg_annotate /tmp/cg.out --sort=Ir --show=Ir,Bcm --annotate=yes 2>/dev/null > /tmp/ann.txt
python3 - <<'PY'
import re
cur=''
rows=[]
for l in open('/tmp/ann.txt',errors='replace'):
    m=re.match(r'-- Auto-annotated source: (.*)',l)
    if m: cur=m.group(1).split('/')[-1]; continue
    m=re.match(r'\s*([\d,]+) \(\s*[\d.]+%\)\s+([\d,]+|\.)( \(\s*[\d.]+%\))?\s\s(.*)',l)
    if m and cur:
        ir=int(m.group(1).replace(',','')); b=m.group(2); b=0 if b=='.' else int(b.replace(',',''))
        rows.append((ir,b,cur,m.group(4).strip()[:120]))
rows.sort(reverse=True)
for r in rows[:110]: print('CG ir=%10d bcm=%8d %-10s %s'%r)
print('CG ==== by bcm')
rows.sort(key=lambda r:-r[1])
for r in rows[:40]: print('CG ir=%10d bcm=%8d %-10s %s'%r)
PY
