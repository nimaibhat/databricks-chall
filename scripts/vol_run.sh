#!/bin/sh
# Run IMAGE on the dataset held in docker volume VOL with scorer-like flags.
# usage: vol_run.sh VOL [extra docker -e flags...]
VOL=$1; shift
docker run --rm --platform "${PLATFORM:-linux/arm64}" --network none --read-only --tmpfs /tmp:rw,size=64m,mode=1777 \
  --cap-drop ALL --security-opt no-new-privileges --pids-limit 4096 --cpus "${CPUS:-8}" --memory "${MEM:-7g}" --memory-swap "${MEM:-7g}" \
  --user 1000:1000 -v "$VOL":/data:ro --tmpfs /output:rw,size=2g,mode=1777 --tmpfs /scratch:rw,size=64m,mode=1777 \
  -e DATA_DIR=/data -e OUTPUT_DIR=/output -e SCRATCH_DIR=/scratch -e HOME=/scratch -e NCPU=8 -e MEM_GB=24 "$@" "${IMAGE:-ss:arm}"
