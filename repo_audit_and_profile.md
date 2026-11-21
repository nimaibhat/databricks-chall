# Repo audit and profile: Rust Shopping Sessions pipeline

Scope: local build, measurement and code reading of `/home/claude/work/repo` (HEAD 3e5cebd). All numbers below were measured in this sandbox on 2026-11-20 unless marked as inference.

Source key (local, no URLs exist for these):
- [main.rs](../../src/main.rs), [parse.rs](../../src/parse.rs), [keys.rs](../../src/keys.rs), [mem.rs](../../src/mem.rs), [out.rs](../../src/out.rs), [pq.rs](../../src/pq.rs), [rules.md](../../rules.md), [Dockerfile](../../Dockerfile)
- [M] = measurement in this sandbox (binary stderr, getrusage, /proc/self/status)
- [CG] = valgrind callgrind with cache simulation on the 1M dev set, 1 thread, debug-info build of an instrumented copy
- Instrumented copy: `/home/claude/work/repo_copy` (adds BENCH=3, STATS=1, /proc status dump, and the SWAR fix). Nothing under `repo/src` was changed; nothing was committed.
- Fix diff with tests: `/home/claude/work/swar_fix_and_tests.diff`
- Datasets: `/home/claude/work/data/dev` (seed 1, 1M events), `d20` (seed 7, 20M events, 5M clicks), `d40` (seed 11, 40M events, 10M clicks)

Sandbox caveat for every timing: 2 vCPUs (Xeon Sapphire Rapids class, 2.1 GHz, 1 thread per core, 2 MB L2 per core), ~6.5 GB memory cgroup, THP = madvise, no `perf`, no `/usr/bin/time`. Run-to-run noise on the 20M set is about +/-0.4 s on a ~3.5 s run, so single-run differences under ~10% are not significant. NCPU=4 and NCPU=8 runs are oversubscribed on 2 cores and only show overhead, not scaling.

## 1. Does it build, and what are the per-phase timings and peak RSS?

### Takeaway
It builds cleanly with rustc 1.97.0 (4 dead-code warnings) and passes 36/36 fixtures and the dev answer. At 20M events on 2 cores it runs in about 3.5 s wall; parse is 50% of wall, phase 4 about 16%, output 6 to 11%, dedup 10%, resolve 8%, links 5%. CPU cost is linear in events (0.33 to 0.37 CPU-microseconds per event from 1M to 40M).

### Cited Findings
- Build: `cargo build --release` with `RUSTFLAGS="-C target-cpu=x86-64-v3"` finishes in ~14 to 22 s; warnings only for unused `prefetch`, `masks64_pub`, and two others. Without the RUSTFLAGS the AVX2 `masks64` is compiled out and the scalar 64-iteration fallback is used ([parse.rs:368-418](../../src/parse.rs)), so local benchmarking must pass the flag. The Dockerfile does pass it ([Dockerfile](../../Dockerfile)). The Docker image build itself was not tested. [M]
- Generator builds (`/home/claude/work/gen_build`); 20M events + 5M clicks generate in 12 s, 40M in 25 s. 60M events is not generable here: the generator was OOM-killed at 6.27 GB anon RSS by the sandbox memory cgroup (dmesg). [M]
- d20 inputs: 21,593,809 event lines in 1.661 GB (76.9 B/line), 5,137,623 click lines in 241 MB (46.9 B/line), 892,711 link lines in 18.4 MB, 143 event files, largest 300 MB. [M]
- d20 per-phase wall (seconds, warm page cache; each phase = difference of the cumulative stamps the binary prints) [M]:

| NCPU | phase0+1 links | phase2 parse | phase3 dedup | phase3b resolve | phase4 sessions | output | total (done) | user | sys | maxrss | arena used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 0.227 | 4.053 | 0.535 | 0.620 | 1.186 | 0.599 | 7.221 | 6.01 | 1.17 | 3.48 GB | 1039 MB |
| 2 | 0.176 | 1.786 | 0.347 | 0.298 | 0.552 | 0.394 | 3.555 | 5.78 | 1.08 | 3.53 GB | 1089 MB |
| 2 (second run) | 0.165 | 1.943 | 0.311 | 0.299 | 0.622 | 0.233 | 3.573 | 5.97 | 0.98 | 3.53 GB | 1088 MB |
| 4 (oversubscribed) | 0.155 | 2.090 | 0.285 | 0.541 | 0.542 | 0.240 | 3.854 | 6.35 | 1.17 | 3.63 GB | 1174 MB |
| 8 (oversubscribed) | 0.152 | 2.184 | 0.343 | 0.361 | 0.866 | 0.094 | 4.000 | 6.55 | 1.24 | 3.97 GB | 1387 MB |

