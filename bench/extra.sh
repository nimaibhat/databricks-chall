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
    head -70 /tmp/ann.txt | cut -c1-230 | sed "s/^/CGR$st /"
    grep -nE "^-- (Auto|User)-annotated|annotated source" /tmp/ann.txt | head -8 | sed "s/^/CGR$st /"
    awk '/annotated source/{f++} f>=1 && NR%1==0' /tmp/ann.txt | grep -E "^ *[0-9,]{9,}" | sort -t'(' -k1,1 -r | head -5 | cut -c1-200 | sed "s/^/CGS$st /"
    python3 - $st <<'PY'
import re,sys
st=sys.argv[1]
cur=''; rows=[]; inblk=False
for l in open('/tmp/ann.txt',errors='replace'):
    if 'annotated source' in l or l.startswith('-- Auto-annotated') or l.startswith('-- User-annotated'):
        cur=l.strip().split('/')[-1][:14]; continue
    m=re.match(r'\s*([\d,]+) \(\s*[\d.]+%\)\s+([\d,]+|\.)?(?: \(\s*[\d.]+%\))?\s*(.*)',l)
    if m and cur:
        try: ir=int(m.group(1).replace(',',''))
        except: continue
        b=m.group(2); b=0 if b in (None,'.') else int(b.replace(',',''))
        if ir>=1000000 or b>=30000: rows.append((ir,b,cur,m.group(3).strip()[:110]))
# function summary
f=False
for l in open('/tmp/ann.txt',errors='replace'):
    if 'File:function summary' in l or 'Function:file summary' in l: f=True; n=0; continue
    if f:
        if l.startswith('-- ') and 'summary' not in l and n>3: break
        if re.search(r'sessions::|\*\*\*|memcpy|memmove|memset',l):
            print(('CGF%s '%st)+l.strip()[:200]); n+=1
            if n>40: break
rows.sort(reverse=True)
for r in rows[:110]: print(('CGL%s ir=%%10d bcm=%%8d %%-14s %%s'%st)%r)
rows.sort(key=lambda r:-r[1])
for r in rows[:25]: print(('CGB%s ir=%%10d bcm=%%8d %%-14s %%s'%st)%r)
PY
  done
fi

# I/O mode comparison, native, N threads: phase2 line + rusage
if [ -n "${IO_REF:-}" ]; then
  cd "$GITHUB_WORKSPACE"; rm -rf /tmp/t3; git worktree prune; git worktree add -f /tmp/t3 "$IO_REF" 2>&1 | tail -1; cd /tmp/t3
  RUSTFLAGS="-C target-cpu=x86-64-v3 -C target-feature=+crt-static" cargo build --release 2>&1 | tail -1
  for rep in 1 2; do for m in 3 4 5; do for ds in d20; do
    rm -rf /tmp/o; mkdir /tmp/o
    IOMODE=$m NCPU=$N DATA_DIR=$HOME/$ds OUTPUT_DIR=/tmp/o ./target/release/sessions 2>&1 | grep -E "^(phase2|rusage|done)" | tr '\n' ' ' | sed "s/^/IO mode=$m $ds : /"; echo
  done; done; done
  IOMODE=5 NCPU=$N DATA_DIR=$HOME/d20 OUTPUT_DIR=/tmp/o strace -f -c -o /tmp/st.txt ./target/release/sessions >/dev/null 2>&1; head -12 /tmp/st.txt | sed 's/^/IOS /'
fi
