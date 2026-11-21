# OVERNIGHT_LOG.md

Experiment log for the coding session, on the codebase as of `941879e`. The research session reads this.

Test machine: AWS `m5zn.2xlarge` (Xeon Platinum 8252C, Cascade Lake, 4 cores / 8 threads, 32 GB), Ubuntu 24.04.
Every candidate is built with the repo `Dockerfile`, then: 36 fixtures and the dev answer through `scripts/docker_run.sh`
(CPUS=8), and its output on the big set compared with the baseline's output.
Timing: the static binary taken out of the image, run directly on a generated set (seed 20, 20M events, 5M clicks, 1.8 GB),
page cache warm, `NCPU=8`, candidates alternated in one loop, one warm-up round dropped. Wall is process start to exit in ms.

Baseline `941879e`: wall min 197.8, median 198.6, max 199.9 over 9 rounds. Stages: parse 146.3, ev3a 35.5, write 9.4.
Noise is about ±1 ms, so a change needs roughly 2 ms (1%) to count.

| # | Change | Result (min / median wall, ms) | Verdict |
|---|---|---|---|
| 1 | Profile-guided build: instrumented build in the same `rust:1.100.0-alpine` image, trained on seed 20 (20M), seed 3 (10M) and dev, `-Cprofile-use` in the Dockerfile. Needed a `__llvm_profile_write_file()` call before `_exit`. | base 197.8 / 198.6, pgo 197.5 / 198.5. Parse 146.3 vs 146.1. Exact on fixtures, dev and big. | Rejected: no measurable gain, and it adds a pinned toolchain and a binary profile to the build. |
| 2 | Every runtime knob flipped one at a time, data on ext4 page cache (`XL_PREFAULT=1`, `XL_NOHUGE=1`, `XL_FIRST4K=1`, `XL_ANONR=0`, `XL_PINSTART=1`, `XL_NOTOUCH=0`, `XL_PPARTS=0`, `XL_POOL=0`, `XL_ALLOC=0`, `XL_POPW=0`, `XL_RECYCLE=0`, `XL_FDALL=0`, `XL_PSETUP=0/1`, `XL_SPIN=0/200`, `XL_OVL=0`, `NCPU=4/6/12/16`, `XL_RD=64..1024`, `XL_PIN=0/1`, `XL_QORD=0/1`, `XL_CH=2048/4096/16384`, `XL_PF=1`, `XL_NT=0`, `XL_REL=0`, `XL_FREE=0`, `XL_FA=0/1`) | Base 198. Nothing better than base beyond noise. Clearly worse: `NOHUGE` 221, `ALLOC=0` 220, `NT=0` 220, `RD=*` 226-239, `FA=0` 226, `QORD=0/1` 207, `RECYCLE=0` 207, `POOL=0` 202, `NCPU=12` 210, `NCPU=16` 217, `NCPU=6` 250, `NCPU=4` 369. | Rejected: the defaults are the optimum on this machine too. |
| 3 | Same on tmpfs input (see "tmpfs" below): `XL_CH=12288/16384/32768/65536` over 15 rounds, `XL_REL=0`, `XL_RD=128`, `XL_FA=0`, `XL_PREFAULT=1`, `XL_PF=1`, `XL_PIN=0`, `XL_QORD=0` | Base 209.2 / 210.1. `CH=65536` 208.0 / 209.2, `CH=32768` 208.3 / 209.8, `CH=16384` 209.0 / 210.2. `REL=0`: parse 150.7 (-7.7) but write 31.6 (+22), wall 224. | Rejected: chunk size is within noise (under 1 ms). Not releasing the input mapping moves the unmap to one thread at the end and costs 14 ms net. |
| 4 | `MADV_POPULATE_READ` on each chunk just before it is parsed (research idea 1, third variant), behind a test knob | Off 209.2 / 210.3, on 209.5 / 210.7; with `CH=32768` 208.9 / 209.9; with `CH=2048` 212.2 / 213.6 | Rejected: no gain. Reverted. |
| 5 | Research row 1: no per-chunk release, parallel release at the end (`XL_REL=0 XL_RGREL=0 XL_ENDREL=2`), and the neighbours | Base 209.1 / 210.4. Row 1 combination 209.4 / 209.9: the in-process total drops to 200.8 (parse 151.0) but the release then takes about 8.6 ms after it. `REL=0 ENDREL=2` 225.5; `REL=0 RGREL=0 ENDREL=1` 227.1; `REL=0 RGREL=0` (leave it to exit) 227.2. | Rejected: unmapping 1.8 GB of 4 KiB input pages costs about 8 ms of wall on 8 threads wherever it is done, and 18 to 26 ms on one thread. |
| 6 | Research row 2: `XL_CKIX=1`, `XL_CKIX=2` | `CKIX=1` 216.0 / 216.9 (ev3a 41.0, +5.5); `CKIX=2` 209.4 / 210.6 | Rejected: mode 1 is slower, mode 2 is the same. |
| 7 | Research rows 3 and 4, build-time: `XL_BB=14`, `XL_BB=13`; `PFD` 512, 2048, 4096 | Base 209.0 / 210.0. `BB=14` 208.2 / 209.1 (ev3a 32.4, -3.1; parse 159.8, +2.1). `BB=13` 217.6 (parse 169.4). `PFD=512` 208.7 / 209.0; `PFD=2048` 209.7 / 210.5; `PFD=4096` 210.3 / 211.3 | Rejected: `BB=14` moves 3 ms from ev3a into 2 ms of parse, net under 1 ms; `PFD=512` is within noise. Neither reaches the 2 ms bar. |
| 8 | Small pages and regular stores for the record arena, so a page is filled while its zeroes are still in cache (`XL_NOHUGE=1`, `XL_NT=0`, `XL_POPW=0` in combinations) | `NOHUGE NT=0` 259; `NOHUGE NT=0 POPW=0` 269; `NT=0 POPW=0` 233; `NOHUGE POPW=0` 248; `NOHUGE` 234 | Rejected: all far slower. |
| 9 | Experiments 3 and 7 together, as defaults: dedup bucket 2^14 (`BB` 15 to 14), input chunk 64 MiB (`CH_DEFAULT` 8192 to 65536), prefetch distance 512 B (`PFD` 1024 to 512) | 21 rounds, seed 20: base 209.0 / 210.0, candidate 206.7 / 207.6 (ev3a 32.6, -2.6; parse unchanged). Parts: chunk alone 208.4 / 209.2; chunk + bucket 207.7 / 208.5; chunk + prefetch 207.6 / 208.8. 11 rounds on other seeds: seed 7 (143 event files) 212.9 / 213.6 to 209.8 / 211.0; seed 11 208.3 / 209.3 to 206.0 / 206.6; seed 3 (10M) 112.0 / 112.7 to 110.0 / 110.4. With a 32 MiB chunk the gain is 0.3 to 1 ms smaller. Gate: 36/36 fixtures and dev through Docker; output identical to the baseline's on seeds 20, 7, 11 and 3 at `NCPU` 8, 2 and 1; dev answer at `NCPU` 1 and 2; no fallback anywhere. **At 40M events (seed 5, one run each) it is slower: 422.7 against 411.5 (parse +16.5, ev3a -5.7), output identical.** | **Kept**: -2.3 to -3.1 ms (1.1 to 1.5%) at 20M, which is the size other teams' probes report for the hidden set. It would cost about 2.7% if the hidden set were twice that. |
| 10 | Fifteen other build-time constants, one at a time on top of experiment 9: `XL_UB=11/13`, `XL_SLABROWS=12/14`, `XL_XRC=2/8`, `XL_SUB_DEF=2048/8192`, `XL_SPIN_DEF=100/1500`, `XL_LSD_MIN=1024/4096`, `XL_SPLIT_PER=4/8`, `XL_CH_DEF=131072` | 7 rounds, seed 20. Base (experiment 9) 207.4 / 208.1. Best minimums: `UB=13` 206.3 / 208.0, `SPIN_DEF=100` 206.5 / 207.3, `SLABROWS=12` 206.6 / 207.5, `XRC=2` 206.6 / 207.5. The rest 207.0 to 207.6. | Rejected: every one is within 1 ms of the base and none moves the median by more than 0.8 ms. |
| 11 | Research row 5 (15.3): the scanner's tables on huge pages. A 2 MiB zero pad after the data so both 2 MiB windows holding `k_free::T` lie inside the image; at start each window is copied to an anonymous huge page and `mremap`ped over itself (RWX). Verified in `smaps`: both windows `AnonHugePages: 2048 kB`. Switch `XL_HUGIFY=0` in the same binary | 11 rounds, seed 20 (VM restarted since experiment 10, see note): base 209.2 / 210.7; remap off 209.5 / 212.7; remap on 209.6 / 212.5. Parse 158.5 on against 159.6 off (-1.1), but the total is unchanged. | Rejected: the 1 ms it takes off parse is spent making the two pages. Reverted. |
| 12 | Research row 3 (15.1), first half: event arena created right after the listing; setup workers that would spin fault in its first blocks with `MADV_POPULATE_WRITE` (up to 32 blocks, half the expected record bytes), also after their sampling work until the chunk queue is ready. Switch `XL_PREFILL=0`. Tree spawn left as it is | 15 rounds, seed 20: base 208.3 / 211.6; off 208.7 / 211.0; on 208.5 / 210.3. Parse 158.8 on, 159.0 off. | Rejected: 0.2 ms of parse, far inside the noise. Reverted. The flat-spawn half was not built: with the window this small it cannot reach the bar. |
| 13 | Experiment 12 again with a counter, to answer whether the setup window is real (15.1, and 16's click-index variant). Same change, `XL_PREFILL=N` caps the blocks, count printed under `XL_TIMING` | The window is real: idle setup workers clear 14 to 18 blocks (28 to 36 MB) of the event arena per run, with the tree spawn as it is; a cap of 128 gives the same count. 21 rounds, off / on: seed 20 207.9 / 209.5 against 208.8 / 209.8; seed 7 212.0 / 213.7 against 212.4 / 217.0. Parse 158.2 on, 158.6 off (seed 20); 159.6 against 161.4 (seed 7), total not lower in either. | Rejected for good. About 32 MB is cleared before parse and the run is not faster. The click index as the target (iteration 16) was not built: it is the same window and the same number of megabytes, only a different address, so it would recover the same thread-ms. Reverted. |

One thread per core (research 11.1), `cand` (801982c), `XL_PIN=0`, best of 7, seed 20. CPUs 0-3 are four cores, 4-7 their siblings.

| Threads, CPUs | parse | ev3a | ev3b | write | total |
|---|---|---|---|---|---|
| 8 on `0-7` (4 cores, both threads) | 159.2 | 33.2 | 1.7 | 9.8 | 206.8 |
| 4 on `0-3` (4 cores, one thread each) | 194.6 | 37.7 | 2.2 | 12.1 | 249.3 |
| 4 on `0,1,4,5` (2 cores, both threads) | 307.8 | 62.3 | 3.2 | 16.8 | 393.8 |

So the second hyperthread takes parse from 194.6 to 159.2 (18% less time, 1.22x), ev3a 1.14x, write 1.23x; and two cores to four is 1.93x in parse.

Note on the VM: it was stopped at 13:50 and restarted at 14:05 (new host). Since then the same binary reads about 1.5 ms higher at the median and the maximum of a set is 215 to 217 where it was 209 to 211. Minimums are unchanged. Anything under 2 ms cannot be told from noise on this host at 15 rounds.

Experiment 14 (research row 1, 14.1): which part of experiment 9 costs time on larger inputs. Binaries: `base` 941879e; `cand` 801982c (all three); `safe` = bucket 2^15 with the 64 MiB chunk and 512 B prefetch kept (branch `safe-constants`, commit 06ddf5389663c4b36f8dafb8e74e70600113a3ce, gate clean: 36/36, dev and seed 20 match); single parts built on `base`. Wall min / med, then parse and ev3a minimums.

| Events | base | cand (all three) | safe (chunk + prefetch) | bucket 2^14 only | chunk only | prefetch only |
|---|---|---|---|---|---|---|
| 20M seed 20, 15 rounds | 212.1 / 216.9 | 208.1 / 215.4 | 210.2 / 215.7 | | | |
| 20M seed 7, 15 rounds | 215.6 / 220.1 | 212.2 / 217.2 | 213.7 / 217.8 | | | |
| 30M seed 6, 11 rounds | 314.7 / 322.3 (parse 239.5, ev3a 54.2) | 319.3 / 322.9 (248.2, 48.7) | 313.5 / 319.0 (238.7, 53.7) | | | |
| 40M seed 5, 9 rounds | 419.1 / 426.0 (324.0, 69.7) | 434.9 / 440.7 (344.7, 63.6) | 422.1 / 426.3 (326.0, 69.9) | 436.3 / 441.9 (345.5, 63.6) | 424.3 / 426.3 | 420.8 / 425.4 |

Result: the research reading is right, it is the bucket size alone. Bucket 2^14 saves about 3.5 ms of ev3a at 20M and costs nothing in parse there; at 30M it costs parse 9 and saves ev3a 5.5 (net +4.6 at the minimum, level at the median); at 40M parse +21, ev3a -6 (net +15). The crossover is near 25M events. Chunk and prefetch are neutral to slightly good at every size. Main stays on 801982c (best at 20M, the reported size); `safe-constants` is the pick if the hidden set is thought to be 30M or more. The host is noisy today (maxima to 254), so the 20M minimums carry the comparison.

## Facts for the research session (its requests 1 to 5)

1. VM: Intel Xeon Platinum 8252C @ 3.80 GHz (Cascade Lake), 4 cores / 8 threads, 32 GB, kernel 7.0.0-1018-aws, THP `enabled=madvise`, `defrag=madvise`. Timing runs the binary directly (no Docker) with `NCPU=8 XL_TIMING=1`; output goes to ext4.
   **tmpfs:** from experiment 3 on, the input is on a tmpfs mount (`/mnt/t/big`), as on the scorer. Baseline there: wall 209.2 min / 210.1 median; parse 157.8, ev3a 35.4, write 9.5. On ext4 page cache it was 198 (parse 146).
2. `[v2b] ms: s.list=1.0 s.mmap=1.1 s.sample=0.7 maps+keys=0.2 lookups=0.0 setup=0.0 parse=159.2 st2b=0.0 st2a=0.0 st2sort=0.0 ev3a=36.0 stitch=0.1 ev3b=2.6 c.pre=0.0 write=9.8 total=210.6 peak_rss_kb=623920`
   `[v2b] prof (thread-ms): scatter=98.1 compact=21.6 runs=155.3 st2b=185.0 st2b_max_bucket=35.1 st2b_pass1=18.4 st2b_pass2=113.2 st2b_sort=53.4`
   `[v2b] rows: events=20923142 clicks=5140098 links=884845 | kept valid: simple=19274577 complex=202264 odd=77073 | runs=666655 sessions=3409434 user_rows=2247046 uf_nodes=22065 cx_users=6345 joined=599 nb=769 nub=386`
3. `--features tl`: `s.list wall 0.90 | job0: wall 2.46 busy 10.0 idle 9.68 (49%)`; `parse wall 159.22 | job1: busy 1273.0 idle 0.75 (0%)`; `ev3a wall 35.98 | job2: busy 286.8 idle 1.04`; `ev3b wall 2.56 | job3: busy 16.2 idle 4.34 (21%)`; `write wall 9.76 serial 0.15 | job4: busy 74.9 idle 1.91`; `total 210.70 ms; idle in jobs 17.7 thread-ms`; `write syscalls (thread-ms): create 0.27 write 9.18 close 0.00`; workers start 0.9 to 1.45 ms after t0; parse threads start at 2.5 ms and their first chunk runs 2.52 to about 8.1 ms.
4. `--features pmu`: this VM exposes no hardware counters (instructions and cycles read 0). What it does give: `[pmu] parse: faults=28762 task_ms=1265.9 max_thread_ms=158.8` and `thread totals: utime_ms=1002.5 stime_ms=271.7 nvcsw=22 nivcsw=61`. So about 21% of thread time is in the kernel.
5. Output is 18 MB. `[v2b] exit at` is 210.7 ms and the parent sees the process gone 1.5 to 2 ms later.

`perf record` (cpu-clock, ext4 run), share of all samples: `k_free::run_events` 31.4%, kernel `kernel_init_pages` (zeroing huge pages) 11.1% (6.2% under `MADV_POPULATE_WRITE`, 4.6% under page faults, 3.1% of that in `links::job`), `parse::Ctx::ev_batch` 10.5%, `events::stage3` closure 9.1%, `links::job` 5.0%, `k_free::cl_loop` 4.8%, `events::walk_runs` 2.6%, `events::run_clicks` 2.1%, `ck_batch` 1.9%, `Part::flush_full` 1.1%. Whole run: 1.56 CPU-s in 0.2 s wall (1.31 user, 0.25 system), 9,640 faults, peak RSS 620 MB, RSS at exit 305 MB.

4 threads take 369 ms against 198 for 8 on the same 4 cores, so the second hyperthread nearly doubles throughput: the parse threads spend much of their time waiting on memory, not executing.

Memory bandwidth of the VM (512 MB per thread, huge pages, AVX-512 loads / non-temporal stores): read 14.5 GB/s at 1 thread, 54 at 4, 57 at 8; write 7.4, 29, 30. The run reads 1.8 GB and writes about 0.56 GB twice (kernel zeroing, then the records), so roughly 70 ms of the 158 ms parse is the least the memory traffic can take; the rest is computation.
