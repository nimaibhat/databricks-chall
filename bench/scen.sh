#!/bin/bash
# Scenario probes reusing images built by gha_bench.sh: THP off, cold cache, 4K-only.
set -u
DATA=~/data; N=$(nproc)
run() { # label envs
  out=$(mktemp -d); scr=$(mktemp -d); chmod 777 $out $scr
  ef=""; for e in $2; do ef="$ef -e $e"; done
  t0=$(date +%s.%N)
  docker run --rm --network none --read-only --tmpfs /tmp:rw,size=64m,mode=1777 --cap-drop ALL --security-opt no-new-privileges --pids-limit 4096 --cpus $N --memory 14g --memory-swap 14g --user 1000:1000 -v $DATA:/data:ro -v $out:/output -v $scr:/scratch -e DATA_DIR=/data -e OUTPUT_DIR=/output -e SCRATCH_DIR=/scratch -e HOME=/scratch -e NCPU=$N -e MEM_GB=24 $ef "sut:$1" 2> $out.err
  t1=$(date +%s.%N)
  echo "$3 $1 [$2] wall=$(echo "$t1 - $t0" | bc) $(grep -E '^done' $out.err | cut -c1-70)"
  sudo rm -rf $out $scr $out.err
}
L="c35753a cb3c1d3 main"
echo "=== THP never"
echo never | sudo tee /sys/kernel/mm/transparent_hugepage/enabled >/dev/null
for r in 1 2 3; do for l in $L; do run $l "" thpnever; done; run main "IOMODE=4" thpnever; done
echo madvise | sudo tee /sys/kernel/mm/transparent_hugepage/enabled >/dev/null
echo "=== cold cache"
for r in 1 2; do for l in $L; do sync; echo 3 | sudo tee /proc/sys/vm/drop_caches >/dev/null; run $l "" cold; done
  for e in "IOMODE=4" "IOMODE=2" "IOMODE=1" "RA=0"; do sync; echo 3 | sudo tee /proc/sys/vm/drop_caches >/dev/null; run main "$e" cold; done; done
echo "=== thread scaling (main, warm)"
for n in 1 2 3 4; do run main "NCPU=$n" scale; run main "NCPU=$n" scale; done