- 1 to 2 threads scales 2.03x (7.22 s to 3.55 s). Thread counts above the core count cost 8 to 13% on this box. [M]
- First run after generation was 5.60 s with sys 3.88 s (vs ~1.0 s sys warm), so file-page availability matters a lot. [M]
- dev (1M events), NCPU=2, five warm runs: total 192 to 276 ms; phase0+1 = 27 to 29 ms, parse 93 to 157 ms, phases 3 to 4 about 60 ms, output 7 to 11 ms; arena used 181 MB; maxrss 377 MB. [M]
- d40 (40M events), NCPU=2: links 0.33, parse 5.10, dedup 0.68, resolve 0.72, phase4 1.26, output 0.30, total 8.38 s; user 12.52 s, sys 2.38 s. This run hit the sandbox memory limit (majflt 20,030, VmHWM 6.2 GB), so it is contaminated by page-cache eviction; use it for memory scaling, not speed. [M]
- CPU per event: dev 0.33 CPU-s per 1M, d20 6.86 CPU-s (0.34 per 1M), d40 14.9 CPU-s (0.37 per 1M, memory-pressured). [M]

### Inferences
- Projected 60M: about 20 to 22 CPU-seconds of total work. On 8 hyperthreads (roughly 4.5 to 5.5 effective cores for this instruction-bound workload) that suggests 4 to 5 s wall if the page cache is warm, far inside the 3 minute cap. This is an extrapolation, not a measurement.
- Fixed costs are small: about 30 ms before parse starts at 1M (catalog load plus links), so the practice (20M) and final (60M) scores are dominated by per-event costs.

### Gaps
- No machine with 8 hyperthreads was available: scaling beyond 2 threads, hyperthread yield, and memory-bandwidth limits are unmeasured.
- Cold-cache behaviour on the scorer (is `/data` in the page cache at start?) is unknown and untested; it could dominate everything else.
- Docker build and a run under the scorer's flags (`--read-only`, uid 1000, `/tmp` 64 MB) were not exercised. The binary writes only under OUTPUT_DIR and never uses SCRATCH_DIR or /tmp (code reading, [main.rs:322-504](../../src/main.rs)).

## 2. Parse throughput vs lower bounds; user vs sys; exit cost

### Takeaway
Parsing costs 150 to 190 ns per event line (0.41 to 0.52 GB/s per thread), which is 4 to 5x the cost of the SIMD line/field scan alone (35 ns/line, 2.2 GB/s) and about 22x a raw memchr newline count (7 ns/line, 10.9 GB/s). The gap is instruction count in field decoding (about 1,400 instructions per event line), not cache misses. Kernel time is about 16% of CPU and more than half of it is in the output phase. Exit teardown is negligible (about 20 ms).

### Cited Findings
- Single-thread, events only, all 1.66 GB already touched (BENCH=3 added in the copy, two repetitions) [M]:

| Stage | Time | GB/s | ns/line |
|---|---|---|---|
| `memchr` newline count | 0.152 to 0.155 s | 10.7 to 11.0 | 7.0 to 7.2 |
| `scan_lines::<7>` only (SIMD split, quote check, no decoding) | 0.744 to 0.784 s | 2.1 to 2.2 | 34.5 to 36.3 |
| scalar `for_each_line` + `split` | 1.87 to 1.90 s | 0.87 to 0.89 | 87 to 88 |
| full `parse_events` (decode + 512-way scatter of ERec) | 3.53 to 4.16 s | 0.40 to 0.47 | 163 to 193 |

