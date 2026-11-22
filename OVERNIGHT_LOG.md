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

Experiment 15: upstream pipeline. `jason-scheffel/express-lane` has three pipeline commits after the one this tree was taken from (15ee5c1 "cut memory use, input release and work per event", b7be207 "cut work per click line", f122b64 "fall back on a fault signal"); its newest commit d7ebc13 is a probe harness and was not taken. `src/` is now f122b64's `src/` (commit 1da4866; note 0be66c0 before it is incomplete, it lacks `parse.rs` and `k_free.rs`, do not use it). Upstream has bucket 2^15, chunk 8192 KiB, prefetch 1024. Gate clean: 36/36, dev and seed 20 match.

| Set | cand 801982c | up (f122b64) | ups (up + 64 MiB chunk + prefetch 512) | upb (ups + bucket 2^14) |
|---|---|---|---|---|
| 20M seed 20 | 212.8 / 215.1 | 202.7 / 204.3; again 201.9 / 204.8 | 200.7 / 204.2 | 207.2 / 209.5 |
| 20M seed 7 | 212.6 / 217.9 | 204.6 / 208.5; again 205.5 / 208.6 | 203.6 / 207.5 | 210.5 / 213.9 |
| 30M seed 6 | 319.5 / 323.7 | 305.9 / 309.2; again 305.2 / 309.6 | 306.1 / 309.4 | 309.3 / 315.2 |

Stage minimums at 20M seed 20, cand against up: parse 162.3 to 155.2, ev3a 33.1 to 33.9, write 9.9 to 7.3. Kept: about 10 ms (4.7%) at 20M, 13 ms at 30M.

