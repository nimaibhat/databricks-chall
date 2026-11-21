# OVERNIGHT_LOG.md

**SUBMIT THIS: `e7a116f`** (category lookup only for purchases). x86 runner, full `docker run` wall, median of 7, same run as its parent: 20M 2.822 s vs 2.889 s, 60M 8.056 s vs 8.253 s (Xeon 8573C, 4 vCPU; ranges do not overlap). Passed the full gate: 36/36 fixtures, dev answer, 20M at 1/2/8 threads and 60M equal to the goldens, same on the x86-64-v3 build, outputs equal on the x86 runner. It is the fastest build that has passed everything; larger changes are still being measured.

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

## Current phase profile (`1c4fa8e`)

x86, 4 vCPU, 20M, in-process seconds (median of 7): links 0.112, parse 1.282, dedup 0.147, resolve 0.175, sessions 0.329, output 0.048; container start/exit adds about 0.2 s. At 60M: links 0.320, parse 3.803, dedup 0.453, resolve 0.556, sessions 1.083, output 0.146.

One thread, 20M, x86: total 5.0 s (user 4.74, sys 0.27); parse 2.97, dedup 0.37, resolve 0.54, sessions 0.86, output 0.10. Four threads: user 7.49 s, sys 0.43 s, so the two hyperthreads per core add about 60% CPU time for the same work.

`perf` self time (4 threads, 20M): `parse_events` closure 30.2%, `parse_events` (scanner) 19.5%, `process_partition` 10.6% plus sorts 4.5%, `resolve_partition` 9.3%, `dedup_partition` 6.9%, `parse_clicks` 7.9%, kernel page zeroing 2.2%. Hot source lines: `pack_fast` loop (keys.rs:87-88, 5.8%), the scanner's per-field quote loop (parse.rs:628-634, 7.7%), hash-map probes (keys.rs:206/228, 5.2%), `BList::push` (mem.rs:129-139, 3.1%).

Cachegrind (1M set, earlier run): about 1,040 instructions per event line in parse: scanner 490, ID packing and catalog 230, timestamps and price 230, record build 90.

Memory at 60M: peak RSS 5.4 GB on the Mac, arena 3.3 GB used. Exit teardown is inside the 0.2 s container overhead and not yet split out.

## Research notes

The parallel research session keeps `RESEARCH_NOTES.md` (competitor techniques, primary sources). What this session takes from it and from its own reading, with expected gain:

| Idea | Source | Expected gain | Status |
|---|---|---|---|
| Resolve the person at parse time (global link table + 16-line prefetch ring), scatter once by person, dedup on a narrow 16-byte side stream that tombstones losers in place; delete the resolve pass and dedup pass 2 | cherylaurelia `src/main.rs` header, Vibhor7-7 `src/pool.rs`; RESEARCH_NOTES 1.3 | 10 to 15% wall | next |
| Write-combining stages with non-temporal stores for the scatters | Vibhor7-7 BENCH.md exp. 18 (-12% Ice Lake, -6% Zen 4); RESEARCH_NOTES 1.2, 6 | 6 to 12% on Intel | queued |
| Line-at-a-time split from the line's own comma/quote masks; quote validity by mask arithmetic | cherylaurelia `fast_split` (ratios 0.90 to 0.95); RESEARCH_NOTES 2.1, 4.1, 10.1 | 3 to 8% | queued |
| SIMD ID packing (nibble tables + multiply-add) | Muła and Lemire base64; cherylaurelia `e3bdd51`; Vibhor exp. 2 (-8.6%) | 2 to 8% | queued |
| Product lookup only for purchases; hoist per-line atomic loads | micah-andelman `44f244e`; RESEARCH_NOTES 1.1, 8.2 | 1 to 2% | done: -2.3% (experiment 1) |
| Known-date table for timestamps | RESEARCH_NOTES 1.4 | 2 to 5% of parse | queued |
| `pread` into an L2-sized buffer instead of mmap | Vibhor exp. 10 (mmap +7.5% on a VM); RESEARCH_NOTES 1.5, 3.2 | -7 to +7%, must be measured | queued |
| Sort 8-byte keys per person; 30-minute bins | RESEARCH_NOTES 7.1, 1.6 | 2 to 4% | queued |
| INT32 counts, reserved buffers in the Parquet writer | RESEARCH_NOTES 5 | 1 to 2% | queued |
| Catalog load on a side thread; links tail concurrent with parse | RESEARCH_NOTES 8.1, 1.10 | 1 to 3% | queued |

## Next three ideas queued

1. Person at parse time + single scatter + tombstone dedup (structural).
2. Write-combining with streaming stores on the scatters.
3. Line-at-a-time scanner with mask-based quote validation, together with SIMD ID packing.

## Setup notes

- Data (outside the repo): `~/ss-data/dev` (1M, seed 1), `d20` (seed 7, 20M/5M), `d60` (seed 11, 60M/15M), goldens `gold20`/`gold60` from `1c4fa8e`.
- Gate: `~/ss-data/verify.sh BIN` (fixtures, dev answer, 20M at 1/2/8 threads and 60M against the goldens). The same gate is run on an x86-64-v3 build under Rosetta so the AVX2 paths are covered locally.
- Local A/B: `~/ss-data/ab.py DATASET ROUNDS BIN...` (interleaved, medians and spread).
- x86 A/B: branch `bench`, `.github/workflows/bench.yml` builds each ref's Docker image, runs them interleaved 7 times on 20M and 60M with the scorer's flags, checks output equality with duckdb, runs `cargo test` and the fixtures on x86, and takes a `perf` profile.
