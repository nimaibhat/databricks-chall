# For Nimai

Paste this into a fresh Claude session on `origin/main`. The pipeline source is still `941879e` (`single pipeline binary; smaller output files, stale ones cleared`); commits after it are notes and measurements. The board is about 0.35 s. The shape of a warm run is about 215 ms in process (parse ~160 ms, events ~35 ms, write ~10 ms). On this source the AWS VM measured the static binary directly, page cache warm, `NCPU=8`: median 198.6 ms on ext4 (parse 146.3, events 35.5, write 9.4; noise about ±1 ms) and median 210.1 ms with the input on tmpfs (parse 157.8, events 35.4, write 9.5). The floor people guessed, about 0.29–0.30 s, is that process plus about 0.1 s of container start and exit. `jason-scheffel/express-lane` `92cf412` matches `941879e` for `src/`, `Dockerfile`, `Cargo.toml`, and `.cargo/`. Do not redo that code. Do not merge `saran/work`.

## Experiments

Nothing below is still open. Both candidates were measured on this source, in `OVERNIGHT_LOG.md`, after an earlier note named them. Do not run them again. A change needs about 2 ms on that clock to count.

**PGO, closed.** Experiment 1. Instrumented build in `rust:1.100.0-alpine`, trained on seed 20 (20M), seed 3 (10M), and dev, `-Cprofile-use` in the Dockerfile, and `__llvm_profile_write_file()` before `_exit` (this binary exits with `sys::_exit`, which skips the profiler `atexit`). Base 197.8 / 198.6 ms, profiled 197.5 / 198.5 ms, parse 146.3 vs 146.1. Exact on fixtures, dev, and the big set. No gain, and it pins a toolchain plus a profile into the build. Older trees of this task saw about 3–5% (about 6–15 ms). This source did not. Do not rebuild it, and do not commit a profdata file. The plain `cargo build --release` in the Dockerfile stays the build.

**`pread` into a reused buffer, closed.** The path is already in the binary. `XL_RD` is KiB (`src/parse.rs`, default 0). Zero parses the `mmap` from `src/setup.rs`. Non-zero makes each worker `pread` into one buffer allocated once in `stage1` and reused for every chunk. Experiment 2 set `XL_RD` from 64 through 1024 on the ext4 run: 226–239 ms against a base of 198. That is the whole cut the other way, not a win under 10 ms. Experiment 3 included `XL_RD=128` on tmpfs and did not report a win. Simon Ellmann and Thomas Neumann, “One Pass to Parse Them All,” PVLDB 2026 (https://www.vldb.org/pvldb/vol19/p3579-ellmann.pdf), saw 183 GB/s against 34 GB/s on 128 cores; their own single thread is already 1.8 GB/s quoted and 2.1 GB/s unquoted, so 8 vCPUs were never going to show that gap. Andrew Crotty, Viktor Leis, and Andrew Pavlo, CIDR 2022 (https://www.cidrdb.org/cidr2022/papers/p13-crotty.pdf): `mmap` is acceptable when the read-only file fits in RAM. It does, and the measurement says keep it. Do not set `XL_RD`.

**Prefault stays off.** `XL_PREFAULT` defaults off (`src/main.rs` sets `sys::PREFAULT` only when the env var is `1`). It was in the same sweep and did not beat the default. Experiment 4 added `MADV_POPULATE_READ` on each chunk before parse: 209.5 / 210.7 against 209.2 / 210.3, no gain, reverted. Ellmann and Neumann also measured populate as a loss. Do not set `XL_PREFAULT`, and do not put populate back.

## Already in the binary

Do not redo any of these. The sweep that tried the opposite of the defaults lost: huge pages off was 221 ms, the custom allocator off was 220, non-temporal stores off was 220, all against 198.

- Dense ids. Base-36 inverse is `keyrec::BINV`. The id is an offset.
- AVX2 `k_free` parser (`k_free::ev_fast` → `ev_s` / `ev_d`): two 64-bit comma masks, `tzcnt`, record consumed in that pass. `src/generic/` is only the `--generic` fallback.
- `pext` date cache, then one AVX2 multiply-add that writes both timestamps.
- Gap sessions. `GAP_MS` is 1_800_000. The ordered path cuts a session on a user change or a gap and does not sort.
- Links overlapped with parse. Queue order is clicks, links, then events, and link work runs between event chunks.
- Huge pages, default on (`sys::HUGE`).
- Non-temporal stores, default on (`XL_NT`).
- Exit is `sys::_exit(0)`.
- Handwritten dictionary/RLE Parquet (`src/pqw.rs`, `E_RLE` / `E_RLE_DICT`).
- `XL_PREFAULT` default off, and the input stays mapped (`XL_RD` default 0).

## Papers that do not justify new work

- ConnectIt (Dhulipala, Hong, and Shun, MIT CSAIL, PVLDB 2021, https://www.vldb.org/pvldb/vol14/p653-dhulipala.pdf): billion-edge union-find. Identity links here are a small graph already overlapped with parse.
- SageDB (Kraska et al., CIDR 2019, https://www.cidrdb.org/cidr2019/papers/p117-kraska-cidr19.pdf): a dense integer key is an offset, which this binary already does.
- NoisePage relaxed operator fusion (Menon, Mowry, and Pavlo, PVLDB 2017, https://www.vldb.org/pvldb/vol11/p1-menon.pdf): the win is cache-missing hash joins, not this fused pass.
