#!/bin/bash
# Generates a PGO profile for PGO_REF with the Dockerfile's own toolchain image.
set -u
source bench/config.env
[ -z "${PGO_REF:-}" ] && exit 0
N=$(nproc)
cd "$GITHUB_WORKSPACE"; rm -rf /tmp/t4; git worktree prune; git worktree add -f /tmp/t4 "$PGO_REF" 2>&1 | tail -1
IMG=$(grep -m1 '^FROM rust' /tmp/t4/Dockerfile | awk '{print $2}'); echo "PGO image $IMG"
mkdir -p /tmp/pgo /tmp/pgo_out; chmod 777 /tmp/pgo /tmp/pgo_out
[ -d ~/dev ] || { G=$(dirname $(find ~/gen -name Cargo.toml | head -1)); $G/target/release/generate --seed 1 --events 1000000 --clicks 250000 --output ~/dev >/dev/null 2>&1; }
docker run --rm -v /tmp/t4:/src -v $HOME/d20:/d20:ro -v $HOME/dev:/dev1:ro -v /tmp/pgo:/pgo -v /tmp/pgo_out:/out -w /src "$IMG" bash -c '
  set -e
  rustup component add llvm-tools-preview >/dev/null 2>&1
  export CARGO_PROFILE_RELEASE_LTO=fat CARGO_TARGET_DIR=/tmp/tgt
  RUSTFLAGS="-C target-cpu=x86-64-v3 -C target-feature=+crt-static -Cprofile-generate=/pgo/raw" cargo build --release --target x86_64-unknown-linux-gnu 2>&1 | tail -1
  B=/tmp/tgt/x86_64-unknown-linux-gnu/release/sessions
  for ds in /dev1 /d20 /d20; do rm -rf /out/*; NCPU='$N' DATA_DIR=$ds OUTPUT_DIR=/out $B 2>&1 | grep -E "^done" ; done
  P=$(find $(rustc --print sysroot) -name llvm-profdata | head -1)
  $P merge -o /pgo/pgo.profdata /pgo/raw
  ls -la /pgo/pgo.profdata; rustc --version
' 2>&1 | sed 's/^/PGO /'
ls -la /tmp/pgo/pgo.profdata 2>&1 | sed 's/^/PGO /'