- `cat events/*.csv | wc -l` took 1.48 s (1.1 GB/s, sys 1.69 s, dominated by the pipe copy); `cat > /dev/null` similar. [M]
- Whole phase 2 (events + clicks, 1.90 GB, 26.7M lines): 4.05 s on 1 thread = 0.47 GB/s, 152 ns/line; 1.79 s on 2 threads = 1.06 GB/s. [M]
- Built-in microbench (BENCH=2, first 160 MB, cumulative per added field, noisy by about +/-5 ns): scan 40 to 42, +pack eid 5, +pack uid 5, +ts 16, +ingest 15, +type 0.5, +price 6, +product lookup 37 to 40, full 168 to 174 ns/line. Resolving user to person at parse time would add ~120 ns/line (the 4,194,304-slot link map is 64 MB), which is why commit b3c1d38 moved it to phase 3b. "ts fast" was not faster than "ts" (61 vs 58 ns), which led to the bug in section 4. [M]
- Callgrind on dev, 1 thread: 2.395 G instructions total; `parse_events` + its closure 61.7%, `parse_clicks` + closure 8.8%, `process_partition` 8.0% self (12.5% inclusive), `dedup_partition` 3.95%, `resolve_partition` 2.6%, memcpy 1.7%, memcmp 1.05%, catalog load (`read_small_csv`, `Catalog::finish`, SipHash) about 2.5%. Simulated D1 miss rate is only 2.3%; `dedup_partition` alone has 24.6% of all D1 read misses and 29.5% of D1 write misses with 4% of instructions. [CG]
- user vs sys on d20: NCPU=2 user 5.78 s, sys 1.08 s (16%); NCPU=1 user 6.01, sys 1.17. Sys by phase at NCPU=2: links 0.06, parse 0.25, dedup 0.01, resolve 0.05, phase4 0.03, output 0.68 s. At NCPU=1: output 0.51 of 1.17 s. Minor faults about 12,000 to 14,000 total (the arena is on transparent huge pages: AnonHugePages 1.24 GB of 1.27 GB anonymous at measurement time). [M]
- `strace -f -c` on d20: 1,010 madvise calls (0.27 s under strace), 161 writes, 225 mmaps, 19 clone3 per par_for group; nothing else significant. [M]
- Exit: wall time measured around the process minus the "done" stamp = 17 to 24 ms at 20M for every thread count. `std::process::exit(0)` skips destructors ([main.rs:503](../../src/main.rs)). `AREL=1` (parallel madvise of the arena) and `DROP=1` (MADV_DONTNEED per parsed chunk) did not help: DROP=1 halved maxrss (3.53 to 1.67 GB) but raised sys from ~1.0 to 1.63 s. [M]
- IOMODE on warm cache, NCPU=2, one run each: 0 (plain demand faults) 4.58 s with sys 2.86 s; 1 (whole-file WILLNEED) 3.74 s; 2 (per-chunk WILLNEED) 3.65 s; 3 (default, per-chunk MADV_POPULATE_READ) 4.43 s in that run but 3.55 to 3.57 s in others; 4 (pread into a buffer) 4.22 s with maxrss 1.98 GB. [M]

### Inferences
- Mode 0 is clearly worse (sys nearly 3x). Modes 1, 2 and 3 are within noise of each other here. MADV_POPULATE_READ needs Linux 5.14 or newer; on an older scorer kernel the call returns EINVAL and the code silently degrades to mode 0 behaviour, which is the slow one. Switching the default to mode 2, or falling back to WILLNEED when POPULATE_READ fails, removes that risk at no measured cost.
- Because parse is instruction-bound (high IPC, low miss rate), hyperthreads should give a real but partial gain; every instruction removed from the per-line path converts almost directly to wall time.

### Gaps
- No `perf` (package not available, PMU not exposed), so there are no cycle-accurate hot spots; callgrind instruction counts plus wall-clock deltas are the substitute.
- Callgrind's last-level cache was auto-sized to the host's 260 MB L3, so LL miss numbers are meaningless for the scorer.

## 3. Actual architecture, and where it is inefficient

### Takeaway
The design is a sound radix-partitioned pipeline: each surviving event is written 3 to 4 times and hashed 4 to 5 times, with no global hash table on the hot path. The real inefficiencies are constant factors in the per-line decode (a dead fast path, a scalar ID packer, a per-field quote loop that 79% of lines take), unreserved growing vectors in phase 4 and output, and partition counts that are fixed at 512 regardless of data size.

