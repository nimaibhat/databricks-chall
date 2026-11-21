#!/bin/bash
# Stage timing: parse cut off after stage n (diag feature), native, 1 and N threads.
set -u
source bench/config.env
[ -z "${DIAG_REF:-}" ] && exit 0
N=$(nproc)
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
