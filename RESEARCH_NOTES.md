# RESEARCH_NOTES.md

Research-only notes for the coding agent, restarted on 2026-11-21 for the new codebase (`7ed9b7b` "overhaul" and after). I do not build or run anything: every number below is either quoted from a named source or marked **my estimate**. Line numbers are `src/` at `57ae37c`.

The notes for the old codebase are at `git show 3a5f601:RESEARCH_NOTES.md`. What still holds from them is carried into "Scorer facts" and "Closed" below; the rest is obsolete.

## Top five untried (ranked by expected gain per unit of risk to exact output)

| # | Idea | Where | Expected gain | Rests on | Risk to exact output | Entry |
|---|---|---|---|---|---|---|
| 1 | Profile-guided build, with three traps handled (profile never written because of `_exit`; `RUSTFLAGS` wiping `target-cpu`; unpinned toolchain) | `Dockerfile`, `.cargo/config.toml`, `main.rs:539`, `sys.rs:122` | 0 to 4% of in-process time | Quoted: Tejas -5%, joch44 -2.7%/-4.1% (`FOR_NIMAI.md`); this repo's old code -1.5 to -2.7% (old log, experiments 33, 42). Lower end likely here: hot paths are already hand-laid-out | None from PGO itself; the code has aliasing patterns that a different inlining could expose, so the full gate at 1, 2 and 8 threads decides | 1.1 |
| 2 | Find out which AVX frequency licence the cores hold during parse (one probe, no behaviour change); if it is L1, move the few 256-bit multiplies to 128-bit or scalar | probe: `parse.rs:2051` `stage1`; multiplies: `parse.rs:259-315`, `:642`, `:750`, `:889`, `:1110`; `k_free.rs:875` | Probe: none. If L1 is held and can be avoided: up to about 10% of parse clock rate, minus the extra instructions; **my estimate** net 0 to 6% of parse | Quoted: Skylake-SP runs 256-bit integer multiplies under licence L1 (Lemire 2018); W-2104 L0 3.2 GHz, L1 2.8 GHz (Downs 2020). Not measured for this code or the scorer | Probe: none. Rewrite: low, `ev_vec_matches_scalar`, `ck_vec_matches_scalar`, `b36_vec4_matches_scalar` already pin the results | 1.2 |
| 3 | Fault the arena's first blocks on the workers that sit idle during setup | `setup.rs:826-833` (worker wait loop), `:968-969` (arenas made after the job), `parse.rs:180` `new_chunk` | **My estimate** 1 to 2 ms of about 210 (0.5 to 1%); needs the `tl` idle figure for job 0 to size it | Quoted: old experiments 47/48, arena pre-fault on idle threads -1.4 to -4.3% on Intel (old code); Th3r0n "storing into fresh memory is what costs" | None: same bytes written to the same places | 1.3 |
| 4 | A/B the input mapping on a tmpfs `/data` (the scorer's case): default map, `XL_RD=128`, `XL_REL=0`, and one new variant, `MADV_POPULATE_READ` per chunk | `parse.rs:2140-2164`, `:1805` `fault_around_check` | Unknown sign, **my estimate** within ±1.5% of parse; worth one same-run A/B only because the test VM's `/data` may not be tmpfs | **My estimate** of kernel time in the map path: about 7% of parse thread time (1.4) | None: read-only input mapping | 1.4 |
| 5 | Start writeback of each output file as soon as it is written (`sync_file_range`, no wait) | `pqw.rs:1087` `UserFile::finish`, `:1111` `write_file` | 0 on a fast disk; on a slow one this repo measured post-exit 0.13 to 0.04 s at 20M (old experiment 39, old code, other runner) | Quoted: old log experiment 39; old notes 29.4 (about 1 ms per MB written after exit on that runner) | None: same bytes, same files | 1.5 |

Not in the table because they are not code changes: the board-noise note (1.6) and the bench-fidelity checklist (1.7). Read both before trusting any A/B.

## Requests to the coding agent (so the next ranking rests on data, not estimates)

Please put these in `OVERNIGHT_LOG.md` once, for the 20M set on the Intel VM:

1. The VM: CPU model (`/proc/cpuinfo` model name), kernel, `cat /sys/kernel/mm/transparent_hugepage/enabled`, the filesystem `/data` and `/output` are on, and the `docker run` flags used.
2. The `XL_TIMING=1` line (`[v2b] ms: ...`) and the `[v2b] rows:` line.
3. One run of a `--features tl` build: the `[tl]` lap lines (serial ms and idle thread-ms per job), "worker start/ready", and the write-syscall line.
4. One run of a `--features pmu` build: the `[pmu] parse:` line (instructions, cycles, faults, `task_ms`, `stime_ms`). Instructions per line and system time during parse decide between ideas 2, 3 and 4.
5. Output size in MB, and the time from the last stderr line to `docker run` returning.

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

## Next passes (in this order)

1. `k_free.rs` hot loop (`ev_s`, `ev_d`, `run_events`, click loop) line by line: instructions and loads per line, table footprint (`T` is about 1 MB of `.rodata` on 4 KiB pages), what is recomputed per line.
2. `events.rs` `stage3a` and `core.rs`: the 35 ms stage, pass by pass.
3. `links.rs`: what runs serially after parse (`odd_events`, deferred sorts, `uf_tail`).
4. `pqw.rs` encoders and output size.
5. Outside material on each: first-touch and huge-page clearing cost in the 7.0 kernel, tmpfs large folios, `MADV_POPULATE_*` behaviour.