### Cited Findings
Architecture as written:
- Records ([main.rs:79-107](../../src/main.rs)): `ERec` 32 B (eid u64, person u64, ts u32 ms offset from window start, ingest i32 seconds offset, pc u32 = price<<8 | category, 4 B pad); `SRec` 16 B (person, ts, pc); `CRec` 32 B (person, cid, ts i64, camp u32 + pad); `URow` 32 B. IDs are packed exactly into a u64 when at most 10 bytes over `-0-9A-Z_a-z` (6 bits per char, length in the low nibble, order-preserving); anything else is interned through a global `Mutex<HashMap>` ([keys.rs:28-67](../../src/keys.rs)).
- Phase 0: manifest, products and campaigns read with a scalar splitter into `Vec<Vec<Vec<u8>>>`, then a `Catalog` with a dense array for `<prefix><number>` IDs plus fallback maps ([keys.rs:290-390](../../src/keys.rs)). Serial.
- Phase 1 links ([main.rs:143-272](../../src/main.rs)): parallel parse into per-thread pair lists and 512 key shards; parallel per-shard interning; parallel pair to index translation; serial union-find with path halving over u32 indices; serial naming pass (min key per root); parallel rewrite of shard maps to ID to person-name. Result: 512 `KMap<Key>` shards keyed by the top 9 hash bits.
- Phase 2 ([main.rs:400-426, 569-630](../../src/main.rs)): files mmapped, cut into 4 MB chunks at newlines, one dynamic work queue shared by events and clicks. Each event line becomes an ERec (even when invalid, with `pc = INVALID`, because dedup precedes validation) appended to one of 512 per-thread block lists chosen by the eid hash. Clicks are validated immediately and appended to one of 512 lists by user hash. Block lists are 32 KiB blocks from one MAP_NORESERVE + MADV_HUGEPAGE arena ([mem.rs](../../src/mem.rs); the comments there still say 64 KiB).
- Phase 3 dedup ([main.rs:664-717](../../src/main.rs)): per eid partition, pass 1 inserts every record into a reused `KMap<u64>` (value = ingest<<32 | index, 16 B slots, capacity = next power of two of 2n so load is 0.25 to 0.5) and marks losers in a bitmap; pass 2 re-reads the records and emits valid survivors as SRec into 512 lists by user hash; blocks are recycled.
- Phase 3b resolve ([main.rs:632-662](../../src/main.rs)): per user-hash partition, in-place compaction that rewrites user to person through that partition's link shard and moves records whose person hashes to another partition. In d20, 60.6% of event rows have a user ID present in links and 39.2% are rewritten to a different person (microbench stats line). [M]
- Phase 4 ([main.rs:739-988](../../src/main.rs)): per partition, hash-group by person (`gmap` + a `gid` vector), counting-sort scatter into a contiguous buffer, same for clicks (only for persons with events), then per person: sort events by ts, sort clicks by (ts, click_id), sweep sessions, credit via a moving click cursor, per-day flush into `Vec<URow>`, and assists via per-campaign interval unions with two binary searches over session starts. Campaign totals go to a dense per-thread `[u64; 4]` array of (campaigns + 1) x days.
- Click dedup is never performed. This is correct under rules.md ("Copies of a click have identical values"): duplicates cannot change the latest-click choice, the tie-break, or the once-per-campaign assist.
- Output ([out.rs](../../src/out.rs), [pq.rs](../../src/pq.rs)): one `user_daily/part-i.parquet` per thread written in parallel plus one campaign file; one row group, one uncompressed PLAIN v1 data page per column, all integers as INT64.
- Threads: `par_for` spawns fresh scoped threads per phase and uses an atomic counter, so scheduling is dynamic; no oversubscription inside a phase. Output spawns threads + 1 writers.

