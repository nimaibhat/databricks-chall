#!/bin/bash
# Benchmarks every commit listed in bench/refs.txt (default: HEAD) on ~/data
# with the scorer's docker flags. NCPU = the runner's vCPUs.
set -u
DATA=~/data
N=$(nproc)
refs=$(grep -v '^#' bench/refs.txt 2>/dev/null | awk '{print $1}')
[ -z "$refs" ] && refs=HEAD
runone() { # image, label, extra -e flags...
  img=$1; label=$2; shift 2
  out=$(mktemp -d); scr=$(mktemp -d); chmod 777 $out $scr
  t0=$(date +%s.%N)
  docker run --rm --network none --read-only --tmpfs /tmp:rw,size=64m,mode=1777 \
    --cap-drop ALL --security-opt no-new-privileges --pids-limit 4096 --cpus $N --memory 14g --memory-swap 14g \
    --user 1000:1000 -v $DATA:/data:ro -v $out:/output -v $scr:/scratch \
    -e DATA_DIR=/data -e OUTPUT_DIR=/output -e SCRATCH_DIR=/scratch -e HOME=/scratch -e NCPU=$N -e MEM_GB=24 "$@" $img \
    2> $out.err
  rc=$?
  t1=$(date +%s.%N)
  printf "%-28s rc=%s wall=%.3f | %s\n" "$label" $rc $(echo "$t1 - $t0" | bc) \
    "$(grep -E '^(phase2|phase3 |phase3b|phase4 s|done)' $out.err | sed -E 's/arena.*//; s/\(parse only[^)]*\)//' | tr -s ' ' | tr '\n' '|')"
  sudo rm -rf $out $scr $out.err
}
for r in $refs; do
  sha=$(git rev-parse --short $r)
  rm -rf /tmp/src && git worktree add -f /tmp/src $r -q 2>/dev/null || git -C /tmp/src checkout -q $r
  docker build -q -t sut:$sha /tmp/src >/dev/null || { echo "build failed $sha"; continue; }
  git worktree remove --force /tmp/src
  echo "=== $sha $(git log -1 --format=%s $r)"
  runone sut:$sha "$sha warmup" >/dev/null
  for i in 1 2 3 4 5; do runone sut:$sha "$sha warm#$i"; done
  for i in 1 2; do sync; sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'; runone sut:$sha "$sha cold#$i"; done
  # variants from refs.txt column 2.. (env flags)
  extra=$(grep "^$r " bench/refs.txt 2>/dev/null | cut -d' ' -f2-)
  for e in $extra; do for i in 1 2 3; do runone sut:$sha "$sha $e #$i" -e $e; done; done
done
