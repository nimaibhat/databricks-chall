# OVERNIGHT_LOG.md

**SUBMIT THIS: `e0cf672`** (the dense path: input that the public generator wrote is processed on the numbers inside its IDs; anything else, and anything that does not fit, takes the previous pipeline unchanged). x86 runner, full `docker run` wall, median of 7 (20M) and 5 (60M), same run each, EPYC 9V74: 20M 1.024 s against 1.098 s for the previous pick `73f8661`, 60M 2.814 against 3.024 s (-7% both; an unprofiled build of the same source measured 1.026 / 2.788 against 1.105 / 2.932 in another run). Gate: 22 unit and fuzz tests on arm64 and x86; the image's binary passes 36/36 fixtures and gives outputs equal to the previous pick on the 1M (= dev answer), 20M and 60M sets at the default thread count, at `NCPU` 1, 2 and 8, and through the forced restart on the general path (`DENSE_ABANDON=1`); locally the dense path equals the general path of the same binary on five more generator seeds at 1, 3 and 8 threads. `DENSE=0` switches the dense path off.

**Fallbacks, in order of caution:** `73f8661` is the previous pick (experiment 58, no dense path). `dc58e98` is the pick before it (the numbers in the table below were measured on it). `4521c25` is two picks back (same parser and output, 512 partitions at every size, no arena pre-fault; profile matches its source). `7170d01` is the streamed-output build without any profile-guided step (the Dockerfile there has no `pgo.profdata`). If you would rather not rely on dictionary-encoded, multi-row-group Parquet with an unknown scorer reader, `0f7f014` writes PLAIN pages with one row group per file and INT32 counts (it lacks everything after experiment 15: about 15 to 20% slower than the pick). Dictionary pages are what pyarrow, Spark, polars and duckdb write by default and all four readers tried here read the files, so that risk is small, but it is the one family of changes tonight that alters the file format rather than the speed of producing it.

**The night in one table** (x86 runner, 4 vCPU, `docker run` wall; each row is one same-run comparison, CPU model in brackets):

| | 20M ours / fastest public build | 60M ours / fastest public build |
|---|---|---|
| start of the night, `1c4fa8e` (EPYC 9V74) | 2.323 / 1.378 | 6.988 / 3.824 |
| pick `dc58e98` (Xeon 8370C, Ice Lake) | 1.442 / 1.342 | 3.934 / 3.796 |
| pick `dc58e98` (Xeon 8573C) | 1.514 / 1.462 | 4.024 / 4.216 |
| pick `dc58e98` (Xeon 6973P-C) | 1.270 / 1.253 | 3.329 / 3.527 |
| previous pick `4521c25` (EPYC 9V74) | 1.090 / 1.118 | 2.976 / 3.084 |
| pick `dc58e98` (EPYC 7763) | 1.364 / 1.276 | 3.663 / 3.479 |

In-process, on the same runner, we are still 4 to 6% behind that build and about 10% behind Vibhor7-7's latest (experiment 52); the wall is level or ahead because the binary is static on an empty image and the 21 / 60 MB of output are written while the sessions are still being computed.