Inefficiencies with evidence:
- Dead SWAR timestamp path: see section 4. 8.1% of all instructions on dev are the scalar `civil_secs` path (parse.rs lines 69 to 126 sum to 194 M of 2,395 M Ir) plus 1.6% wasted in the failing SWAR pre-check (lines 716, 720). [CG]
- Scalar `pack` loop ([keys.rs:52-67](../../src/keys.rs)): lines 54 to 63 total about 180 M Ir = 7.5% of all instructions (two 9-byte IDs per event, one table lookup and shift per byte). [CG]
- Quoted-field path in `scan_lines` ([parse.rs:531-575](../../src/parse.rs)): lines 536 to 571 total about 258 M Ir = 10.8%. The generator quotes 20% of fields independently (rules.md dirt table), so 1 - 0.8^7 = 79% of event lines and 59% of click lines contain a quote and take the 7-iteration per-field loop with ring-buffer bit tests; the `qcount == 0` fast path serves only 21% of event lines.
- Product lookup costs 37 to 40 ns/line in the microbench, far more than a 200 KB dense table should. `Catalog::get` compares the prefix with `s[..pl] == self.prefix[..]`, which compiles to a libc `memcmp` call per event (25 M Ir, one call per line) for a 1-byte prefix ([keys.rs:372](../../src/keys.rs)). [CG][M]
- 512-way scatter: 0.86 simulated L1 write misses per event line in the parse closure (929,596 D1mw over ~1.08 M lines); 512 open write heads x 64 B = 32 KB, equal to a Cascade Lake L1D. [CG]
- Dedup is the cache-bound phase (see D1 share above). Partition count is fixed: at 20M a partition holds ~42,000 records, so its map is 131,072 slots x 16 B = 2 MB; at 60M it holds ~127,000 records, giving a 4 MB map (262,144 slots) per partition, several times a Cascade Lake core's 1 MB L2 and shared by two hyperthreads. [M for counts; map sizes follow from `KMap::reset`, [keys.rs:145-156](../../src/keys.rs)]
- Phase 4 stalls from `Vec<URow>` growth: with STATS timers, partitions with only ~30,000 to 70,000 events took 50 to 100 ms while their sort and assist time was 0 to 1 ms (for example "n 71097 ... loop 98.5 ms (sort 1 ms, assists 1 ms)" at 40M, "n 35091 ... loop 80.5 ms" at 20M). `a.users` is never reserved ([out.rs:25](../../src/out.rs), [main.rs:1013](../../src/main.rs)); it reaches 2.24 M rows (72 MB) at 20M and 4.44 M rows at 40M, so each doubling copies tens of MB and faults fresh pages. [M]
- Skew is handled well: at 40M the largest partitions held 680,000 to 814,000 events with single persons of up to ~780,000 events and 114,120 clicks, and took 23 to 37 ms each (sort 14 to 29 ms, assists 3 to 6 ms). At 20M the five largest persons had 237,000 to 360,000 events. Phase 4 total was 1.1 to 1.4 CPU-s at 20M and 2.5 CPU-s at 40M. [M]
- Output: 2,242,393 user rows = 96.3 MB at 20M (42.9 B/row), 182 MB at 40M. Output wall 0.23 to 0.39 s at NCPU=2, 0.60 s at NCPU=1, but only 0.094 s when split into 8 files at NCPU=8 on the same 2 cores. Each row is copied URow to column vectors to a `page` vector to a `file` vector to `write_all`, and `file` grows without a reserve ([pq.rs:121-175](../../src/pq.rs)). `sessions` and `events` are written as INT64 although u32 in memory; 82.4% of rows have a null `top_category` (1,848,431 of 2,242,393). [M]
- Serial sections: union-find 34 to 109 ms at 20M and 76 to 85 ms at 40M; the naming loop and `all_keys` copy are serial too; phase 0 is serial; the campaign-array merge is serial. Phase 1 as a whole runs before phase 2 although parse does not use the link maps (they are used only in phase 3b, [main.rs:446-453](../../src/main.rs)).
- Hash recomputation: `hk(eid)` in parse and again in `mp.entry`; `hk(person)` in dedup pass 2, in resolve `map.get`, again for the moved record, and in `gmap.entry`. Each is 2 multiplies, minor.
- Dead code: `prefetch`, `Ctx::prefetch_person`, `LAG`, `masks64_pub`.

