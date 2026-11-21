# Plan: beat 0.63 s (Tark305)

Our best board score is 1.29 s (`6c300bd`). The board is about 1.5–1.7× slower than local wall time on the dev laptop and tracks **total CPU-seconds (user + sys)** more than wall time (Eagle Eye's four scored points). To reach 0.63 s on the board we need roughly **0.35–0.40 s local wall at 8 threads on d20**, with CPU around 2.5 s. Today we are at about 0.75 s local, 5.0 s user + 1.5 s sys.

The strongest public reference (cherylaurelia/builders-hackathon) measures 0.35–0.41 s native and 0.77 s in Docker at 20M events, with 2.2 s user and 0.56 s sys. Their design is the model.

## Current pipeline (src/main.rs)

| Phase | What | d20, 8 threads |
|---|---|---|
| 1 | links: parse, shard intern, union-find, min-id name (`build_links`), concurrent with phase 2 | ~160 ms (overlapped) |
| 2 | parse events -> 32 B `ERec` into 512 eid partitions; clicks -> 32 B `CRec` into 512 uid partitions (`parse_events`, `parse_clicks`) | ~400 ms |
| 3 | per eid partition: bitmap prefilter + hash dedup, survivors resolved user->person through a prefetch ring and scattered as 16 B `SRec` into 512 person partitions (`dedup_partition`) | ~140 ms |
| 3b | clicks user->person (`resolve_partition`) | ~30 ms |
| 4 | per person partition: hash grouping, counting scatter, sort by ts, sessionize, attribution (`process_partition`) | ~150 ms |
| 5 | Parquet writer (`out.rs`, `pq.rs`) | ~40 ms |

Profile (perf, d20): kernel page zeroing `clear_page_erms` ~21–25%, parse closures ~27%, dedup ~18%, process_partition ~10% + sorts ~4%.

Switches (env): `ONESHUF=1` experimental one-shuffle path (links first, events routed to person partitions at parse, `dedup_mark`); `WC=0/1` write-combining scatters (default 1); `IOMODE=3` mmap+populate / `5` cache-resident pread blocks (default 5); `TSBL=1`; `P1BITS`/`P2BITS`; `BENCH=2` single-thread per-line microbench.

## Research references (read-only clones, Windows)

`C:\Users\saran\dcwork\research\` — `cheryl` (fastest, single `src/main.rs`), `vibhor` (board 0.82 s), `micah`, `th3r0n`, `explorer`, `joshua`, `eagle`. Learn techniques and reimplement them in our code; do not paste their files.

## Work packages (one agent each, separate branches)

- **A — Phase 4 + output.** Owns `process_partition`, `Scratch`, `out.rs`, `pq.rs`. Target: phase 4 + output from ~190 ms to ≤ 80 ms (8 threads, d20). Ideas: counting passes over 30-minute bins instead of sorts (vibhor `person.rs`), radix sort on (person, ts) (micah), cheryl `partition_work`/`person_work`/`ranks_small`/`win_mask`, AVX2 click window, workers writing their own `user_daily` file as they finish, one `write(2)` per file.
- **B — Parse inner loop.** Owns the field-extraction code inside `parse_events`/`parse_clicks`, plus `parse.rs` and `keys.rs` (scanner, `pack`/`pack_fast`, timestamps, price, catalog lookups). Must not change `ERec`/`CRec`/`SRec` definitions or the push calls. Target: `BENCH=2` `full` from ~73 to ≤ 45 ns/line, `cl full` from ~59 to ≤ 35. Ideas: pshufb + PEXT ID packing (cheryl `pack_at`/`pack2_at`, micah `pack16`), date-memo timestamps (vibhor `ts.rs`, cheryl `ts_memo`), branch-free format selection, cheryl `fast_split`. Every fast path needs a fuzz test against the scalar reference.
- **C — Dedup, scatter and memory.** Owns `dedup_partition`, `mem.rs`, the arena, record layouts (`ERec`, `CRec`), and process exit. Target: dedup ≤ 60 ms; sys time halved; fewer fresh pages. Ideas: cheryl cache-line tag table (`DLine`, AVX2 compare), partitions sized to L2 (~8k copies), smaller records (Eagle side vector for purchases), reuse dead blocks, zero-page trap, mimalloc options, `_exit`, early input release.
- **D — Links and the one-shuffle architecture.** Owns `build_links`, `Flat`, `parse_events1`, `parse_clicks1`, `dedup_mark`, the `ONESHUF` branch in `main`. Target: links ≤ 25 ms at 8 threads (lock-free CAS union-find: cheryl `IdTable`/`build_links`, vibhor `links.rs`), cheaper person lookups (dense slot table, prefetch home and home+3), and make `ONESHUF=1` beat the default end to end. Keep `SRec { person: Key, ts, pc }` unchanged so package A's code still fits.

## Rules for every agent

1. **Correctness first.** Gate with `wsl -e bash -c "~/agate.sh <NAME> '<your worktree path>' <CPUS>"`: cargo tests, 36 fixtures, dev answer at NCPU 1 and 2, d5 reference at NCPU 2. All must pass before you commit. Dedup is global on event_id, before validation; the earliest-ingest copy wins even if invalid. Read `rules.md`.
2. **Measure on your own CPUs only.** A: 10-11, B: 12-13, C: 14-15, D: 16-17. CPUs 0-7 are reserved for the integrator's 8-thread benchmarks; 18-19 spare. Use `~/amb.sh BIN_A BIN_B <CPU> 4 d5` (single-thread whole pipeline, interleaved) or `BENCH=2 ... taskset -c <CPU>` for parse. Baseline binary: `~/bin_base` (= scored 6c300bd). Never run d20 full pipelines (memory is 15 GB shared); `BENCH=2` on d20 is fine.
3. **Small, verified commits** on your branch, each with the measured before/after in the message. Leave anything you could not prove faster behind an env switch, default off.
4. **Report** at the end: what changed, measured gains (single-thread ms or ns/line, interleaved A/B), what failed, and any correctness risk.