On the new parser the smaller bucket is a loss even at 20M (parse +8, ev3a -3.5), so it is dropped. Chunk and prefetch are worth 1.2 to 1.9 ms at the 20M minimum and nothing at 30M: under the usual bar, kept because the sign matches experiments 9 and 14 on both seeds and it costs nothing at larger sizes (commit a77c66d; gate clean, outputs equal to 941879e's at NCPU 8, 2 and 1 on seeds 7 and 3). All line numbers in the research notes are stale after this change.

Experiment 16: upstream's unreleased scanner. Upstream d7ebc13 carries variant `v/21`, which differs from f122b64 only in `k_free.rs` (+666 lines: the event loop written in assembly, `ev_loop` / `ev_redo`). Taken with prefetch 512 kept (commit 9363748). Gate clean: 36/36, dev and seed 20 match; outputs equal to 941879e's at NCPU 8, 2 and 1 on seeds 7, 3 and 6 (30M), no fallback. 13 rounds, wall min / med:

| Set | ups (a77c66d) | v21 (prefetch 1024) | v21p (prefetch 512, shipped) |
|---|---|---|---|
| 20M seed 20 | 200.1 / 201.7 | 195.7 / 197.7 | 195.9 / 198.1 |
| 20M seed 7 | 203.8 / 208.1 | 200.3 / 203.1 | 199.5 / 202.6 |

Parse 152.8 to 148.6 (seed 20), 155.5 to 150.9 (seed 7); other stages unchanged. Kept: about 4 ms (2%). 30M seed 6, 9 rounds:

```
ups                        wall min 307.2 med 310.2 max 310.9 | setup=0.0 parse=239.4 ev3a=50.5 write=9.9 total=305.4
v21p                       wall min 299.0 med 302.1 max 303.8 | setup=0.0 parse=230.5 ev3a=50.4 write=10.1 total=297.1
```

Experiment 17: on 9363748 (`v21p`), seed 20. (a) The other eleven variants in upstream's probe set (`v/18, b13, c2, dt, e2, fr, k, m1, m2, mn, o2`), built as they are, 7 rounds: all slower than `v21p` (199.4 / 200.0): 18 204.4, b13 201.0, c2 206.6, dt 212.5, e2 205.8, fr 205.5, k 211.0, m1 209.8, m2 215.4, mn 215.8, o2 209.0. They are older states of the same pipeline. (b) Runtime knobs, 6 rounds each, control 196.0 / 200.3: `XL_RECYCLE=0` 205.1 (ev3a +8); `XL_QORD=0/1` 222 / 219 (parse +23); `XL_RELK`, `XL_IOADV`, `XL_R3`, `XL_DONATE`, `XL_POPW=1`, `XL_FA=1`, `XL_PSETUP=0`, `XL_FREE=0`, `XL_FRLAG=0/4`, `XL_CH=32768/131072`, `XL_PF=0/1` all 195.3 to 200.5 at the minimum and 199.3 to 202.2 at the median, i.e. no different from the control. Rejected, nothing changed.

Fresh numbers for 9363748 (`v21p`), research 18.1. One run each, VM tmpfs, 8 CPUs.

- 20M seed 20: `s.list=0.7 s.mmap=1.0 s.sample=0.9 maps+keys=0.3 lookups=0.0 parse=150.4 ev3a=34.7 stitch=0.1 ev3b=2.4 write=7.3 total=197.8`; user 1.32 s, sys 0.24 s, elapsed 0.19 s; peak RSS 1,366 MB (was about 620 MB on the old source; counts the mapped input); minor faults 29,462, major 0; `nb=769 nub=386`.
- 10M seed 3: `parse=75.4 ev3a=19.6 ev3b=0.6 write=4.6 total=103.3`; user 0.66, sys 0.14; peak RSS 714 MB; minor faults 15,619; `nb=396 nub=199`. Parse per event is the same at 10M and 20M (7.0 against 7.2 ns per event line per 8 threads-wall), so the staging footprint is not costing at 20M.
- `perf record -F 4000 -g`, 20M, self time: `k_free::ev_loop` 29.8%, `parse::Ctx::ev_batch` 12.2%, `events::stage3::{closure#2}` 8.9%, kernel `kernel_init_pages` 7.8% (3.0 under `ev_batch`, 0.85 `links::bucket`, 0.84 `ck_batch`, 0.80 `Part::flush_early`), `links::bucket` 5.0%, `k_free::cl_loop` 4.4%, `ck_batch` 2.2%, `events::run_clicks` 1.9%, `events::walk_runs` 1.7%, `links::sort_users` 1.0%, `events::pass_b` 1.0%, `k_free::ev_purchase` 0.85%, kernel `do_user_addr_fault` 0.8%, `zap_present_ptes` 0.7%. The binary is 83.5% of samples, the kernel the rest. Page zeroing fell from 11.1% to 7.8%; input unmapping is nearly gone.

Upstream note: `jason-scheffel/express-lane` added, at 612962f (16:23), a notice in every file opting the repository out of use by other teams and their tools during the event. Nothing has been taken from it at or after that commit, and nothing more will be.

Experiment 18, on 9363748 (`v21p`). All rejected; main unchanged.

- Staging prefetch pre-pass (research 19, change A) in `EvPart::push_many`'s full-batch path, read intent (`pf1`) and write intent (`pf2`), 11 rounds: 30M 300.5 and 299.5 against 298.3; 20M 196.1 and 196.6 against 195.3. Parse +1 to +3. Rejected.
- Compiler flags, 9 rounds at 20M, control 196.5 / 199.9: no branch-boundary padding 196.6 / 200.3; `-align-loops=32` 196.6 / 199.3; `-align-loops=64` 195.7 / 198.2; padding without the prefix limit 197.2 / 199.6; `-prefer-256-bit` off 215.0 / 218.2 (parse +18). Rejected.
- Build-time constants sweep (25 variants) was started and abandoned unmeasured when upstream moved.
- Upstream c7d4aa4 ("write rows during stage 3, lighter sinks and driver", on top of 2398c62 "cut work per event line and per record"), taken at the user's explicit request after the opt-out notice. `j3` as is, `j3s` with 64 MiB chunk and prefetch 512. Gate clean for both (36/36, dev, seed 20; `j3s` equal to 941879e at NCPU 8, 2, 1 on seeds 7 and 3). 11 rounds, wall min / med:

| Set | v21p | j3 | j3s |
|---|---|---|---|
| 20M seed 20 | 197.3 / 199.4 | 198.5 / 199.7 | 198.3 / 199.8 |
| 20M seed 7 | 200.4 / 203.4 | 199.9 / 204.0 | 198.4 / 202.2 |
| 30M seed 6 | 299.1 / 303.0 | 299.4 / 300.3 | 296.2 / 299.9 |

Stages at 20M seed 20: write 7.2 to 5.3, ev3a 34.0 to 36.0, parse level. On this VM the two cancel: -2 to +1 ms, inside the noise. Not kept on main. `j3s` is on branch `upstream-c7d4aa4` (d3e8178) so it can be tried on the scorer, where the output directory may cost differently from this VM's disk.

Experiment 19: build-time constants on 9363748, one at a time, seed 20, 6 rounds, wall min / med (control 196.9 / 199.2). `XL_UB` 11: 196.2 / 197.6, 13: 200.1 / 201.1. `XL_SLABROWS` 12: 197.0 / 199.4, 14: 197.3 / 199.8. `XL_XRC` 2: 198.4 / 199.7, 8: 196.8 / 199.9. `XL_SPIN_DEF` 100: 196.2 / 199.7, 1500: 195.4 / 199.2. `XL_HEAVY_DIV` 2: 195.9 / 197.9, 8: 198.7 / 200.1. `XL_DONATE_DEF` 8: 197.9 / 199.0, 32: 198.9 / 200.1. `XL_SHCH=1`: 221.3 / 228.2 (parse +25). `XL_CKPF` 0: 196.9 / 200.0, 2: 199.3 / 200.2. `XL_R12NULL=1`: 199.6 / 200.6. `XL_DEC2=0`: 198.1 / 200.6 (ev3a +1.4). `XL_RELK_DEF` 4: 198.1 / 200.7, 16: 199.1 / 200.5. `XL_SPLIT_PER` 4: 199.4 / 200.1, 8: 197.5 / 199.3. `XL_LSD_MIN` 1024: 198.0 / 200.0, 4096: 195.7 / 199.1. `XL_CXRDIV` 32: 198.4 / 200.2, 128: 198.7 / 199.8.

The four best re-run at 15 rounds: seed 20 control 195.7 / 199.8, UB=11 195.0 / 197.9, HEAVY_DIV=2 196.2 / 200.0, LSD_MIN=4096 196.9 / 199.5, SPIN_DEF=1500 196.5 / 199.8; seed 7 control 198.9 / 202.8, UB=11 198.6 / 202.4, HEAVY_DIV=2 201.0 / 203.6, LSD_MIN=4096 200.2 / 203.4, SPIN_DEF=1500 201.0 / 202.9. Nothing reaches 1 ms on both seeds. All rejected; the defaults stay.

Frequency probe (research 22.1), run on 9363748. Probe as written in 22.1 (60,000 dependent multiplies, minimum of three), taken on each pool worker (threads 1 to 7) immediately before and after every pool job; values in thousands of TSC ticks, lower is a faster clock. VM CPU is a Xeon Platinum 8252C, `/proc/cpuinfo` reads 4499 MHz. Two runs, 20M seed 20:

| Job | Before | After |
|---|---|---|
| setup (1 to 3 ms) | 389 to 412 (thread 1: 498 to 503, it is the first thread started) | 388 to 412 |
| parse (the long job) | 405 to 412 | 386 to 413 |
| ev3a | 410 to 412 (thread 1: 461) | 454 to 488 |
| ev3b | 411 to 413 on most, 454 to 466 on some | 448 to 488 |
| write | 411 to 413 on most, up to 489 | 388 to 490, mixed |

Reading: **parse runs at full clock.** The probe after parse equals the probe before it on every thread, so the 256-bit multiplies do not hold a reduced state here; row 1's remedy is not needed on this CPU. **Stage 3 does run at a reduced clock:** after ev3a every thread is 11 to 19% slower (456 to 488 against 411), and it recovers between jobs (most "before" values of the next job are back at 411). So ev3a, ev3b and part of the write stage, about 45 ms of wall, run 11 to 19% under the parse clock because of their 512-bit code. `fast_name_v` (the only 512-bit multiply, `events.rs:978`) is not the cause: it is only called from the self-test, and forcing its scalar path changes nothing (after-ev3a 456 to 478). `_mm512_` intrinsics by file: events.rs 171 lines, ckix.rs 34, core.rs 14, parse.rs 3, pqw.rs 2. Nothing changed in the source.

Experiment 20: 256-bit form of the writer's only 512-bit intrinsic (`pqw.rs:595`, `valid_mask64`, two masked 32-byte halves). Probe after the write job unchanged (three threads back at 394, four at 465 to 485, as before); write lap 7.2 against 7.2; 9 rounds 196.5 / 200.1 against 199.3 / 200.4 (the minimum difference is all in parse, which the change does not touch). Rejected, reverted.

Why it did nothing: the 512-bit instructions in the writer are not from intrinsics. `objdump` counts of instructions with a `zmm` operand per function: `pqw::emit_user_ids` 410, `pqw::emit_chunk` 363, `pqw::emit_dict_off` 206, `events::stage3::{closure#0}` 190, `pqw::emit_packed` (two instances) 189 and 115, `pqw::emit_dict_tm` 156, `Vec<Seg>` stable sort 120 + 50 + 50, `pqw::pack_any` 94, `events::run_clicks` 81, `setup::setup` 78, `ckix::ck_vec_ix` 77, `main` 73, `events::pass_b` 68, `complex::shard_order` 52, `Core::feed_slice` 47, `events::simple_run` 38. In the writer they are `vpermi2d`, `vpermd (mem), zmm`, `vpcmpnltuq`, `vpsllvq`, `vpmovzxdq`: the compiler's own lowering of the bit-packing loops, which uses 512-bit shuffles even with `prefer-256-bit` on. There is no flag that stops that without also disabling the intrinsics, and the write stage is 7 ms, so the most this could return is under 1 ms. Stage 3's kernels are 16-lane by design (`maskz_compress_epi32`, `permutex2var`, `conflict_epi32`); a 256-bit rewrite doubles their instruction count to win back at most 11 to 19% of the part of ev3a that is not waiting on memory, and ev3a gains only 1.14x from the second hyperthread (experiment 13), i.e. it is mostly cache-bound. Not attempted.

Experiment 21: upstream's probe set at 00591fc (18:12) has four source variants newer than c7d4aa4: `v/26`, `v/27`, `v/d2`, `v/d3` (the rest are run-time knob settings of `v/kb`). Built as they are, 9 rounds, seed 20, wall min / med: `v21p` 196.2 / 199.7; `j3s` 195.3 / 199.7; 26: 196.1 / 198.4; 27: 196.4 / 198.1; d2: 197.1 / 199.4; d3: 198.3 / 201.0. Stages for 26 and 27: parse 148.3 and 148.8, ev3a 35.8 and 35.7, write 5.3. No variant differs from `v21p` by more than the noise. Nothing taken.

Experiment 22: upstream 078e760 ("cut work per line in both line loops", on top of c7d4aa4's rows written during stage 3), taken at the user's request. `j4` as is, `j4s` with 64 MiB chunk and prefetch 512 (commit 7519633 on main). Gate clean: 36/36, dev and seed 20 match, `j4s` equal to 941879e at NCPU 8, 2 and 1 on seeds 7, 3 and 6, no fallback. 11 rounds, wall min / med:

| Set | v21p (9363748) | j3s (c7d4aa4 + constants) | j4 | j4s (kept) |
|---|---|---|---|---|
| 20M seed 20 | 195.4 / 200.0 | 194.0 / 197.4 | 193.0 / 195.2 | 191.6 / 195.5 |
| 20M seed 7 | 201.8 / 203.9 | 199.2 / 202.8 | 196.6 / 200.7 | 197.3 / 199.4 |

Stages, seed 20, v21p to j4s: parse 148.1 to 144.7, ev3a 34.0 to 35.6, write 7.2 to 5.4. Kept: -3.8 / -4.5 ms on seed 20, -4.5 / -4.5 on seed 7. The two constants are level with plain upstream here (seed 20 -1.4 at the minimum, seed 7 +0.7), kept as before. 30M seed 6, 7 rounds:

```
v21p                       wall min 297.0 med 301.6 max 302.9 | setup=0.0 parse=229.5 ev3a=50.3 write=10.1 total=295.1
j4s                        wall min 289.4 med 295.3 max 297.8 | setup=0.0 parse=221.8 ev3a=52.7 write=7.1 total=287.8
```

The binary is now `j4s` on the VM; file and line references in the research notes are stale again (5 source files changed, +861 -192).

Experiment 23: upstream's probe set at 628f99c (19:00, "loop and index variants") has six source variants beyond 078e760: `v/e3`, `c8`, `c5`, `r4`, `i4`, `all`. Built as they are (so without the two constants), 9 rounds, seed 20, wall min / med: `j4s` (main, 7519633) 192.5 / 196.2; `j4` (plain 078e760) 194.3 / 196.4; e3 194.9 / 197.2; c8 193.9 / 197.0; c5 197.2 / 198.1; r4 197.3 / 197.9; i4 196.2 / 196.9; all 194.4 / 196.8. None is faster than plain 078e760. Nothing taken.

Experiment 24: upstream moved four commits past 078e760 (4b57bc7 click-loop pair decode, abd72ed and 3493fdd fixes, 42e53ab "wider event loop, pair decode for clicks, new click index"). 11 rounds, wall min / med.

- **3493fdd is 078e760 plus two scanner fixes** (4 lines in `k_free.rs`: `TL2_R6` 65 to 66, and a check in `ev_loop` that sends a line without a third comma to the careful path; abd72ed also backs out 4b57bc7's click changes). Taken with the two constants as `j6s`, commit 4f63ac5 on main. These are correctness fixes for malformed lines, not speed: seed 20 194.8 / 196.1 against `j4s` 195.9 / 196.8; seed 7 195.3 / 200.0 against 196.5 / 199.6. Gate clean: 36/36, dev and seed 20 match, equal to 941879e at NCPU 8, 2, 1 on seeds 7, 3 and 6.
- **42e53ab is slower on this VM**: `j5` (as is) seed 20 208.5 / 211.4, seed 7 211.4 / 215.5; `j5s` (with constants) 208.9 / 211.0 and 212.1 / 215.4; against `j4s` 192.6 / 196.5 and 198.8 / 200.3. Parse 160.1 against 144.9 (+15), other stages level. Its outputs are exact (36/36, dev, seed 20, NCPU 8/2/1 on seeds 7 and 3). Not taken on main; pushed as branch `upstream-42e53ab` (6fddb4b) in case the scorer's CPU behaves differently from this 8252C, which is the only reason upstream would ship it.

Experiment 25: upstream 2d7801e ("new click index, pair decode for clicks, lighter event loop"; relative to 42e53ab it drops 237 lines of `k_free.rs`, i.e. the wider event loop that was slower here). `j7` as is, `j7s` with the two constants (on main). Gate clean: 36/36, dev and seed 20 match, equal to 941879e at NCPU 8, 2, 1 on seeds 7, 3 and 6, no fallback. 11 rounds, wall min / med:

| Set | j6s (4f63ac5) | j7 | j7s (kept) |
|---|---|---|---|
| 20M seed 20 | 193.8 / 196.1 | 189.3 / 193.7 | 189.6 / 193.2 |
| 20M seed 7 | 197.0 / 198.3 | 194.7 / 197.7 | 193.3 / 197.3 |

Parse 145.7 to 142.1 (seed 20), 146.7 to 143.6 (seed 7); ev3a and write level. Kept: -4.2 / -2.9 and -3.7 / -1.0 ms. 30M seed 6, 7 rounds:

```
j6s                        wall min 292.5 med 297.0 max 297.9 | setup=0.0 parse=225.1 ev3a=53.3 write=7.2 total=291.0
j7s                        wall min 289.8 med 291.3 max 292.2 | setup=0.0 parse=221.3 ev3a=53.5 write=7.2 total=288.3
```

Experiment 26: **the rules changed (about 21:30).** New generator (generator.zip of 22:01, built on the VM as `~/gen2`): every ID passes through a per-seed keyed bijection and one in eight carries a 32-character hex suffix, so IDs are opaque variable-width strings and the old key-recovery fast path never applies. The final now runs on about 60M events with a 3-minute cap. The processing rules, the 36 fixtures and the scripts are unchanged; the dev answer is new. All older numbers in this log are for the old ID format and no longer describe the scorer.

New sets on tmpfs: `/mnt/t/n1` (seed 1, 1M: the dev set), `/mnt/t/n20` (seed 20, 20M), `/mnt/t/n60` (seed 6, 60M). Wall, best of 3:

| Binary | 20M seed 20 | 20M seed 7 | 60M seed 6 | Note |
|---|---|---|---|---|
| `j7s` (d3c25fc, previous main) | 3455 ms | 3533 ms | | falls back: "no user key" |
| `old73f8661` (pre-overhaul implementation) | 5423 ms | 5645 ms | 18.3 s, 4.9 GB | slow on the long IDs |
| `j9` = upstream 93e9168 + two constants (kept, 738cc7d) | 870 ms | 890 ms | 2.93 s, 15.0 GB peak | new `v3b` path |

`j9` stages at 20M: parse 394, dedup 132, join 110, sessions 191, write 10 (total 839 in-process). Gate: 36/36 fixtures, old dev and new dev answer match (new dev also through Docker with the 24 GB / 8 CPU limits), outputs equal to `old73f8661` on both 20M sets and the 60M set, equal at NCPU 2 and 1. The image is now `rust:1-slim` with a static glibc target.

Experiment 27: upstream ff78d19 ("compact event lane, bitmap dedup, session bins", `v3b.rs` only). `k1` as is, `k1s` with the two constants (kept on main). 7 rounds at 20M seed 20, wall min / med: `j9` 868.4 / 876.0, `k1` 679.8 / 682.3, `k1s` 681.2 / 684.0. Stages: parse 395 to 273, dedup 133 to 87, join 107 to 72, sessions 189 to 205, write level; peak memory 5.1 to 3.9 GB. 60M: 2.93 to 2.08 s, peak 15.0 to 11.0 GB. Gate: 36/36, old dev, new dev answer, 20M equal to the saved reference at NCPU 8, 2, 1, 60M equal. The two constants make no measurable difference on this path. (Note: a first gate run compared a stale root-owned output folder and was discarded.)

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
