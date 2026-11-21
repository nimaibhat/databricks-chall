# OVERNIGHT_LOG.md

**SUBMIT THIS: `0a06317`** (adds the sessions-phase rework). x86 runner, full `docker run` wall, median of 7, one run on an Intel Xeon 8573C (4 vCPU, a slow runner): 20M 2.257 s, 60M 6.342 s, against 2.306 / 6.479 s for its parent `62abf9c` and 2.401 / 6.775 s for the pick before that; ranges do not overlap with the parent at 20M. Passed the full gate on arm64 and on the x86-64-v3 build (36/36 fixtures, dev answer, 20M at 1/2/8 threads and 60M equal to the goldens); unit and fuzz tests and fixtures pass on the x86 runner; outputs equal there on both datasets. The fastest public build takes 1.430 / 4.022 s in the same run.

How to read the numbers: "x86" is a GitHub Actions `ubuntu-latest` runner (4 vCPU, 16 GB; CPU model varies per run, so only same-run comparisons count), timed around the whole `docker run` with the scorer's flags, variants interleaved, warm page cache. "M4" is this Mac (arm64, 10 cores), process wall. The x86 number decides.

## Yardstick (same x86 run, 2026-11-21, EPYC 9V74 4 vCPU)

Public competitor builds were run next to ours to translate runner time into board position (outputs were identical to ours on both datasets).

| Build | 20M wall (med/min/max) | 60M wall (med/min/max) |
|---|---|---|
| ours `1c4fa8e` | 2.323 / 2.308 / 2.355 | 6.988 / 6.597 / 7.210 |
| cherylaurelia `2e4261f` | 1.378 / 1.369 / 1.455 | 3.824 / 3.808 / 3.890 |
| Vibhor7-7 `1359a83` (board 0.82 to 0.96) | 1.589 / 1.579 / 1.650 | 4.648 / 4.588 / 4.698 |
| micah-andelman `4685e4a` | 1.922 / 1.906 / 1.939 | 6.029 / 5.892 / 6.046 |
| explorer-skp `d58857c` | 1.975 / 1.970 / 2.031 | 9.513 / 9.383 / 9.819 |
| Th3r0n `81a7e86` | 2.217 / 2.201 / 2.240 | 7.139 / 6.945 / 7.281 |

Reading: Vibhor's build scored 0.82 to 0.96 s on the board and takes 1.59 s here, so board is roughly 0.52 to 0.6 of this runner's time. Target 1 (under 0.85 s board) is about 1.5 s here at 20M; target 2 (under 0.65 s) is about 1.2 s. We are at 2.32 s.

## Experiments

Before/after pairs come from one x86 run each; absolute times differ between runs because the runner CPU differs (EPYC 9V74 in run 1, Xeon 8573C in run 2, where container start/stop alone is about 0.5 s).

