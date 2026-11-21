#!/bin/bash
# Variants (bench/variants.txt): label|ref|build-args|env-flags ; ref may be ext:owner/repo[@sha]
# Interleaved rounds per dataset; full container start-to-exit wall; output equality vs first variant (duckdb).
set -u
source bench/config.env
N=$(nproc)
pip install -q duckdb >/dev/null 2>&1
mapfile -t V < <(grep -v '^#' bench/variants.txt | grep -v '^$')
for v in "${V[@]}"; do
  IFS='|' read -r label ref bargs envs <<< "$v"
  rm -rf /tmp/src; git worktree prune
  if [[ "$ref" == ext:* ]]; then
    r=${ref#ext:}; repo=${r%@*}; sha=""; [[ "$r" == *@* ]] && sha=${r#*@}
    git clone -q https://github.com/$repo /tmp/src && [ -n "$sha" ] && git -C /tmp/src checkout -q $sha
    desc="$repo $(git -C /tmp/src rev-parse --short HEAD)"
  else
    git worktree add -f /tmp/src "$ref" -q; desc="$(git rev-parse --short $ref) $(git log -1 --format=%s $ref | cut -c1-70)"
  fi
  ba=""; for b in $bargs; do ba="$ba --build-arg $b"; done
  docker build -q $ba -t "sut:$label" /tmp/src >/dev/null 2>/tmp/build.err || { echo "BUILD FAILED $label"; tail -5 /tmp/build.err; }
  [[ "$ref" == ext:* ]] && rm -rf /tmp/src || git worktree remove --force /tmp/src
  echo "built $label = $desc [$bargs] [$envs]"
done
runone() { # ds label envs keep
  ds=$1; label=$2; envs=$3; keep=${4:-}
  out=$(mktemp -d); scr=$(mktemp -d); chmod 777 $out $scr
  ef=""; omount="-v $out:/output"
  for e in $envs; do if [ "$e" = "@tmpfs" ]; then omount="--tmpfs /output:rw,size=2g,mode=1777"; else ef="$ef -e $e"; fi; done
  t0=$(date +%s.%N)
  docker run --rm --network none --read-only --tmpfs /tmp:rw,size=64m,mode=1777 \
    --cap-drop ALL --security-opt no-new-privileges --pids-limit 4096 --cpus $N --memory 14g --memory-swap 14g \
    --user 1000:1000 -v $HOME/$ds:/data:ro $omount -v $scr:/scratch \
    -e DATA_DIR=/data -e OUTPUT_DIR=/output -e SCRATCH_DIR=/scratch -e HOME=/scratch -e NCPU=$N -e MEM_GB=24 $ef "sut:$label" \
    2> $out.err >/dev/null
  rc=$?
  t1=$(date +%s.%N)
  w=$(echo "$t1 - $t0" | bc)
  ph=$(grep -E '^(phase1|phase2|phase3 |phase3b|phase4 s|done)' $out.err | sed -E 's/\[[^]]*\]//; s/arena.*//; s/\(parse only[^)]*\)//' | awk '{print $NF}' | tr '\n' ' ')
  es=$(grep -oE '^epoch_start [0-9.]+' $out.err | awk '{print $2}'); ed=$(grep -oE '^epoch_done [0-9.]+' $out.err | awk '{print $2}')
  pre=""; post=""; [ -n "$es" ] && pre=$(echo "$es - $t0" | bc); [ -n "$ed" ] && post=$(echo "$t1 - $ed" | bc)
  echo "$ds $label rc=$rc wall=$w pre=$pre post=$post | $ph" >> /tmp/results.txt
  if [ -n "$keep" ]; then grep -vE "^(epoch|rusage)" $out.err | tail -${ERRLINES:-0} | cut -c1-220 | sed "s/^/ERRLOG $ds $label /"; sudo chmod -R a+rwX $out; rm -rf /tmp/out_${ds}_$label; mv $out /tmp/out_${ds}_$label; else sudo rm -rf $out; fi
  sudo rm -rf $scr $out.err
}
# fixtures against the binary inside each image named in IMG_FIX
pip install -q polars pyarrow >/dev/null 2>&1
for label in ${IMG_FIX:-}; do
  rm -rf /tmp/imgbin; mkdir -p /tmp/imgbin
  cid=$(docker create "sut:$label"); docker cp $cid:/sessions /tmp/imgbin/sessions >/dev/null; docker rm $cid >/dev/null
  git worktree prune; rm -rf /tmp/fx; ref=$(grep "^$label|" bench/variants.txt | cut -d'|' -f2); git worktree add -f /tmp/fx "$ref" -q
  (cd /tmp/fx && python3 scripts/run_fixtures.py fixtures -- /tmp/imgbin/sessions 2>&1 | tail -1 | sed "s/^/IMGFIX $label /")
  git worktree remove --force /tmp/fx
done
: > /tmp/results.txt
# fixed container cost: the first image with no data (the binary exits at once)
first_label=$(echo "${V[0]}" | cut -d'|' -f1)
for i in 1 2 3 4 5 6 7; do
  t0=$(date +%s.%N)
  docker run --rm --network none --read-only --tmpfs /tmp:rw,size=64m,mode=1777 --cap-drop ALL --security-opt no-new-privileges --pids-limit 4096 --cpus $N --memory 14g --memory-swap 14g --user 1000:1000 -v $HOME/d0:/data:ro --tmpfs /output:rw,size=64m,mode=1777 -e DATA_DIR=/data -e OUTPUT_DIR=/output -e NCPU=$N "sut:$first_label" >/dev/null 2>&1
  t1=$(date +%s.%N); echo "EMPTY $(echo "$t1 - $t0" | bc)"
done
for ds in d1 d20 d60; do
  R=$R20; [ $ds = d60 ] && R=$R60; [ $ds = d1 ] && R=${R1:-0}
  [ "$R" -gt 0 ] || continue
  # only this dataset in the page cache, so the larger one is not evicted mid-run
  sync; sudo sh -c 'echo 1 > /proc/sys/vm/drop_caches'; 
  cat $HOME/$ds/*/*.csv $HOME/$ds/*.csv > /dev/null; free -m | sed -n 2p | sed "s/^/MEM $ds /"
  for v in "${V[@]}"; do IFS='|' read -r label ref bargs envs <<< "$v"; runone $ds "$label" "$envs" keep; done
  for r in $(seq 1 $R); do for v in "${V[@]}"; do IFS='|' read -r label ref bargs envs <<< "$v"; runone $ds "$label" "$envs"; done; done
done
echo "=== raw"; cat /tmp/results.txt
echo "=== summary (warmup excluded): ds label n | wall med min max | phase medians (links parse dedup resolve sess out)"
python3 - <<'PY'
import re,collections,statistics
rows=collections.defaultdict(list); seen=set()
def sec(s):
    m=re.match(r'([0-9.]+)(ms|s|µs|us)',s)
    if not m: return float('nan')
    return float(m.group(1))*{'s':1,'ms':1e-3,'µs':1e-6,'us':1e-6}[m.group(2)]
for l in open('/tmp/results.txt'):
    p=l.split(); key=(p[0],p[1])
    if key not in seen: seen.add(key); continue
    w=float(re.search(r'wall=([0-9.]+)',l).group(1))
    st=[sec(x) for x in l.split('|')[1].split()]
    ph=[st[0]]+[st[i]-st[i-1] for i in range(1,len(st))] if len(st)==6 else []
    mp=re.search(r'pre=(-?[0-9.]+)',l); mq=re.search(r'post=(-?[0-9.]+)',l)
    rows[key].append((w,ph,float(mp.group(1)) if mp else float('nan'),float(mq.group(1)) if mq else float('nan')))
for (ds,lab),v in rows.items():
    ws=[x[0] for x in v]
    pm=' '.join('%.3f'%statistics.median(x[1][i] for x in v) for i in range(6)) if all(len(x[1])==6 for x in v) else '-'
    print(f"SUM {ds} {lab:22s} n={len(ws)} | {statistics.median(ws):.3f} {min(ws):.3f} {max(ws):.3f} | {pm} | pre {statistics.median(x[2] for x in v):.3f} post {statistics.median(x[3] for x in v):.3f}")
PY
echo "=== output equality vs first variant"
first=$(echo "${V[0]}" | cut -d'|' -f1)
for ds in d1 d20 d60; do
 for v in "${V[@]}"; do
  label=$(echo "$v" | cut -d'|' -f1)
  [ -d /tmp/out_${ds}_$label ] || continue
  python3 - "$ds" "$first" "$label" <<'PY'
import duckdb,sys
ds,a,b=sys.argv[1:]
ok=True
for t in ["user_daily","campaign_daily"]:
    cols = "user_id,day,sessions,events,revenue_cents,top_category" if t=="user_daily" else "campaign_id,channel,day,sessions,purchasing_sessions,revenue_cents,assisted_sessions"
    q=lambda x: f"select {cols} from read_parquet('/tmp/out_{ds}_{x}/{t}/*.parquet')"
    try:
        n1=duckdb.sql(f"select count(*) from ({q(a)} except all {q(b)})").fetchall()[0][0]
        n2=duckdb.sql(f"select count(*) from ({q(b)} except all {q(a)})").fetchall()[0][0]
        c=duckdb.sql(f"select count(*) from ({q(b)})").fetchall()[0][0]
    except Exception as e:
        print("  ERR",b,t,str(e)[:200]); ok=False; continue
    ok = ok and n1==0 and n2==0
    print(f"  {ds} {b} {t}: rows {c}, diff {n1}/{n2}")
print(f"EQ {ds} {b}: {'SAME' if ok else 'DIFFERENT'} as {a}")
PY
 done
done