### Inferences
- The slow small partitions are best explained by reallocation of `a.users` (and to a lesser degree first-touch growth of the per-thread scratch vectors); the output phase's inverse dependence on file size points to the same cause (large unreserved vectors, fresh page faults, 3 copies). Both are small and safe to fix with `with_capacity` or by streaming pages straight to the file.
- Running phase 1 concurrently with phase 2 (or simply starting event parsing first and joining before phase 3b) would hide the whole links phase, including its serial union-find, behind the parse. At 60M the links phase is likely 0.3 to 0.5 s of mostly serial wall time, so this is worth several percent, but it is a structural change to `Ctx`.
- Scaling P1 with input size (for example 2,048 eid partitions at 60M so a dedup map fits in L2) should help dedup, but more partitions raise the partial-block footprint (512 lists x 32 KiB = 16 MB per thread per list family; five families) and the scatter cost in parse, so it needs measuring on real hardware.
- The scorer has AVX-512 (starter README). `target-cpu=x86-64-v3` leaves that unused; a `cmpeq_epi8_mask` scan would shave a few ns of the 35 ns scan cost. Low priority compared with the decode path.

### Gaps
- I did not measure alternative partition counts, write-combining buffers, or a SIMD `pack`; the percentages above are instruction shares, so realized gains will be somewhat lower.

## 4. Correctness status and fast-path risks

### Takeaway
Output is correct today: 36/36 fixtures pass, the dev answer matches exactly, output is identical across 1, 2 and 8 threads, and the Parquet files read correctly in polars, pyarrow and duckdb. One fast path is silently dead: `civil_swar` always returns None, so every F1 timestamp takes the scalar parser. Fixing two masks gives about 9% less parse CPU with the scalar fallback still guarding correctness.

### Cited Findings
- `scripts/run_fixtures.py fixtures -- BIN`: "36/36 fixtures passed". `scripts/compare.py out/dev dev-answer`: user_daily 117,038 vs 117,038 rows, campaign_daily 27,400 vs 27,400, 0 differences, "MATCH". d20 outputs from NCPU=1, 2, 8 and from IOMODE/DROP/AREL variants all "MATCH" each other. [M]
- d20 output read with pyarrow and duckdb: `day` is date32, strings are UTF8, 2,242,393 user rows; sum(sessions) = 3,402,836 and sum(revenue_cents) = 6,791,186,462 in both tables; sum(assisted_sessions) 36,877,579 >= non-organic sessions 2,092,518 (the rules.md self-check). [M]
- Bug: in `civil_swar` ([parse.rs:709-746](../../src/parse.rs)) the third word covers bytes 12..20 (`H:MM:SSZ`), which holds colons at byte 13 and byte 16, but the mask `SC = 0x0000_00FF_0000_0000` only neutralizes byte 16, so `digits8(c2)` always fails. A new unit test `civil_swar("2027-12-18T22:22:54Z")` fails on the unmodified code; the existing `ts_fast_matches` test cannot see it because it only compares `parse_ts_fast` to `parse_ts`, and the fallback makes them equal. [M]
- Fix (applied only in `repo_copy`): `SC = 0x0000_00FF_0000_FF00`, separator constant `0x0000_003A_0000_3A00`, fill constant `0x3000_0030_0000_3000`. With the fix, a 3,000,000-case fuzz (random and corrupted F1 strings) shows `civil_swar` and `parse_ts_fast` equal to the scalar `parse_ts` in every case, all 6 unit tests pass, fixtures 36/36, dev MATCH, d20 output MATCH vs the unfixed binary. [M]
- Measured gain of the fix on d20, NCPU=2, three alternating pairs: parse-phase user CPU 3.74 to 4.12 s before vs 3.36 to 3.63 s after; total wall 3.44 to 4.34 s before vs 3.32 to 3.40 s after; single-thread event parse 163 to 147 ns/line (BENCH=3), microbench full 173.8 to 158.3 ns/line. [M]
- Edge cases checked by reading the code against rules.md:
  - `\r`: not special in the scanner, so it stays in the last field; `1000\r` fails `parse_price`, `...Z\r` has length 21 and fails `parse_ts`; an ID containing `\r` or a space goes to the interned long-key path and stays distinct. Correct.
  - Quotes: a line passes only if every quote sits at both ends of a field of length >= 2 and the number of such quotes equals the line's quote count ([parse.rs:559-575](../../src/parse.rs)); `""` yields an empty field. Wrong field count and empty lines are dropped by the separator count. Lines longer than 3,968 bytes use the scalar splitter. A fuzz test against the scalar splitter exists and passes.
  - F3: 9 to 16 digits use SWAR; shorter or longer go to `parse_f3`, which accepts leading zeros and returns None above 19 significant digits. Rules say such a value is "only outside the window", and None is treated the same as out of window, so the result is identical.
  - `ingest_ts`: stored as whole seconds relative to window start, clamped to i32 (about +/-68 years); exact for valid F1. An unparsable ingest becomes i32::MAX (loses dedup, counts as late); rules say ingest is always valid F1.
  - Dedup before validation, late-event test (`> 24 h` strict), 30 minute gap (`<=` kept in session), credit tie-break (first of equal-ts run after sorting by click_id), assist window (inclusive on both ends), click window, organic handling: all match rules.md.
  - Upper-case IDs fit the packed alphabet and order correctly; link naming uses `key_cmp`.
  - IDs that do not pack (over 10 bytes, or bytes outside `-0-9A-Z_a-z`) are exact but slow: global mutex, heap allocation, and `key_cmp_slow` allocates two vectors per comparison. Generator IDs are a 1-letter prefix plus base-36 of a 40-bit number, so at most 9 bytes ([generator text.rs:192-217, user.rs:121-125](../../../x_generator/generator/src/text.rs)); measured lengths in d20 were 6 to 9 unquoted. So this path is essentially unused on generated data at any size.