| # | Commit | Change | x86 20M before/after | x86 60M before/after | M4 20M before/after | Verdict |
|---|---|---|---|---|---|---|
| 0 | `1c4fa8e` | baseline | 2.323 | 6.988 | 0.517 | baseline |
| 1 | `e7a116f` | Category lookup only for purchases; per-line atomic and `OnceLock` loads hoisted | 2.889 / 2.822 (min-max 2.876-3.019 / 2.802-2.843) | 8.253 / 8.056 (8.192-8.615 / 8.034-8.136) | 0.516 / 0.506 | kept |
| 2 | `b9c5acc` | Person resolved at parse (prefetched global link table), one scatter by person, 16-byte dedup keys that mark losers dead in place; resolve pass removed | 2.008 / 2.046 (9V74) | 5.706 / 5.788 | 0.515 / 0.494 | slower alone (+1.9%: parse +0.21 s, dedup+resolve -0.18 s); kept only as the base of #3 |
| 3 | `3826acb` | Line-at-a-time scanner: 128-bit mask window per line, quote validity by mask arithmetic | #2+#3 vs parent: 2.008 / 1.958 (9V74), 2.100 / 2.000 (7763) | 5.706 / 5.526, 6.333 / 5.935 | 0.489 / 0.472 | kept (pair -2.5% to -4.8% at 20M, -3.2% to -6.3% at 60M) |
| 4 | `cc5e2bd` | SIMD ID packing (nibble tables, shift-merge) | 2.000 / 1.939 (7763) | 5.935 / 5.760 | 0.461 / 0.456 | kept (-3.0%) |
| 5 | `e75b17b` | Write-combining scatter with non-temporal stores, 128-byte stages, one pointer of hot state per partition | 1.591 / 1.553 (9V45) | 4.473 / 4.379 | 0.465 / 0.440 | kept (-2.4%, -2.1%; medians agree on both sizes, ranges overlap) |
| 6 | same image as #5 | `IOMODE=4` (pread into a reused buffer) with 256 KiB and 1 MiB chunks, against mmap | 1.553 / 1.570 / 1.550 | 4.379 / 4.335 / 4.319 | n/a (Linux only) | within noise at 20M, about -1% at 60M; not switched, re-test later |
| 7 | `62abf9c` | Known-date table for F1/F2 timestamps and `ingest_ts` | 1.920 / 1.851 (Xeon 6973P-C; ranges 1.890-2.067 / 1.840-1.910) | 5.418 / 5.227 (5.305-5.612 / 5.165-5.493) | 0.442 / 0.432 | kept (-3.6%, -3.5%) |
| 8 | same image as #7 | `IOMODE=4 CHUNKKB=1024` (pread, 1 MiB) against mmap, second measurement | 1.851 / 1.821 | 5.227 / 5.182 | n/a | -1.6% / -0.9%, same direction as #6; a pread path that never maps the input is queued |
| 5b | `e75b17b` | whole series #2-#5 against `e7a116f` on Intel | 2.243 / 1.920 (Xeon 6973P-C) | 6.339 / 5.418 | | -14.4% / -14.5% (Zen runs showed -6%) |
| 9 | `0a06317` | Sessions phase: 8-byte event keys, 16-byte clicks inside a group, interval-counted assists, largest partition first, reserved user rows | 2.306 / 2.257 (Xeon 8573C; ranges 2.299-2.336 / 2.237-2.293) | 6.479 / 6.342 (6.388-6.562 / 6.247-6.503) | sessions 0.434 / 0.376 at one thread | kept (-2.1%, -2.1%) |
| 10 | n/a | Teammate branch `origin/saran/work` (`6ff3e77`) measured in the same run for reference | 2.459 | 7.067 | | slower than main (2.257 / 6.342); nothing to take |

## Current phase profile (`e75b17b`)

x86, 4 vCPU EPYC 9V45, 20M, in-process seconds (median of 7): links 0.084, parse 0.673, dedup 0.060, sessions 0.214, output 0.035, total about 1.07; the `docker run` wall is 1.553, so about 0.49 s is container start/stop on that runner (an empty-container baseline is now part of every run). 60M: links 0.247, parse 1.948, dedup 0.196, sessions 0.648, output 0.100. The fastest public build runs 0.917 s / 2.468 s wall in the same run, about 0.43 s in-process at 20M: still 2.4 times faster than us in-process.

Parse cut off after each stage, one thread, 20M events only, EPYC 7763 (ns per event line, cumulative seconds in brackets): scan and split 34.5 (0.745), event_id pack and ingest_ts +12.6 (1.017), ts +14.8 (1.337), type, price, user pack, hash +13.0 (1.618), prefetch ring and link lookup +14.9 (1.94), event record push +9.0 (2.13), dedup-key push plus all click lines +31 (2.81). Every stage is slow in absolute terms; the per-line work is instruction-bound, and the two pushes were cache-bound before the write-combining commit.

`perf` self time (4 threads, 20M, before SIMD packing): parse closure 40.8%, scanner 14.4%, `process_partition` 12.0% plus sorts 5%, `parse_clicks` 10.5%, `dedup_partition` 5.4%, kernel page zeroing 2.5%.

Memory at 60M: peak RSS 5.4 GB on the Mac; arena reservation is address space only (about 11 GB at 60M), used 3.3 GB.

