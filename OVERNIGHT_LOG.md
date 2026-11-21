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
