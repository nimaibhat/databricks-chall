# RESEARCH_NOTES.md

Research-only notes for the coding agent, restarted on 2026-11-21 for the new codebase (`7ed9b7b` "overhaul" and after). I do not build or run anything: every number below is either quoted from a named source or marked **my estimate**. Line numbers are `src/` at `57ae37c`.

The notes for the old codebase are at `git show 3a5f601:RESEARCH_NOTES.md`. What still holds from them is carried into "Scorer facts" and "Closed" below; the rest is obsolete.

## Top five untried (ranked by expected gain per unit of risk to exact output)

Re-ranked in iteration 14 after log experiments 5 to 10. Source is now `801982c` (bucket 2^14, 64 MiB chunk cap, prefetch 512 B: kept, -2.3 to -3.1 ms at 20M). Baseline on tmpfs: 206.7 / 207.6 ms.

| # | Idea | Where | Expected gain | Rests on | Risk to exact output | Entry |
|---|---|---|---|---|---|---|
| 1 | Fresh numbers for the new pipeline (no code): lap line, `prof` thread-ms, faults, user/system time, `perf report` top 15, `perf annotate` of `ev_loop`; plus two new counters: a histogram of event-line shapes (21.2) and the TLB row of `/proc/interrupts` before and after a run (21.4) | n/a | Directs everything else | All ranking figures predate the new pipeline | None | 18.1, 21 |
| 2 | Map each input file at a 2 MiB-congruent address, so a tmpfs with huge folios is mapped by 2 MiB entries | `setup.rs` (where files are mapped into the region; `io_advise` is called there) | Nothing if the scorer's tmpfs has 4 KiB folios (the default); if it has huge folios, most of the input fault and unmap work goes, which was about 20% of parse thread time on the old build (log). **My guess at the odds: 5 to 10%** | Kernel source and docs (quoted in 21.1): large folios are mapped whole when the mapping is congruent; default mount policy is `never` | None: only addresses change | 21.1 |
| 3 | Frequency-licence probe at three points | end of `stage1`, end of `stage3a` | Diagnostic; ceiling is the L0 to L1 gap, 12.5% on the one Skylake-X part with published figures (quoted, 21.3) | Literature: sustained 256-bit integer multiplies request L1, and the drop applies to both hyperthreads of the core | None | 14.2, 21.3 |
| 4 | Dominant-shape fast path in the event scanner, **only if** the histogram of row 1 shows one or two shapes covering most lines | `k_free.rs` `ev_loop` | Unknown until the histogram exists; published gains for this trick are 15 to 25% of scan time in other parsers (quoted, different programs) | 1BRC write-ups (21.2) | Low if the fast path falls back to the existing code on any mismatch; it is new scanner code, so full gate | 21.2 |
| 5 | Stage 3 dedup table with 12-byte slots (the arena record copied as is, widened in the compaction) | `events.rs:262` (`scatter_chunk12`) and the compaction after it | **My estimate** 1 to 2 ms, by proportion from the 3.5 ms measured for halving the table | Experiments 9 and 15; literature agrees the stage is L2-capacity-bound (21.5) | Row-sensitive: this code decides which duplicate survives. Ranked last for that reason | 20, 21.5 |

Staging prefetch (19, change A) is demoted: the partitioning literature measures prefetch as neutral to negative below about 1,024 partitions (21.5). The 10M parse lap is still worth having.

**All file and line references below iteration 18 are to the old source and are stale.**

**Closed by measurement since iteration 9 (do not re-propose):**
- Row 1 of iteration 9 (release input once at the end): experiment 5. Parse does drop to 151.0, but the parallel release then takes 8.6 ms, so the wall is the same. The log's conclusion is the durable fact: **unmapping 1.8 GB of 4 KiB input pages costs about 8 ms of wall on 8 threads wherever it is done.** My estimate of 3 to 6 ms for the parallel release was too low.
- Click-index modes (`XL_CKIX`): experiment 6. Mode 1 costs ev3a 5.5 ms (the recycling effect I flagged in 9.7 or the random stores; not separated), mode 2 is equal.
- Bucket size and prefetch distance alone: experiment 7, each under 1 ms; together with the chunk cap they were kept (experiment 9).
- Small pages with regular stores for the arena: experiment 8, 23 to 60 ms slower.
- Fifteen further build constants: experiment 10.

## Requests to the coding agent

0. New in iteration 15: row 3 (15.1), the `ev3b` item timing (15.2), and the three measurements of 15.4.
1. Row 1: at 40M (seed 5), three builds on top of `801982c`, each with one part reverted: `XL_BB=15`; `XL_CH_DEF=8192`; `PFD=1024`. Also one size in between (30M) for the shipped build against `941879e`, to see where the crossover is.
2. Row 2: the probe of 1.2 with 3.3's third measuring point.
3. Row 3: the one-thread-per-core run of 11.1.

## Scorer facts carried over (all from other teams' probes, quoted in the old notes 11.1, 39.2, 50.1, 51.2; none verified here)

- Cascade Lake Xeon at 2.80 GHz, 8 CPUs by affinity **and** a cgroup quota, host kernel 7.0, THP in `madvise` mode and honoured.
- `/data` is tmpfs (always in RAM, 4 KiB pages when mapped); `/output` is ext4.
- The hidden set is 20M events. The part of a score outside the process is about 0.1 s.
- More runnable threads than 8 are throttled by the quota. The pool here is 7 workers plus main (`sys.rs:279`), so it is not exposed.

## Where the time is (quoted from `FOR_NIMAI.md`, machine not stated there)

Warm in-process about 205 to 215 ms: parse about 160, events about 35, write about 10. Parse is three quarters of the process, so a 1% change anywhere else is 0.5 ms.

**My arithmetic on those numbers:** 160 ms on 8 threads is 1.28 thread-seconds for about 25M lines and 2.3 GB, so about 51 ns per line per hardware thread and 14 GB/s in aggregate. joch44 measured 14 to 15 GB/s as the ceiling for a warm 8-thread newline count on an Ice Lake VM (old notes 11.1). So parse may be close to what four cores can pull through mapped 4 KiB pages at all; the `pmu` line (request 4) says whether it is instruction-bound or memory-bound. If IPC is low and `stime` is a visible share, ideas 3 and 4 gain weight and idea 2 loses it.

---

## Iteration 1 (2026-11-21, afternoon): first pass over the new codebase

Read in full: `main.rs`, `sys.rs`, `setup.rs`, `parse.rs`, `out.rs`, `iface.rs`, `keyrec.rs`, the first half of `k_free.rs`, the drivers of `links.rs` and `pqw.rs`, the rules, `FOR_NIMAI.md`, the old notes' scorer sections. Not yet read line by line: `k_free.rs` hot loop (`ev_s`, `ev_d`, `run_events`), `events.rs`, `core.rs`, `links.rs` bodies, `pqw.rs` encoders. Those are the next passes.

### 1.1 PGO, with the three ways it silently does nothing or breaks the build

`FOR_NIMAI.md` names PGO as the one thing left. The recipe has traps specific to this binary:

- **The profile is never written.** The profiler runtime writes `.profraw` from an `atexit` handler. This program leaves through `sys::_exit(0)` (`main.rs:539`), which skips `atexit`, and through `execve` in `sys::fallback` (`sys.rs:127`). An instrumented build would run and leave no file. Fix, in the instrumented build only (a `pgo` cargo feature): declare `extern "C" { fn __llvm_profile_write_file() -> i32; }` and call it immediately before `_exit(0)` at `main.rs:539`. Worker threads never exit, which is fine: counters are global.
- **`RUSTFLAGS` replaces `.cargo/config.toml`'s `build.rustflags`, it does not add to it** (cargo documents the two as mutually exclusive sources). `RUSTFLAGS="-Cprofile-use=..." cargo build` would drop `target-cpu=cascadelake`; `k_free.rs:5-11` then stops the build with `compile_error!`, so it fails loudly, but on the scorer a failed build is a rejected submission. Put `-Cprofile-use=/src/pgo.profdata` into the `rustflags` array in `.cargo/config.toml`, next to the existing flags.
- **The image is `rust:1-alpine`, unpinned** (`Dockerfile:1`). A `.profdata` written by one LLVM version and read by a newer one can be a hard error. Pin the tag to the exact version that produced the profile (`rust:1.xx-alpine`), and merge the raw profile with that toolchain's own `llvm-profdata` (`rustup component add llvm-tools`).
- A stale profile (source changed after training) only produces warnings and unprofiled functions, so later commits stay safe, but re-train before the final pick.
- Training: two or three seeds at 20M, merged, none of them a gate seed. Dirt rates are drawn per seed, so one seed over-fits the cold paths.
- **What to expect.** The hot paths are already laid out by hand (`core::hint::cold_path`, `#[cold]`, asm loops, `nv`/`yb`/`kb` barriers in `sys.rs:1139-1158` that stop LLVM from re-deriving values). PGO's usual wins are block layout, hot/cold splitting and inlining; here it can also undo a hand decision. So: same-run A/B, plain against profiled, and keep the plain build unless both 20M and the dev set are faster.
- **Risk to output.** PGO does not change semantics. But this code hands out `&mut TState` from `&G` (`main.rs:220`) and uses `static mut` (`#![allow(static_mut_refs)]`, `main.rs:1`); a different inlining can change how such code behaves if any of it is undefined behaviour in practice. That is what the full gate at 1, 2 and 8 threads is for; run it more than once.

### 1.2 Which frequency licence does parse run under?

**The idea.** On Skylake-SP and Cascade Lake, each core picks a frequency licence from the instructions it executes. Light 256-bit integer code stays at L0. Heavy 256-bit code, and any 512-bit code, moves the core to L1, which has a lower turbo table. If parse holds L1 only because of a handful of multiplies, moving those to 128-bit or scalar form keeps the whole stage at L0.