- Hard-failure conditions in the code: `assert!(we - ws < 2^32 ms)` (49.7 days; generator windows are 14 days), `assert!(categories <= 256)` (generator has 20 + unknown), `expect` on manifest parsing, panic = abort. Any of these firing gives a non-zero exit.
- Page sizes are written as i32 and one page holds the whole column; at 60M the largest column chunk is on the order of 100 MB per file, far below 2 GB.

### Inferences
- The SWAR fix is the best risk-adjusted change in the audit: two constants, protected by an exact scalar fallback for any input the fast path rejects, and now covered by a test that proves the fast path is actually taken.
- Any change to `scan_lines`, `pack` or the Parquet writer should be gated on: fixtures, dev answer, the 20M cross-check against the current binary's output, and a pyarrow or duckdb read of the files, since a wrong answer scores zero.

### Gaps
- There is no independent reference answer for 20M or 40M; correctness at scale rests on determinism across thread counts plus the dev answer and fixtures.
- The scorer's Parquet reader is unknown; three readers accept the files, which is good evidence but not proof.

## 5. Memory at 60M events

### Takeaway
Memory is not a risk. Anonymous memory grows about 76 MB per million events (1.67 GB at 20M, 3.19 GB at 40M with 2 threads), so 60M should need roughly 5 to 5.5 GB anonymous with 8 threads, plus up to 5.5 GB of mapped input pages that are reclaimable page cache: well under 24 GB.

### Cited Findings
- 20M, NCPU=2: VmHWM 3.53 GB = RssAnon 1.67 GB + RssFile 1.86 GB (the mmapped CSVs); arena used 1,088 MB of 1,922 MB reserved; AnonHugePages 1.24 GB. [M]
- 40M, NCPU=2: VmHWM 6.22 GB, RssAnon 3.19 GB, RssFile 2.83 GB (capped by the sandbox cgroup), arena used 2,028 MB of 3,113 MB reserved. [M]
- Thread count effect at 20M: arena used 1,039 MB (1 thread), 1,089 MB (2), 1,174 MB (4), 1,387 MB (8); maxrss 3.48 to 3.97 GB. [M]
- Arena sizing: reserve = 1.5 x (event bytes / 78 x 32 + click bytes / 40 x 32 + (P1 + 2 x P2) x threads x BLK x 2 + 256 MB), mapped MAP_NORESERVE, with a heap fallback when exhausted ([main.rs:373-375](../../src/main.rs), [mem.rs:66-75](../../src/mem.rs)).
- Non-arena anonymous memory at 20M is about 0.6 GB: link shards (4,194,304 slots x 16 B = 64 MB), phase 4 scratch (sized to the largest partition), `Vec<URow>` (72 MB plus reallocation slack), and output buffers (about 3 to 4 copies of the 96 MB output while writing). [M, derived from RssAnon minus arena used]
- IOMODE=4 (pread) and DROP=1 cut peak RSS to 1.98 GB and 1.67 GB at 20M if ever needed. [M]

