# OVERNIGHT_LOG.md

**SUBMIT THIS: `bae2b62`** (adds the reworked scanner: masks at each line start, pext/pdep field split). x86 runner, full `docker run` wall, median of 9 (20M) and 7 (60M), EPYC 7763 4 vCPU, caches dropped before each dataset, same run: 20M 1.506 s against 1.586 s for the previous pick `fc2fb77`; 60M 4.151 s against 4.373 s (-5.0%, -5.1%). On a fast runner (EPYC 9V45) the previous pick took 1.072 s at 20M where the fastest public build took 0.902 s. Passed the full gate on arm64 and on the x86-64-v3 build, in both I/O modes and with 1 KiB read ranges (36/36 fixtures, dev answer, 20M at 1/2/8 threads and 60M equal to the goldens); unit and fuzz tests and the fixtures pass on the x86 runner; outputs equal there on both datasets; output files read identically with polars, pyarrow, duckdb and fastparquet.

**If you would rather not rely on dictionary-encoded Parquet with an unknown scorer reader, submit `0f7f014` instead** (PLAIN pages, INT32 counts, without tonight's last two speedups: 1.725 / 5.038 s on an EPYC 7763 run where the dictionary build took 1.699 / 4.795 s). Dictionary pages are what pyarrow, Spark, polars and duckdb write by default, so the risk is small, but it is the one change tonight that alters the file format rather than the speed of producing it.

**Why the output size matters:** after the process prints its last line, `docker run` on the runner does not return until the output files have been flushed to disk. That time follows the output size and the runner's disk: at 60M it was 0.42 to 1.10 s with 273 MB, 0.39 s with 223 MB, 0.135 s with 101 MB, 0.117 s with output on tmpfs. Whether the scorer behaves the same is unknown; a smaller output can only help.

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

From experiment 27 on, the bench drops the page cache before each dataset: with both datasets cached the 16 GB runner evicted input mid-run and the 60M figures of some earlier runs (noted in their rows) were polluted. Before/after pairs come from one x86 run each; absolute times differ between runs because the runner CPU differs (EPYC 9V74 in run 1, Xeon 8573C in run 2, where container start/stop alone is about 0.5 s).

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
| 11 | `493f36e` | Links: union-find root = smallest ID (no naming pass), pre-sized lists, catalogs on a side thread | 1.753 / 1.737 (9V74; ranges 1.747-1.800 / 1.730-1.745) | 4.925 / 4.875 | links 0.051 / 0.035 | kept (-0.9%, -1.0%) |
| 12 | same image | `RA=0` (no background read-ahead), `P1BITS=10`, `P1BITS=11 P2BITS=10` | 1.737 / 1.744 / 1.738 / 1.745 | 4.875 / 4.881 / 4.884 / 4.875 | | no difference; defaults stay (matches Vibhor7-7's sweep) |
| 13 | `fe79ca9` | Dedup bitmap prefilter | 1.746 / 1.736 (dedup phase 0.078 / 0.069) | 4.886 / 4.860 (dedup 0.245 / 0.202) | dedup 0.028 / 0.021 | kept on the phase time; wall gain is inside the range |
| 14 | same image | `AREL=1`, `IOMODE=4 CHUNKKB=1024`, both: effect on the time after exit | post-exit 0.406 / 0.405 / 0.401 / 0.399 | 1.100 / 1.099 / 1.087 / 1.080 | | neither changes it; pread wall -1.1% / -1.2% again |
| 15 | `0f7f014` | Parquet: `sessions`, `events` as INT32; one pre-sized buffer per file | 1.743 / 1.725 (7763) ; 1.703 / 1.687 in the run before | 5.178 / 5.038 ; 5.045 / 4.952 | | kept (-1.0%, -2.7%; post-exit 0.465 -> 0.348 s at 60M) |
| 16 | same image | Output on tmpfs instead of the runner's disk (diagnostic) | 1.687 / 1.687 | 4.952 / 4.681 (post-exit 0.348 -> 0.117) | | shows the post-exit time is output writeback |
| 17 | same image | `MIMALLOC_ALLOW_LARGE_OS_PAGES=1` | 1.687 / 1.692 | 4.952 / 4.994 | | no gain; rejected |
| 18 | `38f0f08` | `IOMODE=5`: pread-only input, nothing mapped (512 and 256 KiB ranges) | 1.699 / 1.982 / 1.992 | 4.795 / 5.626 / 5.600 | 0.483 / 0.481 | rejected as default: parse +30% on x86 (0.98 -> 1.28 s), unlike `IOMODE=4` (-1%); cause being investigated (native I/O-mode comparison with rusage and strace is in the next run) |
| 19 | `79cb1f5` | Parquet: dictionary-encoded `user_daily` (user_id runs, day, sessions, events, top_category) | 1.725 / 1.699 | 5.038 / 4.795 (post-exit 0.391 -> 0.135) | out 0.024 / 0.021 | kept (-1.5%, -4.8%) |
| 20 | `50b8e52` | Link table: 4 keys + 4 people per cache line, vector compare, `select_unpredictable` | 1.662 / 1.663 (7763) | 4.607 / 4.554 | 0.528 / 0.505 (noisy) | neutral at 20M, -1.2% at 60M inside the range; kept only as the base of #21 |
| 21 | `a88caa3` | Scanner: single call site so the per-line callback is inlined | 1.212 / 1.188 (9V45; ranges 1.172-1.313 / 1.155-1.194) | 3.345 / 3.263 (3.325-3.505 / 3.243-3.329) | parse 1.250 / 1.191 at one thread | kept (-2.0%, -2.5%) |
| 22 | not committed | Scanner field ends through a small array from two 64-bit words, quote test by byte | not run | not run | parse 1.194 / 1.237 at one thread | rejected on the Mac (slower); not taken to x86 |
| 23 | `598748e` | pread-only input as the default, after fixing its 4K-aliasing copy (page-aligned reads into a page-aligned buffer, 256 KiB ranges) | 1.362 / 1.310 (Xeon 6975P-C; ranges 1.347-1.372 / 1.307-1.322); run before 1.348 / 1.305 | 3.816 / 3.630 ; 3.825 / 3.627 | 0.462 / 0.456 | kept (-3.8%, -4.9%). Before the fix the same mode was +17% (experiment 18): `_copy_to_iter` took 21% of CPU |
| 24 | not merged | Global allocator that puts allocations of 1 MiB and more on huge pages, phase scratch sized to qualify (Vibhor7-7 measured -3.7% for theirs) | 1.317 / 1.310 | 3.620 / 3.630 | 0.462 / 0.456 | rejected: no difference on x86 (sessions phase 0.238 both) |
| 25 | `fc2fb77` | Parquet: `revenue_cents` as a dictionary column when smaller (output 35 -> 22 MB, 101 -> 63 MB) | 1.847 / 1.794 (Xeon 8573C; ranges 1.833-1.881 / 1.784-1.806; post-exit 0.204 -> 0.149) | run unusable (input read from disk); post-exit 0.576 -> 0.360 | out 0.015 / 0.018 | kept (-2.9% at 20M) |
| 26 | `026b2fb` | Scanner field split by pext/pdep on x86 (RESEARCH_NOTES 34.4 stage A) | 1.585 / 1.585 (7763); 1.072 / 1.062 (9V45) | 4.739 / 4.686 (run with a polluted cache) | unchanged (x86 only) | neutral alone; kept as part of #27 |
| 27 | `bae2b62` | Scanner: masks at each line start instead of stored mask streams and windows (stage B) | #26+#27 vs parent: 1.586 / 1.506 (7763, n=9; ranges 1.574-1.701 / 1.495-1.741) | 4.373 / 4.151 (n=7; 4.352-4.488 / 4.146-4.436) | 1.898 / 1.930 at one thread (NEON masks cost more) | kept (-5.0%, -5.1%; parse -8%) |

## Current phase profile (`a88caa3`)

x86, 4 vCPU EPYC 9V45 (a fast runner), in-process seconds, median: 20M links 0.079, parse 0.582, dedup 0.057, sessions 0.174, output 0.024, total 0.916; before `main` 0.087, after the last line 0.180; `docker run` wall 1.188. 60M: links 0.254, parse 1.674, dedup 0.143, sessions 0.571, output 0.069, total 2.71; before 0.089, after 0.456; wall 3.263. A container on a tiny dataset takes about 0.155 s in all, so roughly 0.12 s (20M) and 0.39 s (60M) of the time after exit is ours: output flush (35 / 101 MB) and address-space teardown.

`perf` self time (4 threads, 20M): `parse_events` (scanner and decode, now one function) 50.3%, `process_partition` 14.5% plus sorts 4.5%, `parse_clicks` 9.3%, `dedup_partition` 5.9%, kernel page zeroing 3.3%, link build 4.2%. The line profile is flat: the largest single lines are a record store (4.4%), the scanner's field loop (6.6% over three lines) and ID packing (5.4% over three lines).

Cachegrind, 1M set: 1.69 G instructions (2.15 G at the start of the night), about 1,575 per event line for the whole run; the scanner body alone is still about 315 per line and the decode about 430.

Parse cut off after each stage on an EPYC 7763 before tonight's decode work (ns per event line): scan and split 34.5, event_id and ingest_ts +12.6, ts +14.8, type/price/user/hash +13.0, link lookup +14.9, event push +9.0, dedup-key push and clicks +31.

Memory at 60M: peak RSS about 5.4 GB on the Mac (3.2 GB anonymous plus mapped input).

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

1. Links phase: lookahead prefetch in the pair translation and the serial union-find (built, gated); links are 8% of in-process time at 20M and 10% at 60M.
2. The two scatter pushes still cost about 9 and 6 ns per event line on Zen 4 (stage timing): sweep the partition counts downward (8/8, 8/9 bits; cherylaurelia uses 2^8 at 20M), then prefetch the stage line when the partition is known.
3. Scanner stage C (hand the callback packed offsets instead of a slice array), guided by the cachegrind line table.

## Setup notes

- Data (outside the repo): `~/ss-data/dev` (1M, seed 1), `d20` (seed 7, 20M/5M), `d60` (seed 11, 60M/15M), goldens `gold20`/`gold60` from `1c4fa8e`.
- Gate: `~/ss-data/verify.sh BIN` (fixtures, dev answer, 20M at 1/2/8 threads and 60M against the goldens). The same gate is run on an x86-64-v3 build under Rosetta so the AVX2 paths are covered locally.
- Local A/B: `~/ss-data/ab.py DATASET ROUNDS BIN...` (interleaved, medians and spread).
- x86 A/B: branch `bench`, `.github/workflows/bench.yml` builds each ref's Docker image, runs them interleaved 7 times on 20M and 60M with the scorer's flags, checks output equality with duckdb, runs `cargo test` and the fixtures on x86, and takes a `perf` profile.
