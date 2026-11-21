#!/bin/bash
# Variants from bench/variants.txt, one per line:  label|ref|build-args|env-flags
# Runs are interleaved across variants (R rounds) so machine drift cancels.
# Output of every variant is compared with the first one (sorted rows, duckdb).
set -u
DATA=~/data
N=$(nproc)
R=${R:-5}
pip install -q duckdb >/dev/null 2>&1
mapfile -t V < <(grep -v '^#' bench/variants.txt | grep -v '^$')
declare -A IMG
for v in "${V[@]}"; do
  IFS='|' read -r label ref bargs envs <<< "$v"
  sha=$(git rev-parse --short "$ref")
  tag="sut:${label}"
  rm -rf /tmp/src; git worktree prune; git worktree add -f /tmp/src "$ref" -q
  ba=""; for b in $bargs; do ba="$ba --build-arg $b"; done
  docker build -q $ba -t "$tag" /tmp/src >/dev/null || echo "BUILD FAILED $label"
  git worktree remove --force /tmp/src
  echo "built $label = $sha $(git log -1 --format=%s $ref) [$bargs] [$envs]"
done
runone() { # label envs keep_out
  label=$1; envs=$2; keep=${3:-}
  out=$(mktemp -d); scr=$(mktemp -d); chmod 777 $out $scr
  ef=""; for e in $envs; do ef="$ef -e $e"; done
  t0=$(date +%s.%N)
  docker run --rm --network none --read-only --tmpfs /tmp:rw,size=64m,mode=1777 \
    --cap-drop ALL --security-opt no-new-privileges --pids-limit 4096 --cpus $N --memory 14g --memory-swap 14g \
    --user 1000:1000 -v $DATA:/data:ro -v $out:/output -v $scr:/scratch \
    -e DATA_DIR=/data -e OUTPUT_DIR=/output -e SCRATCH_DIR=/scratch -e HOME=/scratch -e NCPU=$N -e MEM_GB=24 $ef "sut:$label" \
    2> $out.err
  rc=$?
  t1=$(date +%s.%N)
  w=$(echo "$t1 - $t0" | bc)
  d=$(grep -oE '^done [0-9.]+m?s' $out.err | awk '{print $2}')
  ph=$(grep -E '^(phase2|phase3 |phase3b|phase4 s|done)' $out.err | sed -E 's/\[[^]]*\]//; s/arena.*//; s/\(parse only[^)]*\)//; s/phase//' | awk '{print $NF}' | tr '\n' ' ')
  echo "$label rc=$rc wall=$w done=$d | $ph" >> /tmp/results.txt
  if [ -n "$keep" ]; then sudo chmod -R a+rwX $out; mv $out /tmp/out_$label; else sudo rm -rf $out; fi
  sudo rm -rf $scr $out.err
}
: > /tmp/results.txt
for v in "${V[@]}"; do IFS='|' read -r label ref bargs envs <<< "$v"; runone "$label" "$envs" keep; done   # warmup + keep output
for r in $(seq 1 $R); do for v in "${V[@]}"; do IFS='|' read -r label ref bargs envs <<< "$v"; runone "$label" "$envs"; done; done
echo "=== raw"; cat /tmp/results.txt
echo "=== summary (excluding warmup): label  median_wall  median_done  min_done"
python3 - <<'PY'
import re,collections,statistics
rows=collections.defaultdict(list)
seen=set()
for l in open('/tmp/results.txt'):
    lab=l.split()[0]
    if lab not in seen: seen.add(lab); continue
    w=float(re.search(r'wall=([0-9.]+)',l).group(1))
    m=re.search(r'done=([0-9.]+)(m?s)',l)
    d=float(m.group(1))/(1000 if m.group(2)=='ms' else 1) if m else -1
    rows[lab].append((w,d))
for lab,v in rows.items():
    print(f"{lab:24s} {statistics.median([x[0] for x in v]):.3f} {statistics.median([x[1] for x in v]):.3f} {min(x[1] for x in v):.3f}")
PY
echo "=== output equality vs first variant"
first=$(echo "${V[0]}" | cut -d'|' -f1)
for v in "${V[@]}"; do
  label=$(echo "$v" | cut -d'|' -f1)
  python3 - "$first" "$label" <<'PY'
import duckdb,sys
a,b=sys.argv[1],sys.argv[2]
ok=True
for t in ["user_daily","campaign_daily"]:
    q=lambda x: f"select * from read_parquet('/tmp/out_{x}/{t}/*.parquet')"
    n1=duckdb.sql(f"select count(*) from ({q(a)} except all {q(b)})").fetchall()[0][0]
    n2=duckdb.sql(f"select count(*) from ({q(b)} except all {q(a)})").fetchall()[0][0]
    c=duckdb.sql(f"select count(*) from ({q(b)})").fetchall()[0][0]
    ok = ok and n1==0 and n2==0
    print(f"  {b} {t}: rows {c}, diff {n1}/{n2}")
print(f"  {b}: {'SAME' if ok else 'DIFFERENT'} as {a}")
PY
done
