#!/bin/bash
# user/sys/faults of ours and two public builds: inside the scorer-flag container and native, 20M, warm.
set -u
N=$(nproc)
cat $HOME/d20/*/*.csv > /dev/null
FMT="wall %e user %U sys %S minflt %R majflt %F maxrss %MKB csw %c/%w"
for label in ours cheryl vibhor; do
  ep=$(docker inspect -f '{{join .Config.Entrypoint " "}} {{join .Config.Cmd " "}}' sut:$label)
  echo "SYS $label entry: $ep"
  bin=$(echo $ep | awk '{print $1}')
  cid=$(docker create sut:$label); docker cp $cid:$bin /tmp/bin_$label >/dev/null 2>&1; docker rm $cid >/dev/null
  file /tmp/bin_$label | cut -c1-120 | sed "s/^/SYS $label /"
  for i in 1 2 3 4 5; do
    out=$(mktemp -d); scr=$(mktemp -d); chmod 777 $out $scr
    docker run --rm --network none --read-only --tmpfs /tmp:rw,size=64m,mode=1777 --cap-drop ALL --security-opt no-new-privileges --pids-limit 4096 --cpus $N --memory 14g --memory-swap 14g \
      --user 1000:1000 -v $HOME/d20:/data:ro -v $out:/output -v $scr:/scratch -v /usr/bin/time:/tm:ro \
      -e DATA_DIR=/data -e OUTPUT_DIR=/output -e SCRATCH_DIR=/scratch -e HOME=/scratch -e NCPU=$N -e MEM_GB=24 --entrypoint /tm sut:$label -f "$FMT" $ep 2>&1 >/dev/null | grep -E "^wall |rror|cannot|not found" | sed "s/^/SYS $label docker /"
    sudo rm -rf $out $scr
  done
  for i in 1 2 3 4 5; do
    rm -rf /tmp/o; mkdir /tmp/o
    NCPU=$N DATA_DIR=$HOME/d20 OUTPUT_DIR=/tmp/o /usr/bin/time -f "$FMT" /tmp/bin_$label 2>&1 >/dev/null | grep -E "^wall |rror" | sed "s/^/SYS $label native /"
  done
  # native inside a memory-limited cgroup scope, to separate the memory controller from the rest of the container
  for i in 1 2 3; do
    rm -rf /tmp/o; mkdir /tmp/o
    sudo systemd-run --quiet --scope -p MemoryMax=14G -p MemorySwapMax=0 env NCPU=$N DATA_DIR=$HOME/d20 OUTPUT_DIR=/tmp/o /usr/bin/time -f "$FMT" /tmp/bin_$label 2>&1 >/dev/null | grep -E "^wall |rror" | sed "s/^/SYS $label memcg /"
    sudo rm -rf /tmp/o
  done
done
cat /sys/kernel/mm/transparent_hugepage/enabled | sed 's/^/SYS thp /'
