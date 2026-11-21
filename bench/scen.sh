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
cg_annotate /tmp/cg.out --sort=Ir 2>/dev/null > /tmp/cg_ir.txt
cg_annotate /tmp/cg.out --sort=Bcm 2>/dev/null > /tmp/cg_bcm.txt
echo "CG ==== functions by Ir"; awk '/^-- (File:)?function summary|^-- Function summary/{f=1} f' /tmp/cg_ir.txt | head -40 | cut -c1-200 | sed 's/^/CG /'
echo "CG ==== functions by Bcm"; awk '/^-- (File:)?function summary|^-- Function summary/{f=1} f' /tmp/cg_bcm.txt | head -30 | cut -c1-200 | sed 's/^/CG /'
cg_annotate /tmp/cg.out --sort=Bcm --show=Ir,Bc,Bcm --annotate=yes 2>/dev/null | grep -E "^ *[0-9,]+ \([ 0-9.]+%\) +[0-9,]+ \([ 0-9.]+%\) +[0-9,]+ \([ 0-9.]+%\)  " > /tmp/lines.txt
echo "CG ==== lines by Bcm"; sort -t'(' -k4 -rn /tmp/lines.txt | awk '{print}' | python3 -c "
import sys,re
rows=[]
for l in sys.stdin:
    m=re.match(r' *([0-9,]+) \(([ 0-9.]+)%\) +([0-9,]+) \(([ 0-9.]+)%\) +([0-9,]+) \(([ 0-9.]+)%\)  (.*)',l)
    if m: rows.append((int(m.group(5).replace(',','')),int(m.group(1).replace(',','')),m.group(7)[:110]))
rows.sort(reverse=True)
for r in rows[:45]: print('CG  bcm=%9d ir=%11d %s'%r)
print('CG ==== lines by Ir')
rows.sort(key=lambda r:-r[1])
for r in rows[:70]: print('CG  bcm=%9d ir=%11d %s'%r)
"