## Research notes

The parallel research session keeps `RESEARCH_NOTES.md` (competitor techniques, primary sources). What this session takes from it and from its own reading, with expected gain:

| Idea | Source | Expected gain | Status |
|---|---|---|---|
| Resolve the person at parse time (global link table + 16-line prefetch ring), scatter once by person, dedup on a narrow 16-byte side stream that tombstones losers in place; delete the resolve pass and dedup pass 2 | cherylaurelia `src/main.rs` header, Vibhor7-7 `src/pool.rs`; RESEARCH_NOTES 1.3 | 10 to 15% wall | done (experiments 2, 3): +2% alone, a win with the new scanner |
| Write-combining stages with non-temporal stores for the scatters | Vibhor7-7 BENCH.md exp. 18 (-12% Ice Lake, -6% Zen 4); RESEARCH_NOTES 1.2, 6 | 6 to 12% on Intel | done (experiment 5): -2.4% on Zen; Intel runner not seen yet |
| Line-at-a-time split from the line's own comma/quote masks; quote validity by mask arithmetic | cherylaurelia `fast_split` (ratios 0.90 to 0.95); RESEARCH_NOTES 2.1, 4.1, 10.1 | 3 to 8% | done (experiment 3) |
| SIMD ID packing (nibble tables + multiply-add) | Muła and Lemire base64; cherylaurelia `e3bdd51`; Vibhor exp. 2 (-8.6%) | 2 to 8% | done (experiment 4): -3% |
| Product lookup only for purchases; hoist per-line atomic loads | micah-andelman `44f244e`; RESEARCH_NOTES 1.1, 8.2 | 1 to 2% | done: -2.3% (experiment 1) |
| Known-date table for timestamps | RESEARCH_NOTES 1.4, 15.3 | 2 to 5% of parse | done (experiment 7): -3.6% wall |
| `pread` into an L2-sized buffer instead of mmap | Vibhor exp. 10 (mmap +7.5% on a VM); RESEARCH_NOTES 1.5, 3.2 | -7 to +7%, must be measured | queued |
| Sort 8-byte keys per person; interval assists; 30-minute bins | RESEARCH_NOTES 7.1, 17.1, 1.6 | 2 to 4% | keys and assists done (experiment 9): -2.1%; bins not tried |
| INT32 counts, reserved buffers in the Parquet writer | RESEARCH_NOTES 5 | 1 to 2% | queued |
| Catalog load on a side thread; links tail concurrent with parse | RESEARCH_NOTES 8.1, 1.10 | 1 to 3% | queued |

## Next three ideas queued

1. Find the 0.3 to 0.4 s that `docker run` takes beyond our in-process time plus the empty-container baseline (0.19 s with a tiny dataset): wall-clock stamps at process start and end are in the build being measured now.
2. Links phase (union-find root = smallest ID, no naming pass, catalogs on a side thread): built, passes the gate, x86 A/B running together with partition-count variants.
3. Instruction diet for the per-line path from a cachegrind line profile (1.67 G instructions for the 1M set now, 2.15 G at the start of the night), then a duplicate prefilter for dedup.

## Setup notes

- Data (outside the repo): `~/ss-data/dev` (1M, seed 1), `d20` (seed 7, 20M/5M), `d60` (seed 11, 60M/15M), goldens `gold20`/`gold60` from `1c4fa8e`.
- Gate: `~/ss-data/verify.sh BIN` (fixtures, dev answer, 20M at 1/2/8 threads and 60M against the goldens). The same gate is run on an x86-64-v3 build under Rosetta so the AVX2 paths are covered locally.
- Local A/B: `~/ss-data/ab.py DATASET ROUNDS BIN...` (interleaved, medians and spread).
- x86 A/B: branch `bench`, `.github/workflows/bench.yml` builds each ref's Docker image, runs them interleaved 7 times on 20M and 60M with the scorer's flags, checks output equality with duckdb, runs `cargo test` and the fixtures on x86, and takes a `perf` profile.