### Inferences
- 60M estimate: ERec 64.8 M x 32 B = 2.07 GB, CRec about 15.4 M x 32 B = 0.49 GB, SRec reuses freed ERec blocks, partial blocks at 8 threads about 0.3 to 0.6 GB, so arena about 3.2 to 3.5 GB; other anonymous about 1.6 to 1.9 GB (output buffers about 1.1 GB for ~275 MB of Parquet, URow 215 MB, link maps about 200 MB, scratch). Total anonymous about 5 to 5.5 GB.
- The one thing to avoid is anything that makes memory scale with thread count times partition count (for example raising P1 to 4,096 with 32 KiB blocks would touch about 1 GB per list family at 8 threads).

### Gaps
- 60M could not be generated or run here (generator OOM in a ~6.5 GB cgroup), so the 60M figure is a linear extrapolation from two points.

## 6. Ranked optimization targets (deadline Sunday 2026-11-22 11:00 EST)

### Takeaway
Three small, safe changes are worth doing first (SWAR mask fix, reserve or stream the output and `URow` buffers, harden the IOMODE default). The larger wins are all in the per-line decode (ID packing, quoted-field handling, product lookup) and carry more correctness risk.

### Cited Findings
Small and safe (hours, low risk):
1. Fix `civil_swar` masks ([parse.rs:715-719](../../src/parse.rs)). Measured about 9% less parse CPU, roughly 4 to 6% of total wall. Diff and fuzz test in `/home/claude/work/swar_fix_and_tests.diff`. [M]
2. Reserve `Acc.users` (or store URow in arena block lists) to remove the 50 to 100 ms reallocation stalls seen in phase 4. [M]
3. Output: reserve `file` exactly or write header and pages directly; build pages straight from URow; write `sessions` and `events` as INT32 (rules.md: "Any integer width"), which cuts user_daily from about 43 to 35 B/row. Output is 6 to 11% of wall and about half of all sys time at 20M. Validate with pyarrow and duckdb after any change. [M]
4. IOMODE: default to 2, or fall back to MADV_WILLNEED when MADV_POPULATE_READ returns an error, so an older kernel cannot drop to the mode-0 behaviour (sys 2.86 s vs ~1.0 s at 20M). [M]
5. Replace the per-event `memcmp` prefix compare in `Catalog::get` with a fixed-width compare for short prefixes ([keys.rs:372](../../src/keys.rs)). [CG]
6. Order phase 4 partitions largest first so a whale partition cannot be the last one running alone (tail risk about 30 to 50 ms). [M for partition times]

Medium (half a day each, moderate risk, need the full regression gate):
7. Vectorized or SWAR `pack` for the two IDs per event (7.5% of instructions). Must keep exact order-preserving semantics and the long-key fallback; exhaustively compare with the scalar version. [CG]
8. Cheaper quoted-line handling in `scan_lines` (10.8% of instructions; 79% of event lines take it). The existing fuzz test is the safety net. [CG]
9. SWAR for F2 timestamps (10% of `ts` values) by reusing the fixed `civil_swar` plus an offset parse. Small gain. 
10. Start link building concurrently with event and click parsing, joining before phase 3b; hides 0.15 to 0.45 s (20M to 40M measured, 1 to 2 threads) including the serial union-find. [M]

Larger or uncertain (only with spare time and real hardware to measure on):
11. Scale `P1` with input size or use a smaller dedup slot so the dedup map fits L2 at 60M; interacts with partial-block memory and scatter cost. 
12. Write-combining buffers for the 512-way scatters in parse and dedup.
13. AVX-512 scanner via a `cascadelake` target (scorers are "Cascade Lake or newer, with AVX-512 available" per the starter README); gains limited to the 35 ns/line scan.

### Inferences
- Items 1 to 4 together plausibly give 8 to 15% at 20M with almost no correctness exposure. Items 7 and 8 together could give a similar amount again but touch the two most correctness-critical routines.
- Nothing found suggests an algorithmic problem at 60M: cost is linear, skew is absorbed, memory is about a quarter of the cap.

### Gaps
- All gain estimates except item 1 are derived from instruction shares or phase timings, not from implemented prototypes.