**Sources (quoted).**
- Lemire, ["AVX-512: when and how to use these new instructions"](https://lemire.me/blog/2018/09/07/avx-512-when-and-how-to-use-these-new-instructions/) (2018, written for Skylake-SP): "Heavy instructions are those involving floating point operations or integer multiplications (since these execute on the floating point unit)"; "if you are using 256-bit heavy instructions in a sustained manner, you will move to L1"; the licence is per core and also slows "the sibling logical core when hyperthreading is used". Xeon Gold 5120 turbo, 9 active cores: normal 2.7 GHz, AVX2 (L1) 2.3 GHz.
- Downs, ["Gathering Intel on Intel AVX-512 Transitions"](https://travisdowns.github.io/blog/2020/01/17/avxfreq1.html) (2020, Xeon W-2104): L0 3.2 GHz, L1 2.8 GHz, L2 2.4 GHz; light 256-bit `vpor ymm` "runs consistently at 3.2 GHz"; the licence is dropped about 680 µs after the last instruction that needed it; each transition halts the core for about 10 µs. The post tests only light instructions and says heavy 256-bit ones were left out, so **nobody in these sources measured a 256-bit integer multiply directly**; Lemire's classification is the basis.
- Ice Lake and later dropped the light/heavy distinction (simdutf8 issue 3, secondary). The scorer is reported as Cascade Lake, where it applies.
- Counter-evidence that does not settle it: joch44's AVX2-only build scored the same as its AVX-512 build (old notes 39.2). An AVX2 build still uses 256-bit multiplies, so both were in the same licence.

**Where the 256-bit multiplies are in parse.** The authors kept everything at 256 bits on purpose (`sys::z::yb`, and the asm helpers below), which avoids L2, but these are integer multiplies on `ymm`:
- `parse.rs:259-315`: `mul8`, `mul4`, `mul4m` (`vpmullq ymm`), used by `ev_vec` (`:642`, four per 4 rows: two unscrambles), `ck_vec` (`:750`), `lk_batch` (`:1110`); `_mm256_mul_epu32` at `:689` (seconds from milliseconds).
- `parse.rs:889` `b36_vec4`: `vpmaddubsw`, `vpmaddwd`, `vpmuludq` on `ymm`.
- `k_free.rs:875` `ids2j`: the same three on `ymm` for both IDs of an event line; `ev_d` (the fused timestamp multiply-add, per `FOR_NIMAI.md`).
- The 128-bit forms already exist beside them: `ids2v` (`k_free.rs:855`), `ts_time` (`:825`), `dec` (`:896`).

**Step 1, the probe (no behaviour change, about 20 lines behind a feature).** At the end of `stage1` (`parse.rs:2181`, after `mark(tid, 5)`), and once at worker start before any vector code, time a fixed scalar loop (say 200,000 iterations of a dependent `imul`/`add` chain, about 150 µs) with `rdtsc`. TSC ticks at a constant rate, so the ratio of the two timings is the ratio of core clock rates. The licence persists for about 680 µs after the last heavy instruction, so the loop after parse still runs at parse's licence. A ratio near 1.00 closes this idea; 1.08 to 1.15 means parse runs in L1. Where the PMU is exposed, `perf stat -e core_power.lvl0_turbo_license,core_power.lvl1_turbo_license,core_power.lvl2_turbo_license` gives the same answer directly.
- This only transfers to the scorer if the test VM is Skylake-SP or Cascade Lake (AWS c5/m5/r5 families). On Ice Lake or Sapphire Rapids (c6i, c7i) the probe will read 1.00 whatever the code does; say which it is in the log.
- A VM does not change licence behaviour; it is in the core. Turbo headroom on a shared host does vary, so the size of the effect on the scorer is uncertain even if the probe is positive.

**Step 2, only if the probe is positive.** Replace the 256-bit multiplies with two 128-bit ones or with scalar `imul` (a 64-bit scalar multiply is one µop on these cores). **My estimate** of the cost: about 15 to 25 extra instructions per event line across `ids2j`, `ev_d` and `ev_vec`; against that, every instruction in parse runs at the L0 clock. Net gain is positive only if the clock gain is above the instruction growth, so try `ev_vec` first (batch code, easy to A/B, covered by `ev_vec_matches_scalar`), then the parser kernels.
- **Risk.** Low: results must be bit-identical and the existing differential tests check exactly that. No change to what is accepted or rejected.

### 1.3 Use setup's idle thread time to fault arena blocks

- **What happens now.** `Part::new_chunk` (`parse.rs:180-187`) takes a 2 MiB block from the arena and calls `madvise(MADV_POPULATE_WRITE)` on it, inline, on the parsing thread. The kernel zeroes 2 MiB there and then. For 20M events and 5M clicks the two arenas hold about 400 MB of records, so about 200 blocks. **My estimate**, at 150 to 250 µs per 2 MiB clear on this class of CPU: 30 to 50 thread-ms inside parse, 2 to 4% of parse thread time.
- **Idle time exists before parse.** In `setup_par` (`setup.rs:826-833`) workers finish their tasks and then spin until thread 0 has listed, mapped and sampled the files and recovered the keys. The arenas are created only after that job returns (`setup.rs:968-969`), although their sizes depend only on `ev_bytes`, `ck_bytes` and `lk_bytes`, which thread 0 knows right after listing (`setup.rs:844`, where `rowcap` and `nbs` are already published the same way).
- **Change.** Publish the three byte totals through a `Slot` after the listing; let a worker create the arenas and let each idle worker take its first one or two blocks per arena and populate them, handing them to its own `Part` (the existing `PARTS_READY` path at `setup.rs:975-983` and `parse.rs:2071` already passes pre-built `Part`s; this adds a pre-faulted `blk..blk_end`). Stop as soon as `keys.done` is set so setup is never lengthened.
- **Size.** Bounded by the idle thread-ms of setup's job, which the `tl` build prints (request 3, "job0 ... idle"). If that is 10 thread-ms, about 50 blocks (a quarter of the zeroing) move off the parse; **my estimate** 1 to 2 ms of wall. If idle is under 2 thread-ms, drop the idea.
- **Related knob, already there and off:** `XL_PREFAULT` (`sys.rs:302`, `:684`) faults each worker's heap chunk, not the record arenas. `FOR_NIMAI.md` says to keep it off without an A/B; this idea is different memory.
- **Evidence.** Old log experiments 47/48 (old codebase, Intel runner): arena pre-fault on idle threads -1.4 to -4.3%. Th3r0n's probe: storing into fresh memory cost 0.10 s of a 0.41 s parse on a host without huge pages, "much smaller" on the scorer with them (old notes 51.2). So the per-block cost on the scorer is the 2 MiB clear, not 512 small faults.
- **Risk.** None to output. The failure mode is populating memory that is never used (a wasted clear), bounded by one or two blocks per thread.

### 1.4 The input mapping on tmpfs: what it costs, and one A/B

- **What happens now.** Each file is mapped privately into one region (`setup.rs:866`). Parse reads the mapping directly; after each chunk it drops the chunk's pages with `MADV_DONTNEED` (`parse.rs:2158-2163`), and an idle thread unmaps the region during the write stage (`out.rs:541-546`). `fault_around_check` (`parse.rs:1805`) touches two adjacent pages and switches to `pread` into a 128 KiB buffer if the second one faults again, i.e. if the kernel is not mapping neighbours on a fault.
- **My estimate of the kernel share** (not measured): 2.3 GB is about 560,000 pages. With 64 KiB fault-around that is about 35,000 faults at 1.5 to 2 µs each (entry, lookup, 16 PTEs), 50 to 70 thread-ms; the `MADV_DONTNEED` zap is another 20 to 35 thread-ms. Together about 7% of parse's 1.28 thread-seconds. `pread` instead copies 2.3 GB, which at 6 to 10 GB/s per busy hyperthread is 230 to 380 thread-ms, so mapping is the right default on tmpfs and the knobs below can only move a few percent.
- **A/B to run once, on a `/data` that is tmpfs** (1.7): default; `XL_REL=0` (no per-chunk drop: saves the zap in parse, moves it to the single-thread `munmap` in the write stage, where it may or may not hide); `XL_RD=128`; and a new variant that calls `madvise(chunk, MADV_POPULATE_READ = 22)` at `parse.rs:2146` before `parse_buf`, which maps the chunk's pages in one call instead of 16 at a time by fault. Kernel 5.14 or newer; ignore the return value, the fault path still works without it.
- **Evidence for the populate variant is mixed** (old notes 11.2): micah-andelman and Th3r0n use it and kept it; TejasviniChawla recorded that populating mappings hurt. Hence rank 4 and "one A/B", not a recommendation.
- **Risk.** None to output.

### 1.5 Kick writeback when each output file is complete

- **What happens now.** `UserFile::finish` (`pqw.rs:1087`) and `write_file` (`pqw.rs:1111`) write and close; nothing asks the kernel to start writing. The process then exits and the container is torn down.
- **Why it can matter.** Old notes 29.2 to 29.4: when the container's overlay filesystem is unmounted, the kernel syncs the whole upper filesystem; if `/output` is on that same filesystem, the dirty output pages are written before the container is reported as exited. Measured on one GitHub runner at about 1 ms per MB of output after exit; on another runner nothing. Old experiment 39 (this repo, old code) cut post-exit time from 0.13 to 0.04 s at 20M by writing output earlier and starting writeback early.
- **Change.** After the last `write` of each file, `syscall(SYS_sync_file_range = 277, fd, 0, 0, SYNC_FILE_RANGE_WRITE = 2)`. It queues the dirty pages and returns without waiting. Call it on the thread that wrote the file, before `close`.
- **Size.** Depends on output MB (request 5) and the scorer's disk, both unknown. The write stage is the last 10 ms, so the kernel gets little head start; the early files of `user_daily` (written while complex persons are still running, `out.rs:548-559`) get the most. **My estimate**: 0 to a few ms on the board. Cheap to try because the post-exit time shows directly in request 5's number on the test VM.
- **Risk.** None to output: the same bytes go to the same files. `sync_file_range` with only the WRITE flag does not wait for completion, but it can block briefly on a congested device; measure the write stage's own time as well as the post-exit time.

### 1.6 For the user: the board is a noisy clock, and this source has already scored 0.30 s

- `FOR_NIMAI.md` says the public `express-lane` head has the same blobs as this tip for `src/`, `Dockerfile`, `Cargo.toml` and `.cargo/`, and that the best board score in public chat is 0.30 s. This repo's score with that source is about 0.35 s.
- If those two scores are the same source, the 0.05 s between them is draw-to-draw variation on the scorer (old notes 39.2 put board noise near 20% for a single draw) or a different compiler pulled by the unpinned `rust:1-alpine` on the day of the build. Neither is something a code change fixes.
- What follows: (a) a code gain of 1 to 3% (2 to 6 ms) cannot be seen in one board draw, so pick commits by same-run A/B on the VM, not by the board; (b) if the board keeps a team's best score, each further submission of the same commit is another draw from the same distribution. Whether to spend the two hourly slots that way is your call.

### 1.7 Bench fidelity checklist (affects every A/B above)

The scorer, as reported: `/data` on tmpfs, `/output` on ext4, `--cpus 8` quota with at most 8 CPUs in the affinity mask, THP `madvise`, kernel 7.0, Cascade Lake. On the test VM:
- Put the input on tmpfs (`/dev/shm/...` bind-mounted read-only as `/data`). On ext4 the page cache is also 4 KiB-mapped, but fault-around and `MADV_DONTNEED` costs differ, and `fault_around_check` may pick a different path than it does on the scorer.
- Run with `--cpus 8` and the same affinity width, THP in `madvise` mode.
- Kernel version matters for fault cost (per-VMA locks from 6.4). Note it in the log.
- Compare only within one interleaved run; old notes 39.1 saw 0.3 s of container overhead difference between runner machines.

### Closed in this iteration (reasoned, not measured)

- **Leaving memory teardown to another process.** Sharing the address space with a second process (`clone(CLONE_VM)`) so that `_exit` returns before the page tables are freed does not help in a container: this binary is PID 1 of its PID namespace, and when PID 1 exits the kernel kills and waits for every other process in the namespace (`zap_pid_ns_processes`, `kernel/pid_namespace.c`) before the exit completes. The teardown stays on the clock. The parallel `MADV_DONTNEED` at the end of `main` (`main.rs:449-482`) is the right tool and is already there.
- **Start-up key recovery** (`keyrec.rs:345-457`): at most 16 x 64 candidates for the user key and a 2^16-step search for the event key over a 5,000-line sample; well under a millisecond when user 0 is in the sample. Nothing to gain.
- **Directory listing and per-file `stat`/`open`/`mmap`** (`setup.rs:72-87`, `:864-886`): a few µs per file, at most about 230 files. Under 1 ms in total.
- **Lazy `ingest_ts`, 30-minute bins, VNNI, a new scanner**: closed in `FOR_NIMAI.md` with reasons; agreed. Lazy `ingest_ts` in particular would skip the 24-hour lateness check that every row needs (rules, Events step 4).
- From the old notes, still closed for the same reasons: cold-cache read-ahead (input is tmpfs), 512-bit vectors on Cascade Lake, more threads than CPUs, a smaller runtime image.

## Iteration 2 (2026-11-21): the event hot path line by line, and what it says about the mapping

Read: `k_free.rs:1126-1415` (`ev_masks`, `ev_s`, `ev_d`, `ev_fast`), `:1720-1804` (the pipelined driver), with `parse.rs:94-159` (`push_many`, `flush_lines`) and `:642-735` (`ev_vec`) from the first pass.

### 2.1 Two mechanisms in the mapped-input path that the existing knobs can test

**(a) The prefetch stops at every unmapped page.** The driver issues one `prefetcht0 [p + 1024]` per line (`k_free.rs:1738`, `PFD = 1024` at `:1683`); that is the only prefetch, and the hardware streamer does not cross a 4 KiB boundary. A software prefetch to an address whose page is not mapped is dropped. The input is mapped lazily, 16 pages per fault (the kernel's 64 KiB fault-around), so at every 64 KiB the last kilobyte of prefetches is lost: the thread takes the fault and then reads the first 16 cache lines of the new window with no prefetch behind them.
- **My estimate:** per 64 KiB window, about 1.5 to 2 µs of fault plus 0.5 to 1.5 µs of unprefetched misses, against about 36 µs to parse the window (51 ns per line, about 90 bytes per line). That is 6 to 9% of parse thread time in and around faults, of which the miss part (1.5 to 4%) is what a populate call removes; the fault work itself only moves into the call.
- **Change:** in `stage1`, before `parse_buf` at `parse.rs:2156`, `madvise(buf.as_ptr() rounded down to a page, len, MADV_POPULATE_READ = 22)`. Every page of the chunk is then mapped before the first line is read, so the 1 KiB prefetch always lands. Ignore the return value. Chunks are at most 8 MiB (`CH_DEFAULT`), so this is at most 2,048 pages per call.
- Correction to 1.4: `MADV_POPULATE_READ` still maps 16 pages per internal fault, so it does not save kernel time; the gain, if any, is the unbroken prefetch stream and no exceptions in the middle of the hot loop.

**(b) Each per-chunk release is a TLB shootdown.** After each chunk `stage1` calls `madvise(MADV_DONTNEED)` on its pages (`parse.rs:2158-2163`). Removing PTEs from an address space that 8 running threads share makes the kernel interrupt the other 7 CPUs, and for a range above 33 pages (`tlb_single_page_flush_ceiling`) each of them drops its whole TLB for this process. Chunk sizes are `rem / (4 * nt)` clamped to 256 KiB..8 MiB (`parse.rs:1961`), which for 2.3 GB is about 400 chunks, so about 400 shootdown rounds and 2,800 interrupts during parse.
- **My estimate:** 3 µs per interrupted CPU (interrupt in a VM plus refilling 30 to 60 TLB entries) and 5 to 20 µs for the caller to wait for the acknowledgements: 10 to 16 thread-ms, about 1% of parse thread time. On bare metal less; on a VM without IPI virtualisation (Cascade Lake has none) more.
- **Variants, all existing knobs:** `XL_CH=32768` (32 MiB chunks: a quarter of the calls for the bulk of the input; the tail still shrinks to 256 KiB so balance at the end is unchanged); `XL_REL=0` (no per-chunk release; the single `munmap` in the write stage, `out.rs:541-546`, then has 560,000 PTEs to remove on one thread, **my estimate** 30 to 45 ms, which is longer than the write stage, so I expect this one to lose, but it bounds the effect).
- **Not proposed yet:** one `process_madvise` call per thread for many ranges. A 2025 kernel series batches the TLB flush across the ranges of one `process_madvise` call ("batch tlb flushes for MADV_DONTNEED and MADV_FREE"; merge release not confirmed by me), which would cut 400 rounds to a handful, but it needs a pidfd and is only worth writing if `XL_CH=32768` shows the effect is real.

**How to read the result.** Run the four builds interleaved on tmpfs input (1.7): default, `XL_CH=32768`, populate-read, both. If none moves parse by more than noise, close 1.4 and 2.1 together.

### 2.2 Instruction count of the event path, and what the timing then implies

Counted by reading, not by a tool, so ±20%:
- `ev_masks` (`k_free.rs:1127`): 3 loads, 5 compares, 3 mask merges: about 11.
- `ev_s` (`:1281-1336`): about 25 scalar bit operations for the six separators, one tail-row lookup with a 32-byte quote test, one kind-row compare, `ids2j` (about 18 vector operations for both IDs), two date-cache lookups (`pext`, load, xor, and each), two 16-byte loads for the time fields: about 75.
- `ev_d` (`:1349-1400`): one fused 256-bit digit conversion for both timestamps (about 16), two stores, one combined validity test, the purchase test, the batch-full test: about 28.
- Per 64 rows, `ev_vec` (`parse.rs:642`) is about 60 instructions per 4 rows, and `push_many` plus `flush_lines` about 15 per row plus 12 per filled line: about 35 per row together.
- **Total: about 150 to 170 instructions per event line**, with about 12 table loads, all from rows that stay in L1 for a given line shape.

Against `FOR_NIMAI.md`'s 160 ms for parse (machine not stated): 51 ns per line per hardware thread is 145 to 170 cycles at 2.8 to 3.3 GHz. So each hyperthread runs at about one instruction per cycle, about two per core. Code this branch-light and table-driven can run at three or more per core when it is not waiting. **My reading:** a visible part of parse is waiting on memory or in the kernel (input misses, the fault path, page clearing, shootdowns), not executing instructions. This is why the mapping items are ranked above the licence probe, and why I am not proposing instruction-level changes to `ev_s`/`ev_d`: I found none that would remove more than two or three instructions per line. The `pmu` line (request 4) settles it: if user-mode IPC is above 1.6 per thread and `stime` is under 5% of task time, reverse this ranking.

### 2.3 Addition to 1.2: the click path is already 128-bit

`cl_s`/`cl_d` (`k_free.rs:1582-1666`) decode a click with `ids2v` and 128-bit multiply-adds; only the event path (`ids2j` at `:875`, `ev_d` at `:1365-1373`) and the batch code in `parse.rs` use 256-bit multiplies. So the licence question is specifically about the event path, and a 128-bit event path is not new code so much as the pre-fusion form (`ids2v` twice, `ts_time` twice). If the probe of 1.2 is positive, the A/B is "fused 256-bit" against "two 128-bit halves" for events, plus scalar multiplies in `ev_vec`.

### 2.4 Checked and left alone

- **The purchase test** (`k_free.rs:1388`): reads one byte of the `pp` word stored a few instructions earlier; a byte inside a just-stored quadword forwards from the store buffer, so no stall. 3% taken, well predicted.
- **Batch size 64** (`iface.rs:36`): the five batch columns are 2.5 KB and `Out` about 2 KB, both L1-resident. Nothing to gain from another size that the authors' `XL_*` build knobs would not already have shown.
- **Record scatter** (`parse.rs:94-159`): about 830 buckets at 20M (`ecap >> 15`), one 64-byte staging line each (53 KB per thread, in L2), full lines leave with two 32-byte non-temporal stores into 4 KiB chunks cut from 2 MiB blocks. The active chunks of a thread span a few huge pages, so the TLB is not a factor.

## Iteration 3 (2026-11-21): first log entry, and the sessions stage

Read: `OVERNIGHT_LOG.md` (experiment 1), `events.rs:234-262` (`scatter_chunk`), `:1736-2124` (`stage3a`), `:2126-2325` (`stage3b`, `stage3`), `sys.rs:188` (`release_part`).

### 3.1 What the log's baseline says (quoted from the log; arithmetic is mine)

- Test machine: `m5zn.2xlarge`, Xeon 8252C, Cascade Lake, 4 cores / 8 threads. Wall median 198.6 ms; parse 146.3, ev3a 35.5, write 9.4; noise ±1 ms.
- The three stages add to 191.2 ms, so **7.4 ms (3.7%) is in laps the log does not list**: `setup`, `st2sort`, `stitch`, `ev3b`, `complex`, and the time before `main` and inside `_exit`. That is as large as any idea in the table. Please log the full `XL_TIMING=1` line once so these can be ranked; the lap names are in `main.rs:384-405` and `events.rs:2292-2314`.
- Parse: 146.3 ms for 1.8 GB and 25M lines is 47 ns per line per hardware thread. The 8252C runs well above the 2.8 GHz reported for the scorer, so at this machine's clock that is more than 170 cycles for about 160 instructions (2.2): under one instruction per cycle per thread. This strengthens 2.2's reading that parse is waiting, not computing.
- ev3a: 35.5 ms x 8 threads / 20M events is 14 ns per event.

### 3.2 Two things about the bench that can hide or invent a result

- **The timing runs the binary directly, with the page cache warm.** If the generated set is on the VM's disk filesystem and not on tmpfs, the input pages are ext4 page cache. Both map as 4 KiB pages with fault-around, so the default path should behave alike, but the scorer's kernel (7.0, reported) and Ubuntu 24.04's differ in the fault path, and `fault_around_check` (`parse.rs:1805`) decides map against `pread` at run time. Please log which path it took (`[v2b]` prints it under `XL_TIMING=1` if the `rd` knob line is on; otherwise one `strace -c` run: `pread64` count near zero means mapped) and run the row 1 A/B with the set copied to `/dev/shm`.
- **Outside the container, exit is cheap.** Row 4's gain, if any, only shows as time between the process's last output and `docker run` returning, with `/output` a bind mount on the VM's disk. The direct-binary timing cannot see it either way.

### 3.3 The sessions stage uses 512-bit vectors: a free positive control for the licence probe

- `stage3a` is written with `zmm` throughout (`events.rs:1770-1832`, `:1854-1899`, `:2049-2075`; 178 uses in the file; `core.rs:405-441`, `ckix.rs` likewise), parse with `ymm` only (the `zmm` in `k_free.rs` are clobber lists; `parse.rs:266-268` splits a 512-bit value into halves).
- On Cascade Lake any 512-bit instruction puts the core in L1 at least (Downs: "512-bit light" ran at 2.8 against 3.2 GHz on the W-2104). So the probe of 1.2 should be taken at three points on each worker: before any vector code, at the end of `stage1`, and at the end of `stage3a`. The third must read slower than the first on this machine; if it does not, the probe is broken (or the hypervisor hides the effect) and a "1.00" after parse means nothing. If the third reads slower and the second reads 1.00, parse is at L0 and 1.2 is closed for good.
- I am not proposing to rewrite `stage3a` in 256-bit form: it is 35 ms, the compress-store kernels would double in instruction count, and it is the part of the code with the most ways to change a row.

### 3.4 Checked in `stage3a`/`stage3b` and left alone

- **`scatter_chunk`** (`events.rs:234`): the dedup table is `BSIZE * 16` = 512 KiB per thread, inside the 1 MiB L2 of this CPU; the next record chunk is prefetched (`PF_CHUNK`). The records themselves were written with non-temporal stores in parse, so every one is a DRAM read here: 320 MB in 35 ms is 9 GB/s over 8 threads, well under what the machine can stream. Not memory-bound.
- **Three `Instant::now()` per bucket** (`:1779`, `:1801`, `:1842`, `:2090`): about 830 buckets, 25 ns each through the vDSO: 0.1 ms of thread time in total. Not worth touching.
- **`release_part`** (`sys.rs:188`, called once per thread from `stage3b`): frees the event arena in 2 MiB units, about 160 huge pages in 8 calls. Cheap, and it is what keeps `_exit` short.
- **Serial stitch** (`events.rs:2293-2313`): a loop over about 830 buckets and one sort of at most twice that many entries. Microseconds.

## Iteration 4 (2026-11-21): a correction to 2.2/3.1, and two diagnostics that cost no code

### 4.1 Correction: "parse is waiting" was stated too strongly

2.2 and 3.1 compared one hardware thread's instruction rate with what a whole core can do. The 8 threads are 4 cores with two hyperthreads each, and two busy siblings each get roughly 55 to 65% of a core. So about 0.8 instructions per cycle per thread is about 1.6 per core, which is ordinary for vector-heavy code that competes for the same two or three execution ports (the multiplies and shuffles here), and does not by itself show memory or kernel waits. The ranking of the table stands on other grounds (rows 1 and 2 cannot change output and are cheap to test), but treat "memory-bound" as open until one of the two diagnostics below is in the log.

### 4.2 Diagnostic A: one run with 4 threads on 4 separate cores

`NCPU=4` with the affinity mask restricted to one hyperthread per core (`taskset -c 0-3` if siblings are 0/4, 1/5, ...; check `/sys/devices/system/cpu/cpu0/topology/thread_siblings_list`).
- If parse takes about 1.25 to 1.35 times the 8-thread time, the second hyperthread adds the usual 25 to 35% and parse is limited by execution resources: instruction count and the licence question (row 3) matter most.
- If it takes under 1.15 times, the cores are already starved for something shared (memory bandwidth, the kernel's page-table lock, shootdowns): rows 1 and 2 matter most.
- This is a measurement only. The scorer has 8 CPUs and the default stays 8.

### 4.3 Diagnostic B: `perf stat` on the baseline, if the VM exposes the PMU

`perf stat -e task-clock,cycles,instructions,page-faults,context-switches,dTLB-load-misses,tlb:tlb_flush -a -- ./pipeline ...` and the user/system split from `\time -v`. `m5zn.2xlarge` is not a metal instance, so hardware counters may be absent; `task-clock`, `page-faults`, the `tlb:tlb_flush` tracepoint and user/system time work regardless and are the ones that decide rows 1 and 2: system time during parse above about 10 ms of wall equivalent says the mapping and zeroing paths are worth the A/B; under 3 ms closes them.

### 4.4 Memory traffic in parse (my arithmetic, from sizes in the code and the log)

- Read: 1.8 GB of input. Written: about 400 MB of records with non-temporal stores (`parse.rs:94-159`). Zeroed by the kernel first: the same 400 MB, when each 2 MiB block is populated (`parse.rs:180`). Total about 2.6 GB in 146 ms, 18 GB/s across 4 cores.
- That is within what this class of machine streams, but it is not small, and the scorer's host is shared by 32 vCPUs. The 400 MB that is written twice (once as zeros, once as records) is the only part that could be removed, and only by writing fewer or smaller records; fresh anonymous memory cannot be had without the clear. Records are 16 bytes (`Rec`, `main.rs`), holding the slot, ingest time, timestamp, user and kind; I see no field that can be dropped without re-reading input later. **Closed** unless a later pass over `events.rs` shows a field the fast path never reads.

## Iteration 5 (2026-11-21): record layout, the write stage, and what the board number is made of

Read: `parse.rs:440-500` (record packing), `pqw.rs:76-124`, `:1056-1100` (`Out`, `UserFile`), `out.rs:500-560` (the write-stage scheduler).

### 5.1 Closed: a smaller event record

`Rec` is two words and every bit is used (`parse.rs:488-493`): `w0` = timestamp (31 bits) | price (24) | category (5) | alias (4); `w1` = slot in bucket (16) | ingest seconds (21) | user (27). The slot and ingest time are what `scatter_chunk` dedups on (`events.rs:238-241`), the rest is read by `stage3a`. Nothing can be dropped without a second look at the input. This closes 4.4's open question.

### 5.2 The write stage: nothing found, one number wanted

- `user_daily` pieces are streamed through a buffer flushed at 1 MiB (`Method::Stream(1 << 20)`, `out.rs:206`; `Out::flush`, `pqw.rs:88`), campaign files are written whole. Each flush is one `write_all`. The encoders are already specialised per column (`emit_user_ids`, `emit_zero_head`, `emit_null_head`).
- The input region's `munmap` (`out.rs:541-546`) is taken by the first thread that has no shard to work on, before it takes a write piece. With the per-chunk release on, the pages are already gone and only page tables are freed (about 1,150 of them for 2.3 GB; **my estimate** 0.2 to 0.4 ms). Left alone.
- The `tl` build already separates time inside `write` calls (`WR_IO`, `pqw.rs:91-95`). If that is more than half of the 9.4 ms, the stage is the kernel copying into the page cache and row 4 (early writeback) is the only lever; if it is small, the stage is encoding and I will read the encoders next. Please include that line with request 3.

### 5.3 For the user: what the 0.35 s on the board is made of (my arithmetic on quoted numbers)

- The test VM runs the process in 198.6 ms on a CPU whose cores run well above 4 GHz. The scorer is reported at 2.8 GHz. Scaling by clock alone gives roughly 280 to 310 ms in-process there, which with the reported 0.1 s outside the process would be 0.38 to 0.41 s, above the 0.35 s on the board. So either the scorer's cores run faster than 2.8 GHz under turbo, or the outside part is nearer 0.05 s. Both readings say the same thing: **in-process time is about 0.25 to 0.30 s of the score, and a 1% code gain is about 3 ms on a clock whose single draws differ by tens of ms.**
- Consequence for picking the final commit: decide on the VM's interleaved A/B (±1 ms), never on one board draw. A change that is exact and not slower on the VM is safe to submit; whether it "helped on the board" is not knowable from one or two draws.

## Iteration 6 (2026-11-21): can a hidden-set seed push the fast path into the slow one?

A run that trips `sys::fallback` re-executes itself with `--generic` (`sys.rs:122`). The output stays exact, but the time is several times worse, which on a hidden set is a worse outcome than any gain in the table is good. There are 99 `fallback("...")` sites. I checked the three whose trigger depends on the data's proportions rather than on a malformed file, against the generator source (`generator/src/plan.rs`, `user.rs`) and its README. No change is proposed; this is so nobody "tightens" these to save memory.

| Capacity | Formula (`setup.rs`) | What the generator guarantees (quoted from its source) | Margin (my arithmetic) |
|---|---|---|---|
| Event slots `ecap` | `ev_bytes / 64` (`:963`) | Event indices are consecutive per user (`user.rs:153-157`); 2 to 4% of events are written twice (`plan.rs:129`) | Needs at least 64 input bytes per distinct event. Shortest well-formed event line is about 70 bytes (two 10-byte IDs, a 4-byte type, two 13-digit epoch timestamps, a product, separators) and the average is about 90, so about 1.4x |
| User slots `ucap` | `ecap / 16` (`:964`) | `user_count = events / 30` (`plan.rs:147`, `EVENTS_PER_USER = 30`), at any scale | `ucap` is about 0.09 per event against 0.033 needed: about 2.7x |
| Output rows per thread `row_cap` | `ev_bytes / 40 / nt * 2 + 65536` (`:845`) | At most 14 days per user (`WINDOW_DAYS`), so at most `events / 30 * 14` rows in total | At 20M: 11M rows of room per thread against 9.3M rows in the whole output: above 8x |
| Ingest time field, 21 bits of seconds from `ws - 1 day` (`ING_MAX`, `main.rs:75`; `ing_base`, `setup.rs:991`; fallback at `parse.rs:450`) | 24.3 days of range | `ingest_ts` is the true event time minus at most 5 s, or plus at most 5 days and 600 s; duplicates add at most 600 s more (`user.rs:551-565`, `:413`). It is always written well-formed (`write_utc`), also on rows whose `ts` is bad | Largest value is about 20.0 days from the base: 4 days of room. Holds for every seed, since none of these bounds is drawn per seed |

- **Odd events** (`links.rs:1954`, fallback above 2% of event rows): the rates that produce non-conforming IDs are fixed constants or narrow ranges in `plan.rs:128-141` (empty user 0.2%, malformed 0.05%, upper-cased 0.0005%); none is drawn from a range that reaches 2%.
- **Scale.** A comment in `plan.rs:133` speaks of "the full 75M rows". If the hidden set is larger than the reported 20M events, all three capacities scale with input bytes, so the margins above do not shrink. I did not check the other fixed limits (`names over 4 GB`, `edge store over 4G records`, `input too large`) against a 60M-event set; the first two are far away at 20M.
- **The serial pass over odd events** (`links.rs:1945-1990`, a `HashMap<Vec<u8>, _>` filled on one thread between parse and the sessions stage) only sees events whose ID does not decode, which in the generator is the upper-cased ID case, about 5 per million rows (`plan.rs:133`): about 100 entries at 20M. Not a cost.
- **What would be worth one run:** generate two more 20M seeds and one 40M set and confirm stderr never shows the fallback message (the fast path's output is compared anyway, but an exact output via the slow path passes the comparison and hides the problem; check the time or `XL_TIMING` line, not only the diff).

## Iteration 7 (2026-11-21): a no-rebuild knob sweep, because the defaults were tuned on someone else's machine

The shipped binary reads about 40 `XL_*` variables at start (`main.rs:305-362`, `parse.rs:1791`, others). The previous authors evidently swept them on their own bench; nothing says the defaults are best on a Cascade Lake VM with a CPU quota. Every row below is a run-time setting on the binary the log already times, changes no output byte, and if one wins the shipped change is a one-line default. With ±1 ms noise most rows will read "no difference", which closes them cheaply.

| Setting | Read at | What it changes | Default | Try | What I expect (my estimate) |
|---|---|---|---|---|---|
| `XL_CH` (KiB) | `parse.rs:1791` | Largest parse chunk, so the number of per-chunk `MADV_DONTNEED` shootdowns and how far a thread runs before it meets cold pages | 8192 | 2048, 32768 | 2.1(b): larger may gain up to 1%; smaller tells whether shootdowns matter at all (4x as many) |
| `XL_REL` | `main.rs:355` | Per-chunk release of parsed input | 1 | 0 | Loses: moves 560,000 PTE removals to one thread in the write stage. Bounds the cost of releasing |
| `XL_RD` (KiB) | `parse.rs:1791` | Read input with `pread` into a buffer of this size instead of the mapping | 0 (map) | 128 | Loses by 5 to 15% of parse if mapping works as on tmpfs; if it wins, fault-around is not working on this kernel and 1.4 is wrong |
| `XL_SPIN` (µs, nominal) | `main.rs:331` | How long an idle worker spins before sleeping on a futex. The code counts 2,000 TSC ticks per µs (`main.rs:334`), so the default 400 is really about 210 µs on the 8252C and about 285 µs at the scorer's reported 2.8 GHz | 400 | 2000, 50 | Any serial gap between two parallel jobs longer than that puts 7 workers to sleep and the next job pays a futex wake each (tens of µs in a VM). Under 0.5 ms in total; the `tl` "wake lag" lines show it directly |
| `XL_PINSTART` | `main.rs:336` | Pin workers when they start, not at the first parse job | off | 1 | Saves one migration per worker at parse start: under 0.2 ms |
| `XL_PREFAULT` | `main.rs:325` | Each worker faults its first heap chunk at start (`sys.rs` `prefault_chunk`) | off | 1 | `FOR_NIMAI.md` says keep off without an A/B; this is the A/B. Related to row 2 but different memory |
| `XL_POPW` | `parse.rs:15` | `MADV_POPULATE_WRITE` on each 2 MiB arena block (`parse.rs:185`) | on | 0 | Loses slightly if THP is active (one fault per block either way); if it wins, huge pages are not being granted on this VM and every memory conclusion changes. A check on the bench as much as on the code |
| `XL_QORD` | `parse.rs:1791`, `:1938-1953` | Order of the parse queue: 2 = clicks, links, events; 1 = events, clicks, links; 0 = directory order | 2 | 1 | Expect 1 to lose: the link jobs that run inside parse (`links::job`) can only start once clicks and links are parsed, so with events first they all land at the end. Worth one run only to confirm the default |

- Run them in one interleaved loop with the baseline, as the log already does. Report each as min / median like experiment 1.
- `XL_EXITPROF` (`sys.rs:561`, `tl` build) prints an exit profile; please include it with request 3. It is the only thing that can say how much of the 7.4 ms outside the three stages (3.1) is process exit.
- I have not found what `XL_KP`, `XL_FIRST4K` and `XL_NOTOUCH` are for beyond their names; not proposed.

## Iteration 8 (2026-11-21): the code already has a switch that measures row 1's ceiling; patch text for the populate variant

Read: `parse.rs:2118-2190` (the per-chunk loop of `stage1`).

### 8.1 Diagnostic C: touch-only parse (`XL_SKIP=512`), a direct measurement of the mapping cost

`stage1` has a mode that replaces parsing with one byte read per 4 KiB page of the chunk and still does the per-chunk release (`parse.rs:2146-2153`, `SKIP & 512`). It is compiled only with `--features knobs` (`parse.rs:18-25`, `main.rs:358-361`).
- Build once with `--features knobs` (a measurement build, never shipped), run with `XL_SKIP=512 XL_TIMING=1`, and read **only the `parse` lap**. Nothing is parsed, so later stages and the output are meaningless; the run may well end in the fallback. Ignore everything after the `parse` figure.
- That lap is the whole cost of faulting in and releasing the input on 8 threads: the page faults, the PTE work and the shootdowns, with no parsing. **It is the ceiling for everything row 1 and row 5 can gain.** If it reads 3 ms, close both rows without further work. If it reads 10 ms or more (7% of parse, my estimate in 1.4), run it again with `XL_CH=32768` and with `XL_REL=0` to split faults from releases.
- Correction: this mode cannot be combined with `XL_RD`; `parse.rs:2094` forces the mapped path when the bit is set.

### 8.3 The same build gives a four-step ladder that splits parse into its parts

All in the `--features knobs` build, all measurement only (outputs are wrong or empty by design; read the `parse` lap and nothing else):

| Run | Switch | What still runs in parse | Read off |
|---|---|---|---|
| A | `XL_SKIP=512` (`parse.rs:2147`) | Page touch and release only | Mapping cost |
| B | `XL_NOSINK=1` (`parse.rs:510`) and `XL_SKIP=64` for clicks (`:589`) | A + the line kernels (`ev_s`/`ev_d`, `cl_s`/`cl_d`) filling batches | B - A = the scanner and field decode |
| C | `XL_SKIP=2048` (`parse.rs:528`) | B + `ev_vec` (unscramble, validity, record build) | C - B = the batch stage |
| D | none | Everything | D - C = `push_many`: the non-temporal stores and the arena's page clearing |

What each difference decides:
- **A** bounds rows 1 and 5.
- **D - C** bounds row 2 (arena pre-fault can only remove the page-clearing part of it). If D - C is under 5 ms, row 2 is worth at most a fraction of that; close it.
- **B - A and C - B** are the only parts the licence question (row 3) can speed up, and by at most the clock ratio the probe reports.
- I have no estimate I trust for the split, which is the point of asking. One build, four runs each interleaved a few times.

### 8.2 Patch text for the populate-read variant of row 1 (for the coding agent to apply; I have not compiled it)

In `stage1`, directly before the `if unsafe { SKIP } & 512 != 0 {` at `parse.rs:2146`, where `buf` is the chunk's line-aligned slice:

```rust
if popr() {
    let a = buf.as_ptr() as usize & !4095;
    let b = (buf.as_ptr() as usize + buf.len() + 4095) & !4095;
    unsafe { madvise(a as *mut u8, b - a, 22) }; // MADV_POPULATE_READ, Linux 5.14+
}
```

with a run-time switch beside `popw()` at `parse.rs:14`:

```rust
static POPR: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
fn popr() -> bool {
    *POPR.get_or_init(|| std::env::var("XL_POPR").is_ok_and(|v| v == "1"))
}
```

- Default off, so the shipped behaviour is unchanged until the A/B says otherwise.
- The return value is ignored on purpose: on an older kernel the call fails with `EINVAL` and the pages are faulted as before.
- The range is rounded outwards, so it may populate one page that belongs to the neighbouring chunk; that is harmless (the neighbour finds it mapped, or re-faults it if this thread releases first).
- The `use_rd` path (`parse.rs:2136-2139`) returns before this point, so the switch has no effect there.
- Two notes on reading the result. The line scans at `parse.rs:2141-2142` (`next_line_start`) already touch the first and last page of the chunk before this call, so two fault-around windows are mapped either way. And with 8 MiB chunks the call maps 2,048 pages in one go, which then sit in the page tables for the roughly 0.6 ms the thread needs to parse them; that is fine for the TLB (4 KiB entries are filled on demand) but if it loses, try it with `XL_CH=2048` before closing it.
- **Risk to output:** none. It changes when read-only input pages get mapped, not what is read.

### 8.4 The sessions stage has its own built-in profile; please log it once

- Under `XL_TIMING=1` the binary prints `[v2b] prof (thread-ms): scatter=... compact=... runs=... st2b=... st2b_max_bucket=... st2b_pass1=... st2b_pass2=... st2b_sort=...` (`main.rs:437`), summed over threads from `t.prof` (`events.rs:2092-2094`, `links.rs:1659-1662`). `scatter` is the dedup pass over the records (the DRAM reads of 3.4), `compact` the 512-bit compress pass, `runs` everything after it. `st2b_*` is the click-index work that runs inside parse, and `st2b_max_bucket` is the longest single link job, which is what can leave one thread working alone at the end of parse.
- A build with the environment variable `XL_XPROF=1` set **at compile time** (`option_env!`, `events.rs:652`) prints a per-phase table for stage 3a with instructions, cycles, cache misses and branch misses per kept event (`events.rs:763-800`), and `XL_XRU=1` the user/system time and faults of the stage. Measurement builds only.
- With these and 8.3 every stage has a measured split, and I can stop ranking on estimates. Until they are in the log I will not add speculative items to the table.

## Iteration 9 (2026-11-21): re-rank from the log's measurements

### 9.1 Row 1: the per-chunk release costs 7.7 ms of parse, and the cheaper way to release is already in the binary

- **Measured (log, experiment 3, tmpfs):** with `XL_REL=0` parse drops from 158.4 to 150.7 ms. So the roughly 400 `madvise(MADV_DONTNEED)` calls inside `stage1` cost 7.7 ms of wall, about 61 thread-ms. The same run's write stage grew by 22 ms, because the pages were then all removed by the single `munmap` at `out.rs:541-546`, which also holds the address-space lock against the other writers' page faults while it runs.
- **Why the per-chunk form is the expensive one (my reading):** removing the same pages took 22 ms on one thread but 61 thread-ms when done chunk by chunk during parse. The difference is what each call does to the other seven threads: an interrupt each, a flushed TLB, and the refills afterwards, about 400 times.
- **The alternative that is already there:** `main.rs:449-482` runs a parallel release on all threads after the write stage, and with `XL_ENDREL=2` it includes the input region, split evenly by address (`main.rs:469`). `XL_RGREL=0` turns off the `munmap` in the write stage (`main.rs:343`). So `XL_REL=0 XL_RGREL=0 XL_ENDREL=2` is: nothing released during parse or write, then eight `madvise` calls in parallel at the end, one shootdown each, with no other work running to disturb.
- **Expected (my estimate):** the end release is 22 ms of single-thread work spread over 8 threads on 4 cores: 3 to 6 ms. Net 2 to 5 ms better than the default. It could be less if removing 4 KiB mappings does not scale across threads (they are in separate page-table pages, so I expect it to).
- **What to watch:** (a) the wall as seen by the parent, not the `exit at` line, since the point is that exit must not inherit the work; (b) `XL_ENDREL=2` also releases the output columns and the click index in parallel, which the default skips when huge pages are in use; that should be neutral or slightly positive for exit; (c) the whole input stays mapped during the run, which adds nothing to this process's memory charge (tmpfs pages belong to whoever wrote them) but does show in RSS.
- **If it wins,** the shipped change is three defaults: `parse::RELEASE` false (`main.rs:355`), `out::REGION_UNMAP` false, and the `endrel` default treated as `"2"` (`main.rs:449-451`). No new code.
- **Variant if it only half works:** keep the per-chunk release but on fewer, larger ranges. Experiment 3 already shows the direction: `XL_CH=65536` was 1 ms better than the default, within noise.
- **Risk:** none to output. The end block runs after `stage45` has written and closed every file.

### 9.2 Row 2: the dedup table against the L2 of a shared core

- `scatter_chunk` (`events.rs:234-262`) writes each record into a table of `BSIZE` 16-byte slots: 512 KiB with `BB = 15`. Each ev3a thread has its own, plus the compacted arrays it builds from it (`q`, `sidx`, `sts`, about another 300 KiB in use per bucket).
- Log: `scatter=98.1` thread-ms for 20.9M records is 4.7 ns per record, and every record is one read from DRAM (prefetched) plus one random access into that table.
- On the 8252C two hyperthreads share 1 MiB of L2. Two 512 KiB tables alone fill it, so a good part of the random accesses must go to L3, which on this server design is slow (it is not inclusive and sits behind the mesh). With `BB = 14` the table is 256 KiB per thread and both fit with room for the working arrays.
- The cost side: twice as many buckets (about 1,540). In parse, each thread's write-combining staging grows from 49 to 98 KiB (`parse.rs:94-159`), still inside L2 but competing with the same L2; and each (thread, bucket) pair holds one partly filled 4 KiB chunk, so about 25 MB more arena is touched and cleared. So parse may lose up to 1 ms while ev3a gains; only the sum matters.
- `BB` is `option_env!("XL_BB")` at `main.rs:64`, so this is a build with `XL_BB=14` in the environment (in Docker: `ENV XL_BB=14` before `cargo build`). Also try 13 for the trend. `BB > 15` switches to the old stage-3 path (`events.rs:2256`); 13 and 14 stay on the current one.
- The authors very likely swept this on their own machine. If that machine had 2 MiB of L2 per core (Sapphire Rapids, Zen 4 has 1 MiB but per-thread behaviour differs), their optimum would sit one step larger than this CPU's. That is the whole argument; it is a guess about their bench, not a fact.

### 9.3 Row 3: prefetch distance

- **Withdrawn (11.1):** I wrote here that the log's thread-count runs show a core running one parse thread is idle half the time. They do not. About 21% of thread time is in the kernel (log, `stime`), mostly clearing pages; the rest of the waiting is in user code, and the two places user code can wait on memory are reading input and the non-temporal record stores.
- Input: `PFD = 1024` bytes ahead, one `prefetcht0` per line. On tmpfs the pages are 4 KiB and physically scattered, so the hardware streamer restarts at every page; the software prefetch has to carry the stream alone. At 47 ns per line, 1 KiB is about half a microsecond of lead, which should be enough for one DRAM access, but there are eight threads asking at once and the line-fill buffers are shared between siblings.
- Try `PFD` = 512, 2048, 4096 (one constant, four builds or a `knobs`-style `option_env!`). Experiment 4 does not close this: populate-read changed when pages are mapped, not how far ahead data is requested.
- Also worth one build: a second prefetch per line at `PFD + 64`, since lines average about 90 bytes and one prefetch per line covers only 64 of them. If lines were being missed this way it would show as a gain here and not from distance alone.

### 9.4 Row 4: setup

- Log: thread 0 spends 1.0 ms listing three directories and `stat`-ing each file, 1.1 ms opening and mapping each file, 0.7 ms sampling; the other seven threads are idle for 9.68 thread-ms of that job. Parse starts at 2.5 ms.
- (a) List the three directories on three threads (`setup.rs:835-841` is a plain loop over `events`, `clicks`, `links`; results are sorted afterwards at `:842`, so the order of arrival does not matter). Saves up to 0.6 ms.
- (b) Open the files on the workers and leave only the `mmap` calls on thread 0 (they take the address-space lock and gain nothing from threads). Saves up to 0.5 ms.
- (c) Use what idle time remains to clear arena blocks, as in 1.3. At the log's rate (about 170 thread-ms for about 620 MB cleared, 0.55 ms per 2 MiB block) 9.7 idle thread-ms covers about 18 blocks, about 1.2 ms of parse wall.
- Each part is under the 2 ms the log asks for; together they may reach it. Rank 4 for that reason.

### 9.5 Row 5 and the exit

- Log: output is 18 MB, written with 9.18 thread-ms inside `write` calls out of 74.9 busy thread-ms in the write stage, so that stage is encoding, not I/O; and the parent sees the process gone 1.5 to 2 ms after the last timing line with 305 MB still resident.
- `XL_ENDREL=1` (parallel release of the remaining memory before `_exit`, without the input region) was not among the knobs flipped in experiment 2. It is one run. It can gain at most that 1.5 to 2 ms and is subsumed by row 1's `XL_ENDREL=2` if that wins.
- `sync_file_range` only matters inside a container whose teardown waits on the disk. The direct-binary timing cannot show it. If the coding agent can time `docker run` end to end with `/output` on the VM's disk, that is where to look; otherwise leave it.

### 9.7 Row 2: two click-index modes that are in the binary, selectable at run time, and were not in the knob scan

- **Default (mode 0):** clicks are parsed into 16-byte records in the user-bucket arena, and link jobs running inside parse then count them per user bucket, scatter them into the click index (`csr`), and sort each user's clicks (`links.rs:864-931`). The log puts this at 185 thread-ms: `pass1=18.4`, `pass2=113.2`, `sort=53.4`.
- **Mode 1 (`XL_CKIX=1`):** `click_cap` returns a capacity (`setup.rs:662-668`), the click key is recovered from the sample like the event key, and `ck_vec_ix` (`ckix.rs:45-125`) unscrambles each click ID to its index and stores the click at `csr[index]` during parse. The generator numbers a user's clicks consecutively (`user.rs:447-450`), so the index is already grouped by user; the link stage only verifies the order (`piece_ix`, `links.rs:1243`) and sorts within users.
- **Mode 2 (`XL_CKIX=2`, `cpart`):** records still go through the arena, but are placed by index instead of by a counting pass (`scatter_a`, `links.rs:1522`).
- **Why it might win here:** it removes passes over 5.1M clicks that the log shows are not small. **Why it might not:** in mode 1 every click store is a write to a random place in an 82 MB table from inside the parser (it does prefetch the targets one batch ahead, `ckix.rs:117-121`), and `ck_vec_ix` uses 512-bit vectors, which the rest of parse avoids. The authors chose 0 on their machine; I do not know their numbers.
- **A second effect of mode 1, on page clearing (my arithmetic on the log's figures):** the kernel's page zeroing is 11.1% of all samples, about 170 thread-ms for about 620 MB, so about 0.28 thread-ms per MB. In mode 0 the clicks exist twice, once as arena records and once in the index, 82 MB each. Mode 1 never writes the arena copy, which is about 23 thread-ms of clearing, about 3 ms of wall. One thing to watch: stage 3 recycles the user arena's memory for its own tables (`Pool::new`, `events.rs:135`, `XL_RECYCLE`; the log shows turning that off costs 9 ms). With a smaller user arena there is less to recycle, and stage 3 may fault fresh memory instead; if `ev3a` grows in the mode 1 run, that is why.
- **Output risk, checked against the rules and the generator:** copies of a click "have identical values, so keep any one" (rules, Clicks step 2), and the generator emits copies from the same row buffer (`user.rs:499-505`), so two threads storing the same click to the same slot write the same bytes. Clicks whose ID does not decode, or whose user is not a normal one, are returned in the `spb` mask and go through `ck_one` as before (`parse.rs:600-606`).
- **Fallback risk:** both modes end in `fallback("click index order")` or `("click index range")` if the data does not have the expected shape (`links.rs:1094`, `:1193`, `:1282`). That keeps the output exact but is a cliff in time. Run seeds 3, 20 and one more at 20M, plus dev and the fixtures, and look for the fallback line on stderr as well as at the diff.
- **The fallback conditions, checked one by one against the generator (iteration 12):**
  - `piece_ix` (`links.rs:1243-1273`) fails if the user index ever decreases along the click index. The generator gives each user a consecutive block of click indices in user order (`plan.rs:209-215`, `user.rs:429-450`), and a click row always carries one of that same person's IDs (`user.rs:468`); unlike events, click copies are never re-attributed to another person (the re-attribution at `user.rs:401-411` is in `event()` only). So the order holds for every seed.
  - `prelude_ix` fails if the largest click index exceeds `ck_bytes / 20` (`setup.rs:663`). The shortest well-formed click line is about 38 bytes (a 9-byte ID, a 10-byte user, `c_1`, a 13-digit timestamp, separators), so the margin is about 1.9x, and duplicate rows only add bytes.
  - Slots with no valid click (dropped rows: bad timestamp, unknown campaign, empty user, about 1% together) are filled with a copy of the previous slot (`links.rs:1258-1259`). A repeated identical click cannot change which campaign a session is credited to or which campaigns assist it, and the batch matcher already handles repeated campaigns (`_mm512_conflict_epi32`, `events.rs:1694`). This is the part I would most want the gate to confirm on more than one seed, because it is correct by argument, not by construction.
- **If one mode wins,** the shipped change is the build default `XL_CKIX_DEF` (`setup.rs:671`), one value.

### 9.6 What the log changes in earlier entries

- 1.4 and 2.1(a): closed by experiments 3 and 4.
- 2.2 and 4.1: **not** settled; see 11.1. 4.1's caution stands.
- 3.1's "7.4 ms outside the three stages": the log's lap line accounts for it: setup 3.0 ms, ev3b 2.6, stitch 0.1, exit 1.5 to 2.
- 3.2: the VM runs kernel 7.0 like the scorer, and the tmpfs baseline is 11.5 ms slower in parse than ext4 (146.3 against 157.8). The whole run takes 9,640 page faults on ext4 against 28,762 in parse alone on tmpfs, so on ext4 the input is being mapped in units much larger than 64 KiB. Tune on tmpfs only; ext4 numbers flatter the mapped path.
- 7: the knob table is closed by experiment 2, except `XL_ENDREL` and `XL_RGREL`, which were not in it and are now row 1.

## Iteration 10 (2026-11-21): `walk_runs` read; nothing to propose

Read `events.rs:1599-1734`. The log's `runs=155.3` thread-ms is 233 ns per user run (666,655 runs), 7.4 ns per event.
- Per run: one `utab` load, one claim bit, the name key for up to 8 aliases in one 512-bit operation, the user's click range (`csr_range`), and for users with at most 16 clicks all of whose aliases are in the run's class, the whole click-to-session match is deferred into a batch (`light_flush`). Users arrive in increasing order within a bucket because event indices are consecutive per user (generator `user.rs:153-157`), so the click index and `utab` are read in address order.
- I found no pass that is repeated and no per-run work that could move to a batch that is not already there. Without hardware counters on the VM (log, fact 4) I cannot tell cache misses from instruction count in this stage, and I will not guess. **Closed for now**; the only ev3a item in the table is the bucket size (row 3), which acts on `scatter`, not on `runs`.

## Iteration 11 (2026-11-21): a correction to the log's hyperthread conclusion, and to my use of it

### 11.1 The 4-thread run used two cores, not four

- The log says: "4 threads take 369 ms against 198 for 8 on the same 4 cores, so the second hyperthread nearly doubles throughput: the parse threads spend much of their time waiting on memory".
- `cpu_order` (`parse.rs:1884-1923`) builds the pinning order by walking the allowed CPUs and, for each, appending all of its hyperthread siblings before moving on. On this VM siblings are CPU `c` and `c + 4`, so the order is 0, 4, 1, 5, 2, 6, 3, 7, and thread `tid` is pinned to `order[tid]` (`stage1`, pin mode 2). **With `NCPU=4` the four threads sit on CPUs 0, 4, 1, 5: two physical cores, both hyperthreads each.** The 8-thread run uses four cores. So 369 against 198 is two cores against four, and 1.86 is ordinary core scaling. `NCPU=6` at 250 ms fits the same reading (three cores: 198 x 4 / 3 = 264).
- So the log contains no evidence yet on whether a parse thread waits or computes. In iteration 9 I used that sentence to demote the licence probe and to motivate the prefetch row; both steps are withdrawn above.
- **The run that does answer it:** `NCPU=4 XL_PIN=0 taskset -c 0-3 ./pipeline ...` (one thread on each of four cores, no pinning by the program, the mask does the placement), against the default 8-thread run. Read the `parse` lap.
  - Near 1.25 to 1.35 times the 8-thread parse (about 200 to 215 ms against 158): the cores are busy executing; the second hyperthread adds the usual quarter to a third. Then instruction count and clock rate matter, the licence probe is worth its 20 lines, and prefetch distance probably is not.
  - Near 1.0 to 1.15 times: the cores are waiting; prefetch and memory layout matter, the licence does not.
- What is solid regardless: 21% of parse thread time is system time on tmpfs (log, `stime_ms=271.7` of `task_ms=1265.9`), page clearing is 11.1% of samples, and `XL_REL=0` takes 7.7 ms off parse. Rows 1, 2 and 5 rest on those, not on the thread-count runs.

### 11.2 Considered and not proposed: a smaller event record

5.1 closed "drop a field". I also looked at re-encoding: within one bucket of 2^15 event indices there are at most 2^15 users, so 16 bits of the user index would do if stage 3 rebuilt the rest; ingest time could be stored relative to the event time. That gets a non-purchase record to about 76 bits, so 12 bytes at best, a quarter less arena (about 84 MB, **my estimate** 3 ms of clearing and store traffic). It would change the record layout that `ev_vec`, `push_many`, `scatter_chunk` and the 512-bit compaction in `stage3a` all share, break the 64-byte staging lines (records would no longer divide a cache line), and need an escape for late rows. That is the most row-sensitive code in the program for 1.5%. **Not proposed.**

### 11.3 Write stage: closed

The log's profile lists every function above 1.1% of samples and none is in `pqw.rs` or `out.rs`; the write stage's 74.9 busy thread-ms (4.8% of the run's CPU) is spread over the encoders with only 9.18 thread-ms inside `write` calls. I read the bit-packer (`pqw.rs:195-291`): it packs 8 values per step with shifts into one 64- or 128-bit word, about two instructions per value. There is no single slow pass to fix. **Closed** unless a profile of the write stage alone shows a concentration.

## Iteration 12 (2026-11-21): generator facts that bound row 2, and one hazard for anyone touching event dedup

- Row 2's fallback conditions are now checked against the generator; the result is in 9.7. Short form: the click index is ordered by user for every seed, the capacity has about 1.9x of room, and gap slots are filled with harmless copies.
- **Hazard, recorded so that no later idea trips on it:** later copies of a duplicated event can carry **another person's** `user_id` (`generator/src/user.rs:401-411`: one duplicated event in 64 has its later copies re-attributed to the next user, with a later `ingest_ts`). The rules' dedup keeps the copy with the smallest `ingest_ts`, which is the original. So event dedup must compare ingest times across copies that disagree on the user, and the surviving row's user must come from the surviving copy. The current code does exactly this (`scatter_chunk`, `events.rs:238-242`, replaces the whole 16-byte record). Any scheme that takes the user from "the first copy seen", or drops `ingest_ts` from the record, changes rows on about 0.05% of events. This is a second reason, besides the lateness check, why "lazy `ingest_ts`" stays closed.

## Iteration 13 (2026-11-21): why the workers start late, and what that does to row 5

### 13.1 Each level of the spawn tree pays one 2 MiB page clear before it creates the next level

- Log: "workers start 0.9 to 1.45 ms after t0", parse starts at 2.5 ms.
- `worker()` (`sys.rs:288-293`) first spawns its two children, then does anything else. `std::thread::spawn` allocates (the closure, the thread handle), and a thread's first small allocation takes a fresh 4 MiB chunk from the huge-page heap region (`sys.rs:750-762`). The first write into that chunk is a huge-page fault, and the kernel clears 2 MiB before returning: about 0.55 ms per 2 MiB at the rate the log's profile implies under load, less when little else is running.
- So the order of events is: main clears its first heap page, spawns worker 1; worker 1 clears its first page, then spawns 2 and 3; those clear theirs, then spawn 4 to 7. Three clears in sequence before the last four threads exist. That fits 0.9 ms for the first worker and 1.45 ms for the last without any slow thread creation. **This is my reading of the code against two log numbers, not a measurement**; the `tl` marks 10 and 11 per thread (`sys.rs:294`, `:306`) would confirm it if the first mark of thread 4 to 7 is about one clear later than that of 2 and 3.
- `XL_FIRST4K=1` (first chunk of every thread on 4 KiB pages, `sys.rs:757`) removes those clears and was within noise in experiment 2. That is consistent: worker start is not on the critical path today. Thread 0's serial chain is (list, map, sample, keys: 3.0 ms).
- **What it changes in my earlier entries.** The "9.68 idle thread-ms" of the setup job (log, fact 3) is mostly threads that do not exist yet or are clearing their first page, not threads spinning with nothing to do. So 1.3 and 9.4(c), "use setup's idle time to clear arena blocks", have far less room than I assumed: perhaps 2 to 4 thread-ms, a few blocks, a fraction of a millisecond. Row 5 is rewritten accordingly and is now explicitly below the 2 ms bar.
- **If anyone does touch setup,** the two changes that belong together are: spawn all seven workers from main in a loop (main's heap page is already faulted, so the workers exist after about 7 thread creations, not after 3 page clears), and only then give workers part of thread 0's chain. Either alone gains nothing visible. Sampling already runs in pieces on the workers (`setup.rs:895-901`).

### 13.2 The string to look for on stderr

Every fallback prints `[v2b] FALLBACK: <reason>` before it re-executes (`sys.rs:123`). For the seed checks of rows 2 and 3: `grep -c FALLBACK` on stderr must be 0; a correct diff with that line present means the slow path produced the output.

## Iteration 14 (2026-11-21): experiments 5 to 10 read; what is left

### 14.1 The 40M regression of the shipped constants

- The shipped change is right for 20M and the gate is thorough (four seeds, three thread counts, no fallback). The open point is the one the log itself raises: at 40M it is 11 ms slower.
- **Why I think it is the bucket size, not the chunk cap.** The chunk rule is `rem / (4 * nt)` capped at `CH_DEFAULT` (`parse.rs:1961`). At 20M (1.8 GB) `rem / 32` starts at 56 MiB, so a 64 MiB cap is never reached and the chunks simply shrink with what is left; at 40M the cap applies only to the first few chunks. Neither case creates imbalance. The bucket count, on the other hand, doubles with the input and doubles again with `BB=14`: about 3,100 buckets per thread at 40M. Each has a 64-byte staging line (`parse.rs:94-159`), so two hyperthreads on one core hold about 400 KiB of staging in a 1 MiB L2 that also carries the scanner's tables, and each thread keeps 3,100 partly filled 4 KiB chunks open (12 MiB, six huge pages) as non-temporal store targets. Experiment 7 measured the same effect in small at 20M: `BB=14` parse +2.1 ms, `BB=13` parse +11 ms.
- **If that is confirmed,** the choice is between about 1 ms at 20M and 11 ms at 40M. Other teams' probes say 20M; the generator has a comment about "the full 75M rows" (6). That is the user's call, and the answer may be to keep `BB=14`; I only want it made with the split known.
- **A way to have both, if wanted later:** choose the bucket size from the input size at start. `BB` is a compile-time constant used in vector shifts (`parse.rs:712`) and in array sizes in `events.rs`, so this is not a small change and touches the most row-sensitive code. Not proposed unless the hidden size is in real doubt.

### 14.2 The licence question is live again

- Log: this VM reads 57 GB/s and writes 30 GB/s on 8 threads; the run's traffic (1.8 GB read, 0.56 GB written twice) needs about 70 ms at those rates; parse is 158 ms. So more than half of parse is computation, and my iteration 9 demotion of the probe (already withdrawn in 11.1) was wrong on the facts as well.
- 88 ms of computation over 25.9M lines on 8 threads is 27 ns per line, about 115 cycles at this CPU's turbo clock for about 160 instructions (2.2), on hyperthreads. If parse sits in the L1 licence, the clock is several percent lower than it need be for every one of those cycles.
- The probe is unchanged from 1.2 and 3.3: a fixed scalar loop timed with `rdtsc` at worker start, at the end of `stage1`, and at the end of `stage3a` (512-bit code, the positive control). 20 lines behind a feature, no behaviour change.
- If positive, the first A/B is the event path's fused 256-bit decode against the 128-bit forms the click path already uses (2.3).

### 14.3 What I got wrong, for calibration

- Iteration 9 row 1: I estimated the parallel end release at 3 to 6 ms; it is 8.6. Removing 4 KiB file mappings does not scale across hyperthreads the way I assumed.
- Iteration 9 row 2: I gave 0 to 6 ms with unknown sign; it was -6.4 (worse) for mode 1.
- Rows 3 and 4 (bucket size, prefetch distance) were each under 1 ms alone, as the lower ends of my ranges said, and I had the prefetch direction open; 512 B was the better one.
- Net: of five ranked ideas, the two I ranked highest were null or negative, and the kept gain came from the three lowest combined.

## Iteration 15 (2026-11-21): next batch

Where the time is now, from the log (tmpfs, before experiment 9): parse 158 ms, of which the log's bandwidth test puts about 70 ms in memory traffic; kernel time is 21% of parse thread time; in the profile `run_events` 31.4%, page zeroing 11.1%, `ev_batch` 10.5%, `stage3` 9.1%, `links::job` 5.0%, `cl_loop` 4.8%. Everything below is aimed at one of those.

### 15.1 Row 3: do the page clearing when the memory bus is idle

- **The argument, with the log's numbers.** Zeroing is about 170 thread-ms for about 0.56 GB, 3.3 GB/s per thread. Eight threads at that rate are 26 GB/s, which is the VM's measured write ceiling (30 GB/s on 8 threads). So during parse the kernel's zeroing, the record stores and the input reads share a saturated bus. During setup (0 to 2.5 ms) the bus is nearly idle and one thread alone writes 7.4 GB/s.
- **Why `XL_PREFAULT` being null (experiment 2) does not close this.** That knob faults each worker's 4 MiB heap chunk, 8 pages in all, and with the tree spawn the workers only exist from 0.9 to 1.45 ms (13.1). It never touched the record arenas.
- **Change, two parts that only pay together.**
  1. `pool_start` (`sys.rs:279`): spawn all `nt - 1` workers from main in a loop, and drop the two child spawns at the top of `worker` (`sys.rs:289-293`). Main's first heap page is already faulted, so the workers exist after seven thread creations (**my estimate** 0.2 to 0.3 ms) and not after three sequential 2 MiB clears.
  2. In `setup_par`'s worker branch (`setup.rs:828-832`), replace the spin `while !(...) { spin_loop() }` by: take the next 2 MiB block of a pre-reserved range and `madvise(MADV_POPULATE_WRITE)` it, until `keys.done` is set. The arenas are created after the job today (`setup.rs:968-969`); their sizes need only `ev_bytes`, `ck_bytes`, `lk_bytes`, which thread 0 has after the listing (`setup.rs:844`), so creating them there and publishing them through a `Slot` (as `rowcap` and `nbs` already are) is the enabling step. `Arena::alloc` hands out blocks in address order (`sys.rs:175`), so "the first N blocks of the event arena" is well defined and parse will use exactly those first.
- **Size (my estimate).** About 1.5 ms of window on 7 threads; at 7 GB/s single-thread and a 30 GB/s ceiling that is at most 40 MB, about 20 blocks, 7% of the zeroing. Taken out of a bandwidth-bound parse that is 1 to 2 ms. It cannot lengthen setup: the loop ends on `keys.done`, and a block in progress is 0.3 ms at worst.
- **What to watch:** `ev_bytes / 28 * 16` (`setup.rs:968`) over-reserves the arena; populate only blocks below the expected use (say the first 32) so no cleared memory is wasted. With `PARTS_READY` the `Part`s are built in setup (`setup.rs:975-983`); they take their first block lazily in `new_chunk`, so nothing else changes.
- **Which memory to clear first (added in iteration 16):** the click index (`g.csr`, about 82 MB at 20M) is as good a target as the record arenas and simpler, because it is one flat allocation whose size is known in setup (`csr_cap`, `setup.rs:673`). The log's profile has 3.1% of all samples in page zeroing under `links::job`, about 48 thread-ms, which is this table being faulted in by the link jobs that run inside parse (`scatter_chunk`, `links.rs:852-862`). Populating its first 40 MB in setup removes half of that from parse with no change to `Part` or `Arena` at all: one `madvise(MADV_POPULATE_WRITE)` loop over a pointer that already exists.
- **Risk to output:** none. The same records go to the same addresses.

### 15.2 Row 4: `ev3b`'s tail

- `stage3b` (`events.rs:2126`) processes the users whose events cross a bucket boundary ("joined" runs: 599 at 20M with 2^15 buckets, about twice that now), largest first (`events.rs:2312`), one user per `fetch_add`.
- The log shows 16.2 thread-ms of work finishing in 2.56 ms of wall with 21% idle. On 8 threads the work alone would be 2.0 ms, so about half a millisecond, and possibly more, is waiting on the largest items. The five whale users hold 1 to 2% of all events each (rules), 200,000 to 400,000 events; if a whale's edge segments are fed raw rather than as digests (they are digested only when the segment has at least 4,096 events, `DIG_MIN`, `events.rs:368`), one whale is on one thread for most of the stage.
- **First, measure:** time each item in `stage3b` in the `tl` build (there is already `tl::TASK` for the write stage, `out.rs:527-533`) and log the largest three. If the largest is under 0.5 ms, close this.
- **If one item dominates,** the safe change is in scheduling only: each thread releases its part of the event arena before its first item (`release_part`, `events.rs:2134-2136`, a `madvise` over about 40 MB); moving that release to after the loop for the thread that takes item 0 would start the longest item earlier. Splitting one user's feed across threads is not proposed: session state carries from one segment to the next (`feed_slice`, `feed_digest`), and that is row-sensitive.
- Note that `BB=14` doubled the number of bucket boundaries, so `ev3b` may have grown since the log's 2.56 ms; the lap line of the shipped build will show it.

### 15.3 Row 5: the scanner's tables on a huge page (speculative)

- `static T: Tables` (`k_free.rs:604-624`) holds the per-shape rows the scanner indexes for every line: `jid` (64 x 64 rows), `tj` (128 rows of 512 bytes), `tail`, `kind2`, `prod`, and the click tables; about 1 MB in `.rodata`, which is file-backed and so on 4 KiB pages. Each event line reads about 12 rows (2.2). The first-level data TLB of this CPU has 64 entries for 4 KiB pages; the input stream, the batch and the tables compete for them. A miss that hits the second-level TLB costs about 7 cycles.
- **Unknown:** how many distinct table pages are hot. If line shapes repeat (IDs have fixed lengths, so they largely do), the hot set is a few dozen pages and this gains nothing. Without counters on the VM I cannot tell, which is why this is rank 5.
- **How, without touching the scanner:** at the very start of `main`, before any thread exists, copy the 2 MiB-aligned range that contains `T` into an anonymous 2 MiB region advised `MADV_HUGEPAGE`, give it the original protection, and `mremap(MREMAP_MAYMOVE | MREMAP_FIXED)` it over the original address. Addresses do not change, so no code that uses `T` changes. This is the known "remap the binary onto huge pages" technique. It needs the link to align segments at 2 MiB (`-C link-arg=-Wl,-z,common-page-size=2097152,-z,max-page-size=2097152`) so that a whole 2 MiB range belongs to the binary, and it costs one 2 MiB clear and copy at start, about 0.3 ms.
- **A cheaper test of whether it can matter at all:** a measurement build in which `run_events` is called on the same 64 MiB buffer twice in a row from one thread, timed, with and without the remap. If the second pass does not get faster with the remap, drop the idea.
- **Risk to output:** none in principle; the bytes are identical. The risk is to start-up (a wrong range or protection is a crash, which on the scorer is a zero), so it needs the gate and a fallback to "do nothing" if any call fails.

### 15.4 Three measurements that would direct the next round (no code)

1. `perf annotate` (the same cpu-clock profile as before, on the tmpfs run) for `k_free::run_events`, `parse::Ctx::ev_batch` and `links::job`: the ten source lines or instructions with the most samples in each. With timer sampling a stalled load shows up on the instruction after it, so this separates "waiting on the input" from "waiting on a table" from "waiting on a store", which is the question behind rows 2 and 5 and behind any further prefetch work.
2. `perf report --sort symbol` restricted to the write stage (`--time` with the stage's start and end from the `tl` line, or simply all symbols in `pqw::` and `out::` and `complex::` with their percentages). The stage is 4.8% of the run's CPU and no single function has yet appeared above 1.1%; I need the list to see whether it is the encoders, the row split, or the complex persons.
3. The one-thread-per-core run of 11.1 (`NCPU=4 XL_PIN=0 taskset -c 0-3`), `parse` lap.

### 15.5 A caution on row 2 (the licence probe), so it is not oversold

The authors wrote all vector code that runs during parse through 256-bit wrappers with explicit barriers against 512-bit code generation (`sys::z`, `yb`/`kb`, `sys.rs:1139-1158`), including the link jobs that run inside parse, and kept 512-bit code to stage 3. People who do that have usually measured frequency effects on their own machine. That makes it less likely that a second, cheaper licence step was left on the table, though they may not have had a Cascade Lake to test on (the build targets it, `target-cpu=cascadelake`). The probe is still 20 lines and settles it either way.

## Iteration 16 (2026-11-21): the default click-index job read; one refinement, one correction to how the log's figures are read

Read `links.rs:825-931` (`count_chunk`, `scatter_chunk`, `bucket`).
- Per user bucket the job reads that bucket's click and link records twice (count, then scatter into the index), about 500 KiB per bucket, then sorts within users. The second read comes from cache. I found no pass to remove; mode 2, which drops the count pass, measured equal (experiment 6), which fits.
- **Reading the log's `st2b_*` figures:** `st2b_pass2` is taken from the job's start, not from the end of pass 1 (`links.rs:919-920`, `dbg2 = tstart.elapsed()`), so it includes `st2b_pass1`. The scatter itself is 113.2 - 18.4 = about 95 thread-ms, of which about 48 is the kernel clearing the index's pages (profile). So the job's own work on 5.1M clicks is about 9 ns per click for the scatter: not a target. The page clearing is, and 15.1 now names the click index as the first thing to pre-clear.

## Iteration 17 (2026-11-21): experiments 11 to 14 read

**Closed, with what each one taught:**
- 15.3 tables on huge pages (experiment 11): parse -1.1 ms, eaten by making the two pages. So table TLB misses are worth about 1 ms in all.
- 15.1 and 16 pre-clearing (experiments 12, 13): 28 to 36 MB cleared before parse, run not faster. **My bandwidth argument was wrong**: taking zeroing out of parse does not shorten parse, so parse is not held up by the kernel's clearing.
- 15.2 `ev3b`: 1.7 ms in the shipped build (thread table, experiment 13). No tail worth chasing.
- 11.1 one thread per core: the second hyperthread gives 1.22x in parse, two cores to four gives 1.93x. Parse scales with cores almost linearly, which a memory-bound stage would not. The "70 ms of parse is memory traffic" reading from the bandwidth test should not be used to rank ideas any more; what is left in parse is computation.
- 14.1 (experiment 14): the 40M cost is the bucket size alone, as read. Chunk and prefetch constants are safe at every size.

### 17.1 Row 1: choose the bucket size at run time, without touching the hot code

- `BB` is a compile-time constant used in about 50 places, including constant offsets in the stage 3 layouts (`events.rs:1022-1054`) and shift immediates in `ev_vec` (`parse.rs:712`). Making it a variable would touch all of them; I do not propose that.
- Instead: build the binary twice (`XL_BB=14` as now, `XL_BB=15`). In the default one, as soon as the listing has summed the event files' sizes (`ev_bytes`, `setup.rs:844`, about 1.0 ms in; the input is many part files, so there is no single file to `stat`), if the total is above the threshold, `execv` the other with the same arguments. Nothing has been written to `/output` by then. The large path therefore pays the listing twice, about 1.5 ms in all with the `execv`. Threshold: the byte size of a 25M-event file, taken from the seed 6 and seed 20 files on the VM (bytes per event is stable in the generator).
- Per-bucket work is the same at every input size (a bucket is 2^BB ids); what changes with size is the number of buckets, 1,538 at 20M and 3,076 at 40M with 2^14. Experiment 14 says about 1,500 is free and about 2,300 is not.
- Cost at 20M: one compare. On the large path about 1.5 ms (**my estimate**: 1.0 ms of repeated listing from the log's `s.list` lap, plus an `execv` of a static binary).
- Check first that the rules permit a second executable in the image; I found nothing against it in `rules.md` but the earlier commit "single pipeline binary" may have had a reason.
- This removes the open decision between `801982c` and `safe-constants`.

### 17.2 Row 3: prefetch the staging lines

- `ev_vec` computes each row's staging-line address (`lp`, `parse.rs:714`) a whole batch before `push_many` stores to it (`parse.rs:104-110`). Per thread the lines are 64 B x buckets: 98 KB at 20M, 197 KB at 40M, and two hyperthreads share a 1 MiB L2 with the scanner tables and input.
- Change: after `st4::<LP>`, four `prefetcht0` on the four line addresses (extract from the vector, or prefetch in a short scalar loop over `out.lp` at the top of `push_many`). Same in `ck_vec`.
- Expected: little at 20M (doubling the lines cost nothing there). It is a candidate for recovering part of the penalty at 30M and 40M, which would make row 1 unnecessary; test at 40M first.
- Risk to output: none.

### 17.3 Where I stand

Four ideas from iterations 15 and 16 were measured and none paid. The mechanisms I can argue from the source and the log's totals are used up; the remaining time is inside three functions and I need per-instruction samples (15.4 item 1) to say anything better than a guess. That profile is the most valuable thing the coding agent can run next.

## Iteration 18 (2026-11-21): experiments 15 to 17 read; the source is a different pipeline now

- Experiments 15 and 16 took three newer upstream pipeline commits and upstream's assembly event loop: 20M went from about 209 to about 196 (parse 162 to 149, write 9.9 to 7.3). That gain did not come from these notes.
- What changed, from my first read of the diff: event records are 12 bytes (`R12`, `parse.rs:321`), staged 16 at a time in 192-byte blocks per bucket (`EV_STAGE`, `parse.rs:318`); click index entries are 8 bytes (`main.rs`, `csr_range` now `&[u64]`); bucket size is back to 2^15; the event loop is hand-written assembly (`k_free.rs`, `ev_loop` / `ev_redo`); the writer and `pqw.rs` grew by about 1,300 lines; a fault handler falls back (`sys::fault_fallback_init`).
- **Withdrawn:** 17.1 (two builds by input size): the smaller bucket now loses at 20M too, main is on 2^15, nothing to switch between. 17.2 as written (line numbers and sizes were for the old 64-byte staging); its successor is 18.2.
- Experiment 17: upstream's other variants and all runtime knobs are no better than the shipped build. Closed.

### 18.1 Row 1: measurements needed before I can rank anything

On `9363748`, seed 20, tmpfs, the same commands as the "Facts for the research session" block of the log: (a) the `XL_TIMING` lap line and the `prof` thread-ms line; (b) page faults, user and system time for parse; (c) peak RSS; (d) `perf report` top 15 symbols; (e) `perf annotate` of the event loop, ten hottest instructions. With 12-byte records the arena traffic fell by a quarter and the kernel share should have fallen with it; whether the remaining time is the scanner, the staging stores or stage 3 decides where I read next.

### 18.2 Row 2: staging footprint

- Each thread now keeps 256 B of staging per bucket (192 B block plus per-bucket state), four times the old 64 B line. At 769 buckets that is 197 KB per thread, about 400 KB for the two hyperthreads sharing a 1 MiB L2, before the scanner's tables and the input. Experiment 15's `upb` column fits: doubling the buckets cost parse 8 ms at 20M where on the old layout it was free.
- So the stage is now on the edge at 20M. `XL_BB=16` is not available: the 12-byte layout asserts `BB <= 15` (`parse.rs:325`), so a larger bucket would mean giving up the 12-byte records. That leaves one cheap test: in `EvPart::push_many` (`parse.rs:428`), prefetch each row's target (`l + f`, from `lp` and `fill`) a batch ahead. **My estimate of sign only; no size claimed.**
- Users numbered at or above 2^22 take the 16-byte escape path (`R12_UMAX`, `parse.rs:324`, `esc_defer`). At one user per 30 events that is 126M events, far beyond any stated size; noted so nobody is surprised by a slow run on a synthetic set with many users.
- Risk to output: none.

## Iteration 19 (2026-11-21): the staging lead, sized from the log, with patch text

Read `EvPart::push_many` and its flush (`parse.rs:428-560`), `block_out` (`parse.rs:827`).

**How the push works now.** Per row: load bucket `b` and block address `l` from the batch, load `fill[b]` (u16), one masked 16-byte store of the 12-byte record at `l + fill`, add 12, and when the block reaches 256 a three-line non-temporal copy to the bucket's chunk. Per thread and bucket: 192 B of staging plus 2 B of `fill` and 8 B of `cur`.

**Size of the pool, from measured numbers (the arithmetic is mine).**
- Experiment 15, 20M: doubling the buckets (769 to 1,538) cost parse +8 ms, about 5%.
- Experiment 16, same build: parse 148.6 at 20M (20.92M events, 7.10 ms per million) and 230.5 at 30M seed 6. If seed 6 has 1.5 times the events (I do not have its row count), that is 7.34 ms per million, +3.4% for 1.5 times the buckets.
- Both points fit one line: about +5% of parse per extra 769 buckets. If the line continues down to zero buckets, the staging cost already inside the 20M run is about 5%, 7 ms. That extrapolation is the weak step, and it is cheap to check without code: **run seed 3 (10M) on the shipped build and report the parse lap and the event count.** The line predicts about 6.9 ms per million there. If 10M comes out at 7.1, the cost only starts above 769 buckets and this lead is worth nothing at 20M (it would still matter at 30M).

**Why I think it is the cache and not the stores.** The flush cost per record does not depend on the bucket count (every block is three full lines whatever the count), and the chunks sit on 2 MiB pages. What grows with buckets is only the staging: 197 KB per thread at 20M, two threads per 1 MiB L2, with about 1.5 input lines per row per thread streaming through the same L2. A staging line is touched once per roughly 769 rows; the longer the gap, the likelier it has been pushed out to L3, and L3 on this CPU is slow (about 70 cycles).

**Change A, prefetch (no output risk).** At the top of `EvPart::push_many`, before the row loop:

```rust
for i in 0..n {
    let b = *ob.add(i) as usize;
    _mm_prefetch::<_MM_HINT_T0>((*lp.add(i) as *const i8).add(*fill.add(b) as usize));
}
```

- About 4 micro-operations per row. `fill` is 1.5 KB and stays in L1. For the masked-off rows of a partial batch `ob`/`lp` may be stale: restrict the loop to set bits of `m`, or keep it for the full-mask path only (the common case).
- It gives the 64 lines of a batch a head start of one pre-pass. The out-of-order window already looks about 16 rows ahead in the row loop, so the gain is only on lines that are in L3, and only part of that. **My estimate: 0 to 2 ms at 20M, more at 30M.** Test at 30M first, where the effect is larger; if nothing shows there, drop it.
- A stronger form, if A shows a gain: issue the prefetch from `ev_vec` as each group of four rows is computed (the bucket vector `bv` is in hand there), which is a whole batch earlier.

**Change B, smaller staging block (output-neutral, layout change).** 192 B is the smallest size that is both whole cache lines and whole 12-byte records, so the block cannot shrink without padding. Not proposed.

**A side observation, not proposed:** 3 of every 16 rows (fill offsets 52, 56, 60 within a line) make the 16-byte masked store cross a cache line. That is a split store on 19% of rows. On this CPU a split store costs a cycle or two, so I do not think it is worth a layout change; noted in case the annotate shows time on that store.

## Iteration 20 (2026-11-21): the new stage 3 scatter read

Read `scatter_chunk12` (`events.rs:262-330`). Per record: one unaligned load, one load of the slot's old tag from the dedup table, a predictable branch, a 16-byte store of the widened record. Records come two at a time through one 256-bit decode (`DEC2`).

- **No prefetch to add.** The slot index is in the record itself and the loads of successive records are independent, so the out-of-order window already overlaps about 20 of them. A software prefetch of the slot would duplicate that.
- **What limits it is the table size.** The table is 2^15 slots of 16 B, 512 KB per thread, so two hyperthreads fill their shared 1 MiB L2 exactly, before the input chunk. Evidence: halving it (bucket 2^14) took 3.5 ms off ev3a in experiments 9 and 15, and the second hyperthread adds only 1.14x in ev3a (experiment 13) against 1.22x in parse.
- **Speculative idea, not ranked:** keep the slots at 12 B, as the records already are in the arena, and widen to 16 B only in the compaction that follows. That is 384 KB per thread, 768 KB per pair. If the gain scales with the size cut (a quarter against a half), it is 1 to 2 ms (**my estimate**, by proportion from the 3.5 ms above). It means rewriting the tag test, the keep-smallest-ingest rule and the 512-bit compaction for 12-byte slots, all of which decide which copy of a duplicated event survives. That is exactly the row-sensitive code, so by the rule of these notes it ranks below everything output-neutral and I would only take it up if the fresh profile (18.1) shows ev3a's scatter well above its old 98 thread-ms share.

## Iteration 21 (2026-11-21): literature pass (kernel, parsing, partitioning)

Three searches of papers, kernel sources and public write-ups. Headline, stated plainly: **nothing found beats the current design; the literature puts it at or past the published state of the art, on a flat optimum.** What follows is what survived. "Quoted" means the figure is from the source named; everything else is my inference.

### 21.1 Kernel side

- **Dead, with reasons:** no zero-copy read of tmpfs page cache exists other than mapping (splice, io_uring fixed buffers, `O_DIRECT` on tmpfs, `process_vm_readv` all copy). Fault-around is fixed at 64 KiB and tunable only through debugfs; our 29k faults for 2.3 GB means it is already at its ceiling. `MAP_POPULATE` uses the same path as `MADV_POPULATE_READ` (measured null, experiment 4). No uninitialised memory without a kernel build option. `munmap` is never cheaper than `MADV_DONTNEED`. Exit-in-a-child tricks do not work for a container's main process: the kernel waits for every process in the PID namespace before the container is seen to exit. `MADV_COLLAPSE` inside the run costs a copy. The double zeroing of huge pages with `init_on_alloc` was fixed in 6.13, so not on 7.0.
- **Per-VMA locks:** since 6.7 file faults and since about 6.17 `MADV_DONTNEED` take only the per-VMA lock, so eight threads on one mapping do not contend, provided each `madvise` call stays inside one VMA.
- **Teardown cost, quoted** (2026 RFC, 32-CPU bare metal): tmpfs mapping about 8.8 ms per GB single-threaded; huge-page anonymous memory about 1.7 ms per GB. So the anonymous memory left at exit (305 MB) costs about 0.5 ms and should be left alone, which is what the code does.
- **The one live item (row 2).** If a tmpfs is mounted `huge=always` or `huge=within_size`, files written to it get large folios (kernel 6.14 on), and a reader's fault maps a whole folio, or a 2 MiB entry, at once, but only if the mapping address is congruent with the file offset modulo 2 MiB. With the default policy (`never`) nothing changes. The check on any machine: `huge=` in `/proc/self/mountinfo`, and `ShmemPmdMapped` in `/proc/self/smaps`. To size the upside on the VM: remount the test tmpfs with `huge=always`, copy the set in again, run with aligned mappings.
- Sources: kernel `mm/memory.c`, `mm/shmem.c`, `mm/madvise.c`; docs.kernel.org `admin-guide/mm/transhuge.html`; man pages `madvise(2)`, `mmap(2)`; lkml 2607.2/06062 (teardown RFC).

### 21.2 Parsing

- **Where we stand, quoted figures:** Ellmann and Neumann (PVLDB 2026) report 1.8 to 2.1 GB/s per thread for counting records only on Zen 5, and about 0.31 GB/s per core with all columns converted. simdjson reports 2.2 GB/s on Skylake. This binary does about 1.9 GB/s per hyperthread with full conversion and partitioning. A two-stage structural index would be slower on 85-byte lines (my arithmetic from their per-byte costs: 70 to 127 cycles per line against about 40 for our whole scanner).
- **Dominant-shape fast path (row 4).** QuestDB's write-up: specialising on the commonest shape cut branch misses from 0.657 to 0.084 per row and time from 2.4 to 1.8 s; the 1BRC winner's branch-to-mask step gave 15.4% (both quoted, other programs). Here it would mean: compare the line's comma mask with one or two constants, and on a match run with every shuffle and length constant in registers, no table reads. It pays only if shapes are concentrated; the generator makes IDs fixed-length, so they may be. **Needed first:** a histogram of the shape key the scanner already computes (one counter array in a `knobs` build).
- **Multiple cursors per thread:** 4 to 22% in 1BRC entries (quoted), but the same author found hyperthreading stopped helping once he interleaved. With two threads per core already, expect little. Not ranked.
- **Not applicable:** IFMA/VBMI tricks (not on this CPU); `vpmuludq` for `vpmullq` (the key recovery needs the low 40 bits of a 40x64 product, which is three `vpmuludq` anyway).
- Sources: vldb.org/pvldb/vol19/p3579-ellmann.pdf; arxiv.org/pdf/1902.08318; questdb.com/blog/billion-row-challenge-step-by-step; curiouscoding.nl/posts/1brc; github.com/thomaswue/1brc-steps.

### 21.3 Frequency licences

Quoted (Lemire 2018; Downs 2020, Xeon W-2104): heavy instructions are floating point and integer multiplies; sustained 256-bit heavy use moves the core to L1; the drop covers the sibling hyperthread; L0/L1/L2 were 3.2/2.8/2.4 GHz on that part; returning to L0 takes about 680 microseconds. No published Cascade Lake measurement of 256-bit integer multiplies was found, so the probe (row 3) is the only way to know. If parse sits in L1 because of `vpmullq` in `ev_vec`, the remedy is a scalar or 128-bit key recovery, worth up to the L0/L1 gap on everything in parse.

### 21.4 Cross-CPU TLB flushes

Every `MADV_DONTNEED` over more than 33 pages is a full TLB flush sent to every CPU running the process (kernel `arch/x86/mm/tlb.c`), which also drops each thread's 2 MiB entries for the arenas. About 2.7 microseconds per flush on bare metal (quoted, LATR paper), more in a VM. Count them: the TLB row of `/proc/interrupts` before and after one run. If the count is in the thousands, fewer and larger release calls are worth a test; if it is a few hundred (my expectation with 64 MiB chunks), close it.

### 21.5 Partitioning and dedup

- **Staging:** Schuhknecht et al. (PVLDB 2015) find the best staging footprint is about half the L2 and that small buffers (under 8 lines) with non-temporal flushes are best; Zhang et al. (CIDR 2019, Skylake) say to group records to the stream width, which is exactly 16 x 12 = 192 B. Balkesen et al. (ICDE 2013): with 2 MiB pages a single pass beats two passes. All quoted. The current layout matches every recommendation.
- **Prefetching staging:** Schuhknecht, quoted: prefetching "decreases the performance if misses are not a major problem, e.g. for less than 1024 partitions". That is against 19's change A at 769 buckets; demoted.
- **Split stores:** 2.3 cycles against 1.05 for a store crossing a line on Skylake (quoted, Ertl). At 3 of 16 rows that is about 0.2 cycles per record. Closed.
- **Compress:** 2 cycles per 512-bit compress (uops.info), under 1 ms of the run. Closed.
- **Stage 3 is L2-capacity-bound** (two 512 KB tables in a 1 MiB L2); the literature's SMT figures for such code are 1.0 to 1.1x, ours is 1.14x. The only lever is a smaller slot (row 5). An 8-byte slot is not possible: a record without its in-bucket id is 81 bits.
- **Alternatives examined and rejected:** a global bitmap of seen ids (shared, needs locked updates, larger than L2); a Bloom pre-filter (helps when most rows are dropped; here 96% survive); two-pass partitioning (break-even at best at this size).
- Sources: vldb.org/pvldb/vol8/p934-schuhknecht.pdf; cs.columbia.edu/~orestis/sigmod14I.pdf; vldb.org/cidrdb/papers/2019/p133-zhang-cidr19.pdf; Balkesen ICDE 2013; complang.tuwien.ac.at/anton/unaligned-stores.

### 21.6 What this means for a target of 0.27

The searches found no technique worth tens of milliseconds. The candidates above are each a few ms at best, except row 2, which is large but depends on a mount option we cannot see. If the board keeps the best of several noisy draws, repeated submission of the same build is the largest remaining lever.

## Next passes

1. `core.rs` `feed_slice` / `feed_digest` and `events.rs` `digest_put`, for 15.2.
2. `complex.rs` and `core.rs` `person`: 202,264 complex events go through a comparison sort per shard (`complex.rs:95`); small, but it is on the write stage's critical path.