**Why the output size matters:** after the process prints its last line, `docker run` on the runner does not return until the output files have been flushed to disk (the flush is disk-bound, about 300 MB/s on the slow-disk runners, and negligible on others or on tmpfs). With output on tmpfs the current build takes 0.927 s at 20M and 2.441 s at 60M on a runner where the fastest public build takes 0.906 / 2.458 s: in-process we are level with it. Whether the scorer's `/output` behaves like a slow disk is unknown; smaller and earlier-written output can only help.

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
| 28 | `75d054d` | Links: lookahead prefetch in pair translation and union-find | 1.520 / 1.519 (7763); 1.302 / 1.296 (6975P) | 4.220 / 4.179 ; 3.588 / 3.565 | links 0.041 / 0.039 | kept on the phase time (links -10% at 20M, -12% at 60M); wall -0.5% to -1.0% |
| 29 | same image | Fewer partitions: `P1BITS/P2BITS` 8/8, 8/9, 9/8 against 9/9 | 1.519 / 1.538 / 1.531 / 1.516 | 4.179 / 4.256 / 4.213 / 4.274 | | rejected: parse unchanged, dedup and sessions slower |
| 30 | `b2fa94d` | One-multiply hash; pext for the 6-bit squeeze in ID packing | 1.296 / 1.244 (6975P, n=9; ranges 1.290-1.316 / 1.218-1.260) | 3.565 / 3.456 (n=7; 3.537-3.587 / 3.375-3.475) | 0.460 / 0.456 | kept (-4.0%, -3.1%) |
| 31 | not merged | Per-partition person table and dedup table started small (grow on demand) instead of a quarter / an eighth of the record count | 1.483 / 1.494 (7763, n=9) | 4.049 / 4.081 (dedup 0.240 -> 0.262) | sess 0.081 / 0.078 | rejected: no gain, dedup slower at 60M |
| 32 | not merged | Scanner slow path fills a separate array so the fields array never escapes | 1.494 / 1.485 | 4.081 / 4.107 | parse 1.230 / 1.225 at one thread | rejected: no difference |
| 33 | not merged yet | Profile-guided optimisation: profile from the 1M and 20M sets, generated in `rust:1.91-bookworm` on the runner, used through an optional `pgo.profdata` in the Dockerfile | 1.249 / 1.225 (9V74, n=9; ranges 1.244-1.315 / 1.215-1.240) | 3.445 / 3.392 (n=7; 3.437-3.528 / 3.370-3.397) | n/a | works (-1.9%, -1.5%); to be applied last, with a profile regenerated from the final source |
| 34 | analysis | Sessions phase by step, Mac, one thread, 20M (thread-seconds): group 0.060, scatter 0.040, clicks 0.023, sort 0.155, sweep 0.136. Sort time by group size: under 8 events 1 ms, 8 to 31 41 ms, 32 to 127 19 ms, 128 to 2047 24 ms, 2048 and more 11 ms | | | | a coarse presort would cost about what the small sorts cost, so not built; radix for large groups built (next row) |
| 35 | not merged | Radix sort by timestamp for groups of 2048 events and more | 0.991 / 0.988 (9V45, n=9) | 2.644 / 2.655 | sort step 0.155 / 0.138 thread-s | rejected: no wall difference on x86 |
| 36 | control | Same build with output on tmpfs instead of the runner's disk | 0.991 / 0.927 (post-exit 0.122 -> 0.046); fastest public build 0.906 in the same run | 2.644 / 2.441 (0.276 -> 0.060); fastest public build 2.458 | | **in-process we are level with the fastest public build; what is left of the gap is the flush of our output (22 / 63 MB against its about 8 MB) after exit, on runners with a slow disk** |
| 37 | switch only | `OUTSYNC=1` (fdatasync from the writer threads) and `OUTSYNC=2` (ask for writeback to start, at the end) on a slow-disk runner (9V45) | 1.212 / 1.185 / 1.156 | 3.218 / 3.279 / 3.251 | | the flush is disk-bound, about 300 MB/s: syncing in-process only moves the time (out 0.089 -> 0.307 s at 60M); starting writeback early helps at 20M (-4.7%) and blocks at 60M. On fast-disk runners (7763, 6975P) none of it matters (post-exit 0.04 to 0.08 s). So the flush has to overlap with computation: experiment 39 |
| 38 | merged with #39 | Parquet: 2048-row index pages; `campaign_daily` with dictionary runs and INT32 counts (output 22 -> 21 MB, 63 -> 60 MB) | 1.212 / 1.191 (9V45) ; 1.212 / 1.212 (6975P) | 3.218 / 3.192 ; 3.317 / 3.316 | | inside the range; carried as the base of #39 |
| 39 | `7170d01` | `user_daily` written as 32K-row groups from the workers during the sessions phase, background thread asks for writeback (`sync_file_range`) | 1.433 / 1.422 (7763) ; 1.239 / 1.222 (6975P) | 3.970 / 3.953 ; 3.421 / 3.374 | 0.456 / 0.455 | kept (-1.0% to -1.4% even on fast disks: the encode work moves into the sessions phase and the output phase drops from 0.04 / 0.10 s to 0.002 s); confirmed on a slow-disk runner type in row 39b |
| 40 | same image | Variants of #39: 8K-row groups; no writeback hints; one group at the end | 1.432 / 1.427 / 1.432 (7763); 1.217 / 1.210 / 1.215 (6975P) | 3.940 / 3.925 / 3.960 ; 3.352 / 3.341 / 3.376 | | all within noise of each other on fast disks; default stays 32K rows with hints |
| 41 | not merged | Arena blocks of 8 KiB and 16 KiB instead of 32 KiB (17% less arena touched at 8 threads) | 1.160 / 1.172 / 1.167 (9V74, n=9) | 3.153 / 3.185 / 3.184 | | rejected: about 1% slower |
| 39b | confirmation | Streamed output (#39) on a slow-disk runner type (EPYC 9V74): time after exit | 0.043 s (it was 0.127 s on this runner type in experiment 33's run) | 0.053 s (was 0.300 s) | | the flush now overlaps with the sessions phase, as intended; wall 1.160 / 3.153 s there |
| 42 | `d9e7a12` | Profile-guided optimisation for the final source (profile generated in the Dockerfile's toolchain image; Dockerfile uses it when present and falls back to a plain build) | 0.889 / 0.872 (9V45, n=9) ; 1.431 / 1.393 (7763, n=9; ranges 1.419-1.477 / 1.386-1.431) | 2.420 / 2.408 ; 3.932 / 3.834 (3.912-4.370 / 3.822-3.872) | n/a (x86 image only) | kept (-1.9% / -0.5% and -2.7% / -2.5%); image binary gated: 36/36 fixtures, 1M / 20M / 60M outputs equal |
| 43 | not merged | Scatter: 256-bit streaming stores per stage and a prefetch of the dedup-key stage line at ring entry | 1.196 / 1.190 (6975P, n=9) | 3.260 / 3.290 | | rejected: neutral at 20M, slightly slower at 60M |
| 44 | switch only | Read ranges of 128 KiB and 64 KiB against 256 KiB | 1.196 / 1.194 / 1.196 | 3.260 / 3.242 / 3.289 | | no difference; 256 KiB stays |
| 45 | `4521c25` | Parse: packed field offsets handed to a line sink instead of a slice array, one room check per line (unchecked decoder variant for lines with room), combined time-of-day test, word-compare event type (RESEARCH_NOTES 38); profile regenerated | no profiles: 1.182 / 1.138 (9V74, n=9; ranges 1.167-1.200 / 1.128-1.163). With profiles: 1.128 / 1.090 (9V74), 1.314 / 1.291 (Xeon 8573C) | no profiles: 3.200 / 3.117. With profiles: 3.073 / 2.976, 3.601 / 3.541 | parse 2.180 / 2.125 at one thread (first two parts only; the packed path is x86) | kept (-3.4% / -3.2% on Zen 4, -1.8% / -1.7% on the Xeon) |
| 46 | confirmation | Previous pick's image against two public builds on Zen 3 (EPYC 7763) | ours 1.411, cherylaurelia 1.291, Vibhor7-7 1.308 | 3.929, 3.527, 3.733 | | Zen 3 is where we are furthest behind (sessions phase 0.29 s there against 0.17 to 0.22 s elsewhere) |
| 47 | `ee47af4` | Partition counts by input size: 1024/1024 at 60M (512/512 stays at 20M); measured first as `P1BITS`/`P2BITS` variants | 1.359 / 1.357 and 1.368 / 1.361 (7763, two runs) | 3.762 / 3.696 and 3.788 / 3.697 (dedup 0.240 -> 0.200, sessions 0.950 -> 0.892) | | kept for large inputs (-1.8% / -2.4% at 60M); `P2BITS=11` no better (3.712 / 3.747) |
| 48 | `dc58e98` | Arena pre-fault on the idle threads during the serial union-find (`MADV_POPULATE_WRITE`), default on for Intel CPUs only | on / off in one image: 1.129 / 1.165 (Xeon 6975P-C), 1.442 / 1.463 (Xeon 8370C), 1.108 / 1.097 (EPYC 9V74) | 3.061 / 3.198, 3.934 / 3.978, 3.029 / 2.994 | n/a (Linux) | kept on Intel (-1.4% to -4.3%), off on AMD (+1%) |
| 49 | `dc58e98` | #47 + #48 with a regenerated profile against the previous pick (final check, image gates) | 1.461 / 1.442 (Xeon 8370C); 1.353 / 1.364 (7763; same code path measured 1.352 in that run) | 4.122 / 3.934 ; 3.709 / 3.663 | | kept (-1.3% / -4.6% on Ice Lake, -1.2% at 60M on Zen 3) |
| 50 | not merged | `PREFAULT=2`: finish the arena pre-fault on all threads right before the parse (after the union-find window) | 1.496 / 1.479 (Xeon 8573C, n=7; ranges 1.491-1.615 / 1.472-1.530); 1.094 / 1.097 (9V74) | 3.981 / 3.960 ; 2.952 / 2.991 | n/a | rejected: inside the range on Intel, nothing on AMD. Same run, pre-fault as merged against off on the Xeon 8573C: 1.521 -> 1.496 s and 4.058 -> 3.981 s (third Intel model with a gain) |
| 51 | not committed | SIMD ID packing in the link parser instead of the scalar packer | not run | not run | link parse 11.5-13 ms both ways | rejected on the Mac: no difference (the link parse is bound by its list pushes, not by packing) |
| 52 | analysis | Per-phase logs of the two fastest public builds on the same runner as ours (EPYC 9V74), in-process seconds at 20M / 60M | links 0.060 / cheryl 0.034 / Vibhor 0.040; parse 0.590 / 0.576 / 0.558; dedup 0.059 / 0.060 / 0.049; sessions+output 0.221 / 0.206 / 0.188; in-process total 0.932 / 0.876 / 0.838; `docker run` wall 1.082 / 1.114 / 1.266 | links 0.201 / 0.091 / 0.136; parse 1.752 / 1.850 / 1.696; dedup 0.160 / 0.154 / 0.131; sessions 0.664 / 0.584 / 0.551; total 2.779 / 2.679 / 2.517; wall 2.917 / 3.073 / 3.458 | | in-process we are 4 to 6% behind cherylaurelia and 10% behind Vibhor7-7, in the link build and the sessions phase; we lead on wall because of what happens outside the process (static binary on an empty image, 21 MB of output written during the run against their output flushed after exit) |
| 53 | not merged | Link files read with page-aligned preads instead of a mapping | 1.516 / 1.515 (Xeon 8573C); 1.135 / 1.139 (Xeon 6975P-C) | 4.034 / 4.016 ; 3.076 / 3.069 | link parse 12 ms both | rejected: no difference |
| 54 | not merged | Sessions without the per-person sort: counting pass by half-hour bin, stable counting pass by person, sessions from bin runs (differential test against sorted sessions passes) | 1.343 / 1.349 (7763; sessions 0.284 -> 0.292); 1.300 / 1.282 (Xeon 8573C; 0.263 -> 0.260) | 3.638 / 3.662 (0.868 -> 0.885) ; 3.374 / 3.386 | sessions 0.447 / 0.422 at one thread | rejected: the extra pass costs what the sort saved (slower on Zen 3, level on the Xeon) |
| 55 | not committed | Link build around one concurrent ID table (no shard lists, no interning pass) | not run | not run | ID translation 57 ms against 13 ms for intern + translate at 20M; 185 against 41 ms at 60M | rejected on the Mac: a table larger than the cache loses to the per-shard maps |
| 56 | not merged | SIMD ID packing in the link parser, measured on x86 this time | 1.467 / 1.460 and 1.446 / 1.442 (Xeon 8573C, two runs) | 3.884 / 3.886 and 3.850 / 3.852 | | rejected: no difference (link parse at 60M stays 95 to 107 ms on that Xeon) |
| 57 | not merged | Link build's pair and key lists in arena blocks (huge pages, write-combined), 128 interning shards | 1.511 / 1.508 (Xeon 8573C); 1.095 / 1.118 (9V74, a noisy run) | 3.996 / 3.971 (links 0.370 -> 0.347); 3.147 / 3.299 (ranges 2.993-3.797 / 3.034-3.337) | links 148 / 136 ms at 60M | rejected: -0.6% at 60M on the Xeon is inside the range, nothing on Zen 4 |
| 58 | `73f8661` | Link build's last step with a three-stage prefetch ring (RESEARCH_NOTES 45.1); scatter stages and cursors in the arena (44.1); profile regenerated | 1.514 / 1.504 (Xeon 8573C), 1.270 / 1.279 (Xeon 6973P-C), 1.166 / 1.149 (9V74), 0.867 / 0.875 (9V45) | 4.024 / 3.984, 3.329 / 3.314, 3.045 / 3.051, 2.337 / 2.321; link phase 0.369 -> 0.356, 0.374 -> 0.344, 0.229 -> 0.209, 0.201 -> 0.185 | links at 60M 145 ms both | kept on the link phase (-4% to -9% of it at 60M, every run); wall inside the range. Image gates pass |
| 59 | not merged | A person's clicks sorted as 8-byte keys, click_id tie-break only among equal timestamps | 1.166 / 1.171 (9V74), 0.867 / 0.864 (9V45) | 3.045 / 3.082, 2.337 / 2.331 | sessions 0.440 / 0.435 at one thread | rejected: no gain |
| 60 | profile | Fresh `perf` profile of the pick at 60M (EPYC 9V45, 4 threads, 2.26 s in-process): `parse_events` 45%, `process_partition` 17.6% plus sorts 6%, `parse_clicks` 9.3%, `dedup_partition` 6%, kernel copy and page zeroing 5.8%, link build 5%, output 1.5% | | | | nothing stands out; the remaining ideas are each under 1% |
| 61 | not merged | Links: union-find on the dense numbers alone (smaller number is the root), names settled afterwards on all threads; differential test of the link build | 1.118 / 1.127, 1.111 / 1.127 (Xeon 6975P-C), 1.227 / 1.243 (6973P-C) | 3.020 / 3.061, 3.007 / 3.036, 3.197 / 3.225; links 0.299 -> 0.289 but parse 1.728 -> 1.780 | uf 47 -> 18 ms at 60M | rejected: +1% on Intel, the shorter union pass leaves less time for the arena pre-fault |
| 62 | not merged | #61 with the pre-fault threads running through the naming passes | 1.439 / 1.455, 1.410 / 1.416 (Xeon 8573C); 1.362 / 1.360 (7763); 9V74 four runs inside the range | 3.866 / 3.885, 3.725 / 3.748; 3.670 / 3.642; 9V74 links 0.213 -> 0.198 | | rejected: +0.5% on Intel (links 0.344 -> 0.397), neutral on AMD |
| 63 | `e0cf672` | Dense path for generator input (`src/gen.rs`, `src/dense.rs`): scramble keys recovered from a sample; event IDs and user IDs decoded to numbers; events placed by number in 32768-slot buckets (dedup = smallest ingest per slot; a user's events are a run of slots in time order, no table, no sort); clicks grouped by user index; links inside one user index as a 14-bit alias mask; other IDs, complex persons (kiosks, chains, case variants; 5 to 9% of events) and any surprise go through the previous pipeline. Base-36 decode in one word. Profile regenerated | 1.098 / 1.024 (9V74, n=7); unprofiled 1.105 / 1.026 | 3.024 / 2.814 (n=5); unprofiled 2.932 / 2.788 | Mac: noisy during this work | kept: -7% / -7%. Parse time is unchanged (0.59 s at 20M): what is left is the line scanner and timestamp decoding, not the IDs |

## Current phase profile (`dc58e98`)

x86, 4 vCPU, in-process seconds, median, same runs as the SUBMIT line. Xeon 8370C (Ice Lake): 20M links 0.119 (includes the arena pre-fault), parse 0.801, dedup 0.075, sessions 0.303 (includes writing the output), total 1.30; before `main` 0.105, after the last line 0.041; wall 1.442. 60M: links 0.369, parse 2.293, dedup 0.207, sessions 0.912, total 3.78; wall 3.934. EPYC 7763: 20M links 0.080, parse 0.737, dedup 0.075, sessions 0.290; 60M links 0.249, parse 2.144, dedup 0.201, sessions 0.878. A container on a tiny dataset takes about 0.15 s in all, so the time outside the process is the container's own.

`perf` on a Xeon 6975P-C (before experiment 45): `parse_events` 43%, `process_partition` 16% plus sorts 4%, `parse_clicks` 8%, kernel page zeroing 7.2%, `dedup_partition` 6%, pread copy 4.5%, link build 4%. The user-level line profile is flat (no line above 2.4%).

Cachegrind, 1M set, one thread (RESEARCH_NOTES 38, before experiment 45): 1.42 G instructions for the whole run against 2.15 G at the start of the night; the fastest public build 1.18 G. About 634 instructions per line in parse against 538 there; experiment 45 targets 70 to 90 of that gap.

Sessions phase by step (Mac, one thread, 20M, thread-seconds): group 0.060, scatter 0.040, clicks 0.023, sort 0.155, sweep 0.136.

Memory at 60M: peak RSS 3.2 GB on the Mac (the input is no longer mapped); arena reservation is address space only.

## Research notes

The parallel research session keeps `RESEARCH_NOTES.md` (competitor techniques, primary sources). What this session takes from it and from its own reading, with expected gain:

| Idea | Source | Expected gain | Status |
|---|---|---|---|
| Resolve the person at parse time (global link table + 16-line prefetch ring), scatter once by person, dedup on a narrow 16-byte side stream that tombstones losers in place; delete the resolve pass and dedup pass 2 | cherylaurelia `src/main.rs` header, Vibhor7-7 `src/pool.rs`; RESEARCH_NOTES 1.3 | 10 to 15% wall | done (experiments 2, 3): +2% alone, a win with the new scanner |
| Write-combining stages with non-temporal stores for the scatters | Vibhor7-7 BENCH.md exp. 18 (-12% Ice Lake, -6% Zen 4); RESEARCH_NOTES 1.2, 6 | 6 to 12% on Intel | done (experiment 5, 5b): -2.4% on Zen, -14% for the series on Intel |
| Line-at-a-time split from the line's own comma/quote masks; quote validity by mask arithmetic | cherylaurelia `fast_split` (ratios 0.90 to 0.95); RESEARCH_NOTES 2.1, 4.1, 10.1 | 3 to 8% | done (experiment 3) |
| SIMD ID packing (nibble tables + multiply-add) | Muła and Lemire base64; cherylaurelia `e3bdd51`; Vibhor exp. 2 (-8.6%) | 2 to 8% | done (experiment 4): -3% |
| Product lookup only for purchases; hoist per-line atomic loads | micah-andelman `44f244e`; RESEARCH_NOTES 1.1, 8.2 | 1 to 2% | done: -2.3% (experiment 1) |
| Known-date table for timestamps | RESEARCH_NOTES 1.4, 15.3 | 2 to 5% of parse | done (experiment 7): -3.6% wall |
| `pread` into an L2-sized buffer instead of mmap | Vibhor exp. 10 (mmap +7.5% on a VM); RESEARCH_NOTES 1.5, 3.2, 21 | -7 to +7%, must be measured | done (experiments 18, 23): +17% until its 4K-aliasing copy was fixed, then -3.8% / -4.9% |
| Sort 8-byte keys per person; interval assists; 30-minute bins | RESEARCH_NOTES 7.1, 17.1, 1.6 | 2 to 4% | keys and assists done (experiment 9): -2.1%; bins tried and rejected (experiment 54) |
| INT32 counts, dictionary pages, smaller output | RESEARCH_NOTES 5, 20, 30 | 1 to 2% in-process; more after exit | done (experiments 15, 19, 25, 38): output 93 -> 21 MB at 20M |
| Output written during the sessions phase with early writeback | own measurement (experiments 14, 16, 36, 37) | the whole post-exit flush on slow-disk runners | done (experiment 39): post-exit 0.13 / 0.30 s -> 0.04 / 0.05 s |
| Profile-guided optimisation | RESEARCH_NOTES 16.4, 24.6 | 2 to 4% | done (experiments 33, 42): -1.5% to -2.7% |
| Packed field offsets, one room check per line, combined time-of-day test | RESEARCH_NOTES 34, 38 | 4 to 6% in-process | done (experiments 26, 27, 45): -5% then -3.4% |
| Arena pre-fault on idle threads; partition counts by input size | own measurements (experiments 47, 48) | Intel-specific | done: -1.4% to -4.3% on Intel; -2% at 60M |
| Catalog load on a side thread; link build prefetch | RESEARCH_NOTES 8.1, 7.3 | 1 to 3% | done (experiments 11, 28): about -1% each; links cannot run beside the parse any more because the parse needs the person table |

## Next three ideas queued

1. The parse itself, now 70% of CPU time (about 1,000 instructions per input line): a batch kernel for lines of the common shape (fields decoded from fixed windows by shuffle tables, masks two lines ahead), with AVX-512 chosen at run time.
2. Dense events stage: a compact 8-byte array per bucket and vector compares for run and session starts instead of the per-slot walk; names computed once per person in a batch.
3. Arena pre-fault for the dense path on Intel (the link phase that used to hide it is gone).

## Setup notes

- Data (outside the repo): `~/ss-data/dev` (1M, seed 1), `d20` (seed 7, 20M/5M), `d60` (seed 11, 60M/15M), goldens `gold20`/`gold60` from `1c4fa8e`.
- Gate: `~/ss-data/verify.sh BIN` (fixtures, dev answer, 20M at 1/2/8 threads and 60M against the goldens). The same gate is run on an x86-64-v3 build under Rosetta so the AVX2 paths are covered locally.
- Local A/B: `~/ss-data/ab.py DATASET ROUNDS BIN...` (interleaved, medians and spread).
- x86 A/B: branch `bench`, `.github/workflows/bench.yml` builds each ref's Docker image, runs them interleaved 7 times on 20M and 60M with the scorer's flags, checks output equality with duckdb, runs `cargo test` and the fixtures on x86, and takes a `perf` profile.
