# RESEARCH_NOTES.md

Research-only notes for the optimizing session. Nothing here has been built or benchmarked by me; every number is quoted from the source named next to it. Line numbers refer to `src/` as read on 2026-11-21 (HEAD `1c4fa8e` plus uncommitted edits to `keys.rs`).

`OVERNIGHT_LOG.md` did not exist when iteration 1 ran, so "tried or rejected" was taken from `git log`, `speedups.md` and `repo_audit_and_profile.md`.

## Top five untried ideas (ranked by expected gain per unit of correctness risk)

Re-ranked in iteration 14 against the x86 profile in `OVERNIGHT_LOG.md` (parse 1.28 of about 2.1 s in-process at 20M; 1,040 instructions per event line, of which scanner 490, ID packing and catalog 230, timestamps and price 230, record build 90).

| # | Idea | Where | Expected gain | Risk to exactness | Entry |
|---|---|---|---|---|---|
| 1 | Line-at-a-time splitter fed by block masks computed once per 64 bytes; quote validity and field offsets from two mask words, no `pos` index, no quote ring | `scan_lines`, `parse.rs:486-654` | The scanner is 490 of 1,040 instructions per line and 27% of `perf` self time (19.5% plus the 7.7% quote loop). Estimate 130 to 170 instructions per line after, so 10 to 15% of wall. The algorithm is model-checked | Medium: this routine decides which lines are malformed. The model check and `fuzz_scan` are the net | [14.1](#141-a-line-at-a-time-splitter-that-scans-each-byte-once) |
| 2 | Narrow dedup side stream with tombstones, **keeping** partition-by-user and the per-partition resolve | `parse_events`, `dedup_partition`, `resolve_partition` | Removes dedup pass 2 and its second scatter; with two-bit counters the dedup table drops from megabytes to under 256 KB. Estimate 6 to 10% of wall | Medium | [14.2](#142-do-the-tombstone-dedup-without-moving-person-resolution-into-the-parse-loop), [1.3](#13-dedup-that-marks-losers-in-place), [9.1](#91-two-bit-counters-instead-of-a-key-table) |
| 3 | Nibble-table and multiply-add ID packing, two IDs per line in one 256-bit operation | `pack_fast`, `keys.rs:73-97` | `pack_fast`'s loop is 5.8% of `perf` self time; estimate 3 to 5% of wall. Model-checked against `pack` including `-` | Low: bit-exact in the model; `pack_long` path unchanged | [14.3](#143-simd-id-packing-that-matches-this-repos-key-format) |
| 4 | Sort 8-byte `(ts << 32) \| pc` keys per person | `process_partition`, `main.rs:814-830`, `:893-896` | Measured by a competitor at -2.8% for the same change; three teams do it | None | [7.1](#71-sort-8-byte-keys-per-person), [11.3](#113-measured-numbers-for-ideas-already-in-this-file) |
| 5 | Known-date table for timestamps | `civil_swar`, `parse_ingest_s` | Timestamps and price are 230 instructions per line; a hit replaces digit checks, range checks and `days_from_civil` for both timestamps. Estimate 3 to 5% of parse | Low | [1.4](#14-known-date-table-for-timestamps) |

Runners-up, in order: write-combining for event records ([1.2](#12-write-combining-buffers-with-non-temporal-stores), [16.2](#162-non-temporal-stores-gained-7-on-the-ice-lake-mirror-after-all): -7% and -12% on Ice Lake for two teams, no gain on an EPYC, nothing known on Cascade Lake); profile-guided optimisation as the last step ([16.4](#164-profile-guided-optimisation-a-measured-3-to-4-reopening-a-rejected-item): a measured 3 to 4%); `pread` in 512 KB units ([1.5](#15-pread-in-l2-sized-blocks), [11.2](#112-what-the-scorer-facts-change); needs an x86 A/B, sign unknown); catalog load off the serial path ([8.1](#81-catalog-load-is-about-60m-serial-instructions-before-anything-parallel-starts)); 16-byte click records ([13.2](#132-16-byte-click-records-inside-a-person-group)); link shards without self-named IDs ([13.1](#131-leave-self-named-ids-out-of-the-link-shards)); 30-minute bins ([1.6](#16-sessions-from-30-minute-bins)); smaller Parquet ([1.8](#18-smaller-parquet-output)).

Closed: everything for a cold page cache (1.9, 3.1), because `/data` on the scorer is reported to be tmpfs.

**Measure before ranking:** whether output size costs time after the process exits ([20.1](#201-output-bytes-may-be-paid-for-after-the-process-exits)). One team measured +0.15 s for 14 MB more output on a bind mount; this build writes about 96 MB at 20M. If that holds on the x86 runner, shrinking the output outranks everything except item 1.

## A finding to act on before trusting the TSBL toggle

`parse_ts_bl` (`parse.rs:819`) still has the pre-fix mask: `SC = 0x0000_00FF_0000_0000` and fill `0x3000_0030_0000_0000` at lines 829, 834 and 837. That is the same bug `5b17560` fixed in `civil_swar`: the colon at byte 13 is not neutralized, so `digits8(c2)` is false, `civ_ok` is false, and every F1 and F2 input falls through to the scalar `parse_ts`. Tests pass because the fallback is exact. Any timing of `TSBL=1` so far measured the scalar parser plus wasted SWAR work, so a "TSBL is slower" verdict is not evidence about the branch-free design. The fix is the same three constants (`SC = 0x0000_00FF_0000_FF00`, separator `0x0000_003A_0000_3A00`, fill `0x3000_0030_0000_3000`) plus a test that asserts the non-fallback branch is taken.

`parse_ingest_s` (`parse.rs:894-897`) has the old constants too, but it is correct: it only reads the seconds pair from that word, and the stray colon cannot carry into it.

## What is already tried, rejected or queued (do not re-propose)

- Implemented and kept, from this file: product category only for purchases and hoisted per-line loads (entries 1.1 and 8.2; experiment 1 in `OVERNIGHT_LOG.md`, commit `e7a116f`, -2.3% at 20M and -2.4% at 60M on the x86 runner, against a competitor's -2.3% for the same change).
- Implemented (git log): `civil_swar` fix; one-word `Catalog::get`; `FLAT16` and `TSBL` toggles; sliding-window assists; `P1BITS`/`P2BITS`; AVX-512BW masks behind `XCPU`; background read-ahead; SWAR F2; `pack_fast`; branch-free quote check; unchecked F1 ingest; dense catalog; per-partition resolve; flatten-based scanner; prefetch pipeline for parse-time person lookup (replaced by per-partition resolve in `b3c1d38`).
- Queued in `speedups.md` and not yet in git: reserve `Acc.users`; reserve or stream Parquet buffers; `MADV_WILLNEED` fallback; largest partition first; links concurrent with parse; INT32 counts. Competitor numbers for two of these are in [1.10](#110-competitor-numbers-for-ideas-already-queued).
- Rejected in `speedups.md` by measurement: `IOMODE=4` as default (one run, whole 4 MB chunk per `pread`), `DROP=1`, `AREL=1`.
- Rejected in `speedups.md` by argument only, with no measurement: write-combining, date cache. Entries 1.2 and 1.4 reopen these two because competitors have since measured them; everything else on that list stays closed.

---

## Iteration 1 (2026-11-21): what the public competitor repos measured

Sources opened: shallow clones of eight public repos at the commits named below. Each claim cites the file or commit message it came from. These are other teams' measurements on their own code and machines, so ratios transfer better than absolute times.

Context: the practice board top was 0.65 s (`top7_candidate_repos.md`). [micah-andelman PERF.md](https://github.com/micah-andelman/shopping-sessions/blob/4685e4a/PERF.md) reports 0.495 s in-process at 20M on 8 pinned hyperthreads and says container start and exit add about 0.47 s under Docker. [cherylaurelia](https://github.com/cherylaurelia/builders-hackathon/commit/0fcf3a4) reports 355 ms warm and 438 ms cold at 20M on its own box.

### 1.1 Product lookup only for purchases

- **Idea.** Resolve `product_id` to a category only when the row is a valid purchase.
- **Source.** [micah-andelman `44f244e`](https://github.com/micah-andelman/shopping-sessions/commit/44f244e): "product/category lookup only for purchase rows (category is used only when price > 0)". It was part of the v5.5 bundle (0.74 s to 0.66 s local, with two other changes; not isolated).
- **Here.** `main.rs:632` runs `ctx.products.get(f[5], floor)` for every valid event. The low byte of `pc` is read in one place, `main.rs:936`, inside `if price != 0`. Change line 632 to compute the category only when `price != 0` and store 0 otherwise. The `f[5].is_empty()` check at line 615 stays, since a non-empty `product_id` is a validation rule for every type.
- **Expected gain.** The audit's microbench put "+product lookup" at 37 to 40 ns per line before `1c4fa8e` made it a one-word compare. Whatever it costs now, 97% of it goes away, and the 200 KB dense table stops competing for L2 with the 512 write heads.
- **Risk.** None to exactness. `INVALID` is `u32::MAX` and a non-purchase `pc` of 0 cannot collide with it.

### 1.2 Write-combining buffers with non-temporal stores

- **Idea.** Each partition gets a small cache-resident stage (a whole number of cache lines). A full stage is copied to the partition's block with `_mm256_stream_si256`, so the destination line is never read for ownership and never occupies L1/L2.
- **Sources.**
  - [Vibhor7-7 BENCH.md](https://github.com/Vibhor7-7/shopping-sessions/blob/1359a83/BENCH.md), experiment 18 (`9fcffee`): total 1514 to 1334 ms (-12%) and parse 1110 to 931 ms (-16%) on an Ice Lake Xeon 8370C, 4 threads; one thread parse -20%. Experiments 19 and 20: clicks through the same buffers -0.7%; 8 records per buffer instead of 4 for 16-byte records -0.9%. The same bundle was -6.4% on Zen 4 and -1.3% at 60M on Zen 3, so the gain depends on the CPU. Scored 0.96 s to 0.82 s together with an image change.
  - [Vibhor7-7 `src/pool.rs`](https://github.com/Vibhor7-7/shopping-sessions/blob/1359a83/src/pool.rs): `Combine<T, N>` and `stream_lines`.
  - [explorer-skp `src/swc.rs`](https://github.com/explorer-skp/buildathon-shopping-sessions/blob/d58857c/src/swc.rs): 8-row stages, streamed when the destination is 32-byte aligned (`e4c1d90`, `d58857c`).
  - [Th3r0n `5348625`](https://github.com/Th3r0n/shopping-sessions/commit/5348625) and [cherylaurelia `bbb551b`](https://github.com/cherylaurelia/builders-hackathon/commit/bbb551b) (192-byte groups) do the same.
- **Here.** `speedups.md` set this aside because the literature measured it for scatter into contiguous arrays. Four teams with the same block-list design as `mem.rs` kept it after A/B tests, and the audit's own callgrind shows 0.86 L1 write misses per event line in the parse closure. Change `BList::push` (`mem.rs:128`) so the record goes into a per-thread stage `[T; N]` indexed by partition, and a full stage is streamed to `cur`. Arena blocks are 32 KiB-aligned from a page-aligned base, so every stage boundary is 64-byte aligned when `N * size_of::<T>()` is a multiple of 64: `N = 8` for `ERec` and `CRec` (256 B), `N = 8` for `SRec` (128 B). At 512 partitions that is 128 KB of stage per thread for events, which fits L2 (1 MB or more on Cascade Lake and Ice Lake). Add a `finish` that copies partial stages with ordinary stores and issues `_mm_sfence` before `par_for` returns, in `parse_events`/`parse_clicks` callers and in `dedup_partition`.
- **Expected gain.** 6 to 12% of wall if the scorer behaves like the Ice Lake measurement; near zero on the ARM dev machine, so it must be judged on x86.
- **Risk.** Low for exactness, since records are only delayed. The failure modes are a missed partial flush or a `len` that counts staged records the reader cannot see; `for_each_slice`, `compact` and `release` must only run after `finish`. `BList.len` is read in `dedup_partition` and `process_partition`, so `finish` has to precede the transpose.

### 1.3 Dedup that marks losers in place

- **Idea.** 96 to 98% of events have one copy. Write each valid event once, directly into its final (user-hash) partition, and send only a small key record through the event_id partitions. Dedup then finds the rare repeated keys and marks the losing copies dead where they already lie.
- **Sources.**
  - [cherylaurelia `src/main.rs`](https://github.com/cherylaurelia/builders-hackathon/blob/2e4261f/src/main.rs) header, lines 12 to 17: "A valid event is a 12-byte record in its person partition; every event also leaves (event key, ingest_ts, record address) in its event-key bucket ... every copy but the earliest ingested is marked dropped in place." Commits `8dd2213` and `ee47dfe` introduced it ("no dedup copy pass"); `905724d`, `838452e`, `c9a415f` took the dedup phase from 36 ms to 19 ms wall at 20M with a table of 64-byte lines holding 8 tags and 8 positions, probed with one AVX2 compare.
  - [explorer-skp `48f1504`](https://github.com/explorer-skp/buildathon-shopping-sessions/commit/48f1504): "24-byte dedup keys; dedup marks losing copies only"; `eb5fad6`: one pass, "leaders are kept as seen and the rare beaten ones dropped after".
  - Counter-evidence: [micah-andelman `1ff0c20`](https://github.com/micah-andelman/shopping-sessions/commit/1ff0c20) rejected 16-byte dedup records as "slower: more CPU from random payload fetches". That variant fetched the payload for every record; the design above touches a payload only for the 2 to 4% that lose.
- **Here.** In `parse_events`, a valid line pushes an `SRec` into `sbl[hk(user) >> (64 - p2)]` and a 16-byte `(eid, ingest: i32, loc)` into `ebl[hk(eid) >> (64 - p1)]`, where `loc` identifies the `SRec` (block pointer plus index, or 0 for an invalid copy). An invalid line pushes only the key record with `loc = 0`. `dedup_partition` keeps its pass 1 over the key records and drops pass 2: when a copy loses and its `loc` is non-zero, write a dead marker into that `SRec` (`pc = INVALID`). When the winner is an invalid copy, every valid copy of that id is a loser and gets marked, which preserves "a later valid copy does not rescue it". `resolve_partition` already compacts each list through a closure, so returning `false` for dead records removes them at no extra pass; with no links, `process_partition` must skip them instead.
- **Expected gain.** Dedup is about 10% of wall and the only cache-bound phase. Pass 2's scatter of about 20M `SRec`s disappears and pass 1 reads 16 B instead of 32 B per record. Estimate 5 to 8% of wall.
- **Risk.** Medium. The rules that must survive: every parsed line with a non-empty `event_id` takes part, valid or not; the smallest `ingest_ts` wins; equal `ingest_ts` copies are identical so exactly one is kept. Two eid partitions never mark the same `SRec`, so the marking is race-free, but it must finish before `resolve_partition` moves records. Test with the fixtures that cover dedup-before-validation and with a second seed, since 1 in 64 duplicated events names a different user in later copies.

### 1.4 Known-date table for timestamps

- **Idea.** A window has about 30 distinct dates. Keep a small per-thread table of dates already validated: key is the first 11 bytes (`YYYY-MM-DDT`), value is that midnight in seconds. A hit costs two word compares and skips digit checks, month and day validation, the leap rule and `days_from_civil`.
- **Sources.**
  - [cherylaurelia `src/main.rs`](https://github.com/cherylaurelia/builders-hackathon/blob/2e4261f/src/main.rs) lines 1064 to 1074: "A date already checked: its `YYYY-MM-DDT` (11 bytes) and the epoch ms of its midnight ... remembers the dates it has checked (by day of month), so an F1 or F2 timestamp of a known date costs two compares and the time of day."
  - [micah-andelman `3860b34`](https://github.com/micah-andelman/shopping-sessions/commit/3860b34): per-worker date memo; part of v5.9 (0.525 s to 0.495 s with two other changes).
  - [Vibhor7-7 `bb20c47`, `0c6d2a0`](https://github.com/Vibhor7-7/shopping-sessions/commit/0c6d2a0): "look dates up in a table", "the date table compared in two words".
- **Here.** `speedups.md` dismissed a last-date cache because rows are shuffled. A table indexed by day of month does not depend on row order. In `civil_swar` (`parse.rs:772`): index a 64-entry per-thread array with the two day digits (`((b & 3) << 4) | ((b >> 8) & 15)` on the low bytes of `ld8(p + 8)`); on `a == e.a && (b & 0xFF_FFFF) == e.b` take `e.midnight` and go straight to the time-of-day word. On a miss run the existing code and store the entry if it validated. `parse_ingest_s` (`parse.rs:889`) gets the same hit path; its miss path must use the checked parser before storing, so an unvalidated date never enters the table.
- **Expected gain.** Two timestamps per event line, 58% F1 plus 10% F2 for `ts` and 100% F1 for `ingest_ts`. Estimate 2 to 5% of parse CPU; no competitor isolated it.
- **Risk.** Low. The compare is exact on all 11 bytes, and only validated dates are stored. Extend `civil_swar_fuzz` to run every case twice (cold and warm table) and to mix valid and invalid dates sharing a day of month.

### 1.5 `pread` in L2-sized blocks

- **Idea.** Read the input with `pread` into a per-thread buffer small enough to stay in L2, instead of parsing mapped file pages.
- **Sources.**
  - [Vibhor7-7 BENCH.md](https://github.com/Vibhor7-7/shopping-sessions/blob/1359a83/BENCH.md), experiment 10: reading in place through `mmap` was 998 to 1073 ms (+7.5%, parse +11%) on a Granite Rapids VM and was reverted: "page-table walks on 4 KB file pages cost more than the copy, on a VM". Block-size sweep: 64 K to 256 K within 1%, 16 K 3% worse.
  - [cherylaurelia `src/main.rs`](https://github.com/cherylaurelia/builders-hackathon/blob/2e4261f/src/main.rs) lines 4 to 6: "read with pread into a small per-thread buffer, so no input page is ever mapped".
  - Counter-evidence: micah-andelman (0.495 s) and Th3r0n (`b848617`) map the input and use `MADV_POPULATE_READ`, as this repo does. The scorers are Google Cloud n2 VMs (track page), which is the case Vibhor measured.
- **Here.** `IOMODE=4` exists (`parse.rs:258-275`) but reads a whole 4 MB chunk, which overflows L2, so each byte crosses DRAM twice; the audit's single run of it (4.22 s against about 3.6 s) tested that variant on 2 vCPUs. The untried variant is to cut chunks at 128 to 256 KB in `chunks_of` when `IOMODE=4` (`main.rs:411-412`), keeping the 8-byte `pre` margin. It also removes 1.9 GB of file mappings from exit teardown and the per-chunk `madvise` calls (1,010 at 20M).
- **Expected gain.** Unknown sign on the scorer; up to 7% if n2 behaves like Vibhor's VM. This needs an 8-thread x86 VM or a scoreboard A/B.
- **Risk.** Low to medium. The scanner's 64-byte tail copy and the `floor`/`ceil` guards in `pack_fast`, `parse_ts_fast` and `Catalog::get` already handle buffer edges, but smaller chunks hit those edges 16 to 32 times more often, so run the fuzz tests with tiny chunk sizes.

### 1.6 Sessions from 30-minute bins

- **Idea.** Bin a person's events by `ts / 1_800_000`. Two events in one bin are never a session break; bins two or more apart always are; adjacent bins break exactly when `min(later) - max(earlier) > GAP`. Sessions follow from a bitmap walk with per-bin (min, max, count, revenue), with no sort.
- **Sources.** [Vibhor7-7 BENCH.md](https://github.com/Vibhor7-7/shopping-sessions/blob/1359a83/BENCH.md) experiment 8 (`f761de9`): total -2.7%, people phase -10%. [cherylaurelia `5a062ff`](https://github.com/cherylaurelia/builders-hackathon/commit/5a062ff): sessions-phase CPU 0.495 to 0.456 s; rule stated at `src/main.rs` lines 2370 to 2372. Alternative: [micah-andelman `4da6863`](https://github.com/micah-andelman/shopping-sessions/commit/4da6863) kept sorting but used an LSD radix sort on `(person number << 31 | ts)`, person stage 0.102 to 0.091 s.
- **Here.** Replaces `evs.sort_unstable_by_key` at `main.rs:895` and the sweep at `:911-943`. Purchases (3%) need their session's start day for the category sums, so they are joined by bin after the sessions are known.
- **Expected gain.** 2 to 3% of wall.
- **Risk.** Medium. The generator plants gaps of exactly 30:00.000 and 30:00.001; the adjacent-bin rule is exact for both, but the purchase-to-session join is new code on the path that decides `top_category`.

### 1.7 One-multiply hash

- **Idea.** Replace `hk` (murmur fmix64: two multiplies, three shifts) with `k * 0x9E37_79B9_7F4A_7C15`, taking the partition from the top bits and the slot from the bits below them.
- **Source.** [cherylaurelia `838452e`](https://github.com/cherylaurelia/builders-hackathon/commit/838452e): "line and tag from one multiply (Fibonacci hashing) instead of two mix() per record", dedup phase 0.187 to 0.166 CPU-s with a prefetch change.
- **Here.** `hk` at `keys.rs:136` is called 4 to 5 times per event (audit §3). Packed keys carry their entropy in the high bits and the length in the low nibble; a single odd multiply propagates low bits upward only, so slot selection must use bits just below the partition bits (`h >> (64 - p - slot_bits)`), not `h & mask` as `KMap` does now (`keys.rs:203`).
- **Expected gain.** About 1%.
- **Risk.** None to exactness: a hash affects speed only. A poor bit choice shows up as long probe chains, so measure probe length on real IDs first.

### 1.8 Smaller Parquet output

- **Idea.** INT32 for `sessions` and `events`, `DELTA_BINARY_PACKED` for the integer columns, `RLE_DICTIONARY` for `user_id` over runs of a person's rows, one `write(2)` per file.
- **Sources.** [cherylaurelia `88ed99d`](https://github.com/cherylaurelia/builders-hackathon/commit/88ed99d): `user_daily` 60.6 to 24.7 MB, per-thread write 25 to 20 ms, read back with pyarrow and polars. `3458d27`: write-phase CPU 0.137 to 0.116 s. [Vibhor7-7 BENCH.md](https://github.com/Vibhor7-7/shopping-sessions/blob/1359a83/BENCH.md) experiment 17: writing 222 MB to disk added about 35 ms after the process exited under Docker, so smaller output is worth about 1% there.
- **Here.** `pq.rs:152-158` and `out.rs:48-50`. The INT32 step is already queued in `speedups.md`; the encodings are the new part.
- **Expected gain.** 1 to 2% of wall, mostly kernel time.
- **Risk.** Medium for the encodings: the scorer's reader is unknown, and a file it cannot read scores zero. INT32 alone is low risk.

### 1.9 Cold-cache read-ahead

- **Idea.** Two helper threads calling `readahead(2)` in 512 KiB steps, and skip the helpers when the input is already cached.
- **Sources.** [cherylaurelia `0fcf3a4`](https://github.com/cherylaurelia/builders-hackathon/commit/0fcf3a4): cold 20M median 474 to 438 ms against 4 threads of 128 KiB `fadvise`; a single thread of 512 KiB `fadvise` measured 449 ms; warm unchanged. `d8f1773`: 2 helpers beat 4 (0.50 to 0.41 s cold). [micah-andelman `1ff0c20`](https://github.com/micah-andelman/shopping-sessions/commit/1ff0c20): sample residency with `mincore` and start no helpers when 80% is resident.
- **Here.** `background_readahead` (`parse.rs:328`) uses one thread and 32 MiB `posix_fadvise` steps. The measured difference between one 512 KiB `fadvise` thread and two `readahead` threads is 1% cold, so this is only worth doing if the scorer's first run is cold, which is unknown.
- **Expected gain.** 0 to 8% cold, 0 warm.
- **Risk.** None to exactness.

### 1.10 Competitor numbers for ideas already queued

These are in `speedups.md` already; the numbers are new.

- **Largest partition first.** [micah-andelman `848af15`](https://github.com/micah-andelman/shopping-sessions/commit/848af15): person stage 0.096 to 0.084 s at 20M on 8 threads. [explorer-skp `bd47d9c`](https://github.com/explorer-skp/buildathon-shopping-sessions/commit/bd47d9c) does the same.
- **Links concurrent with parse.** [cherylaurelia `400f7da`](https://github.com/cherylaurelia/builders-hackathon/commit/400f7da): wall 1.33 to 1.00 s early in that repo's history. [micah-andelman `26b0e6b`](https://github.com/micah-andelman/shopping-sessions/commit/26b0e6b) parses links on all threads first and runs only the serial tail (union-find, naming) beside event parsing, giving each thread 8 event groups so the tail's thread is not a straggler. micah also records two rejected variants: parallel binary-search link build (no wall gain) and 256 KB link chunks (link parse finished later, 0.045 against 0.034 s).
- **Partition counts.** Vibhor7-7 swept 9/9, 10/10, 11/10 at 20M and 9/9 to 11/11 at 60M: all within 1%. "Fewer partitions make parse faster and people slower by the same amount." This argues against spending a submission on `P1BITS`/`P2BITS`.
- **Thread count.** Vibhor7-7 on an Ice Lake 8370C with 2 cores and 4 hyperthreads: 2 threads 1812 ms, 4 threads 1492 ms, 8 threads 1523 ms. The second hyperthread is worth 21% and oversubscription costs 2%, so `NCPU=8` on 8 hyperthreads is right.
- **Container overhead.** Vibhor7-7 experiment 17: about 80 ms before `main` and 45 to 80 ms after it per `docker run`, independent of image size. micah-andelman: about 0.47 s around the process on a Mac. Neither is the scorer's sandbox.

### Not worth pursuing, from this iteration

- **PEXT ID packing** (reversed in [14.3](#143-simd-id-packing-that-matches-this-repos-key-format) after the x86 profile). Vibhor7-7 experiment 2 measured -8.6%, but against a byte loop. `pack_fast` already removes the serial chain, and the audit prices both packs at about 10 ns per line. cherylaurelia's two-IDs-in-one-256-bit-op version gained about 2% (`e3bdd51`).
- **Zero-page IPIs.** cherylaurelia `c166174` cut TLB IPIs from 8,400 to 2,000 per run by filling tables with real writes instead of `vec![0; n]`. Here `KMap` writes `EMPTY` into every slot and `Acc.camp` is first touched by a write, so the pattern (first read of an untouched page, then a write, while other threads run) does not occur.
- **Wider link table.** Vibhor7-7 experiment 7: twice the slots was 1.3% slower; building it cost more than the lookups saved.
- **Scanner batch size.** micah-andelman swept 2 to 64 KB and found it flat; `SEG` here is 2 KiB.

---

## Iteration 2 (2026-11-21): scanner designs, and counter-evidence from the remaining repos

Sources opened: `src/main.rs` of cherylaurelia at `2e4261f`, `src/scan.rs` of explorer-skp at `d58857c`, and the commit logs and notes of AbhirajTiwari12 (`13c214a`), joshuamuellenbach25 (`ef05a85`) and Eagle Eye (`9e3c34a`). `OVERNIGHT_LOG.md` still did not exist; `git log` showed no new source commits since `1c4fa8e`.

### 2.1 Split each line from its own masks

- **Idea.** Drop the per-segment separator index (`pos`), the newline list (`nl`) and the quote ring (`qr`). At each line start, load 64 bytes (128 if no newline is found), take the comma and quote masks below the first newline bit, and derive all N field offsets and lengths from those two words.
- **Sources.**
  - [cherylaurelia `src/main.rs`](https://github.com/cherylaurelia/builders-hackathon/blob/2e4261f/src/main.rs) lines 1254 to 1380 (`scan_lines`, `fast_split`). Field count is `popcount(c0) + popcount(c1) == N - 1`. Quote validity is one expression: with `b` = comma-or-newline bits, `s = (b << 1) | 1` (field starts) and `ends = b >> 1`, let `x = q & s` (opening quotes) and `y = (q << 1) & b` (closing quotes moved onto the delimiter after them); the line is well quoted exactly when `(q & !(s | ends)) | (x & ends) | ((y - x) & b) == 0`. The subtraction borrows across each quoted field, so an unmatched quote leaves a bit on a delimiter. Which fields are quoted comes from `pdep(pext(x, s), 0x0101...)`, added to the start offsets and subtracted twice from the lengths, with no per-field branch. Commit [`9307ad6`](https://github.com/cherylaurelia/builders-hackathon/commit/9307ad6): A/B ratios 0.904, 0.920, 0.950 of total run time (together with nibble-table ID packing).
  - [explorer-skp `333fd23`](https://github.com/explorer-skp/buildathon-shopping-sessions/commit/333fd23): "Back to the block scanner: the index version measured 6% slower on x86 twice." The index version (`0a9f36d`: "delimiter index per block with a fixed unrolled write, lines taken N entries at a time") is the same shape as `scan_lines` here. Their block scanner ([`src/scan.rs`](https://github.com/explorer-skp/buildathon-shopping-sessions/blob/d58857c/src/scan.rs)) validates quotes per 64-byte block with a prefix-XOR parity mask and steps once per delimiter.
  - [AbhirajTiwari12](https://github.com/AbhirajTiwari12/shopping-sessions-pipeline/commit/b7d6b76) tried a two-stage batched scanner (`ffa9796`), a gather-based field split (`09d73d6`) and a quote index (`1fed555`), and went back to the line-at-a-time scanner each time. [joshuamuellenbach25 `09fe36c`](https://github.com/joshuamuellenbach25/Databricks---Shopping-Sessions/commit/09fe36c): "Split each line from its comma and quote masks."
  - Counter-evidence: [micah-andelman `b20e383`](https://github.com/micah-andelman/shopping-sessions/commit/b20e383) moved the other way, to a two-stage scanner with bulk positions and mask-based quote validation, as part of the 0.525 to 0.495 s bundle. So the index design is not always slower; what the winners share is quote validation by mask arithmetic on the whole line.
- **Here.** `scan_lines` (`parse.rs:486-654`) spends the audit's 10.8% of instructions in the per-field quote loop at lines 621 to 635, which 79% of event lines take, plus the flatten loop at 541 to 562. The smaller step is to keep the index and replace only lines 602 to 638 with the borrow test on the line's quote and delimiter words (lines under 128 bytes; longer ones keep `split`). The larger step is the full line-at-a-time scanner. Either way a line of 77 bytes needs two 64-bit words, so the masks must be re-derived relative to the line start or shifted from the block masks.
- **Expected gain.** 3 to 8% of wall for the full version, less for the quote test alone.
- **Risk.** Medium to high. The rule to keep: a line passes only if it has exactly N fields and every quote is the first or last byte of a field of length at least 2 that has both; `""` is an empty field; a lone `"` field is malformed. `fuzz_scan` (`parse.rs:675`) already compares against `split` on quote-dense input; add 7-field cases and lines straddling 64 and 128 bytes before changing anything.

### 2.2 Type dispatch by word compare

- **Idea.** Decide `view`/`cart`/`purchase` from the field length and one unaligned 8-byte load, instead of slice pattern matches.
- **Source.** [cherylaurelia `src/main.rs`](https://github.com/cherylaurelia/builders-hackathon/blob/2e4261f/src/main.rs) `fast_event`, lines 2118 to 2127: `match kl { 4 if w as u32 == "view" || w as u32 == "cart" => 0, 8 if w == "purchase" => ..., _ => invalid }`. The same function looks the category up only inside the purchase arm, which is idea 1.1.
- **Here.** `main.rs:622-629`. The audit's microbench priced the type match at 0.5 ns per line, so this is only worth doing while editing those lines for 1.1.
- **Expected gain.** Under 1%.
- **Risk.** None if the length is checked first; the 8-byte load needs the same `ceil` guard as `pack_fast`.

### 2.3 More evidence on reading the input (updates 1.5)

- [AbhirajTiwari12](https://github.com/AbhirajTiwari12/shopping-sessions-pipeline/commit/5dad127) tried `pread` into a huge-page buffer twice (`e3be38a`, `a4ec2b1`) and returned to the file mapping both times. The same repo turned per-chunk pre-mapping off by default (`9bbd0da`).
- [joshuamuellenbach25 `213ce9a`](https://github.com/joshuamuellenbach25/Databricks---Shopping-Sessions/commit/213ce9a): "Return to memory-mapped input; paired Linux container runs favored it over buffered reads."
- [Eagle Eye README](https://github.com/shanmukha-here/eagle-eye-shopping-sessions/blob/9e3c34a/README.md): for cold inputs it tries aligned 2 MiB direct reads with a 4 KiB overlap and uses mappings for cached inputs. Its local board evicts the input before every run, so that team assumed a cold scorer; its best score was 1.01 s.
- Tally: mmap won in three repos (AbhirajTiwari12, joshuamuellenbach25, micah-andelman), `read` won in two (Vibhor7-7 on a cloud VM, cherylaurelia). None of the mmap wins is stated to be on a VM. Idea 1.5 is demoted from the top five; it remains a scoreboard A/B candidate only if a submission slot is spare.

### 2.4 Things other teams tried and reverted (do not spend time here)

- Dedup resolved inside the table with 16-byte rows and no separate phase: reverted by [AbhirajTiwari12 `319b917`](https://github.com/AbhirajTiwari12/shopping-sessions-pipeline/commit/319b917); the similar 16-byte dedup record was rejected by micah-andelman. Idea 1.3 differs (the payload is written once to its final partition and touched again only for losers) and is used by cherylaurelia, explorer-skp and Vibhor7-7 (`src/pool.rs`: "every record has a global 32-bit index, so a later phase can point at one (the dedup phase does, to drop the losing copies)"), but treat it as the riskiest of the top five.
- Half the threads: AbhirajTiwari12 `9088f00`, back to all threads in `13c214a`.
- Two-stage link prefetch (filter word first): explorer-skp `c2a3163`, back to one stage in `f5d71b4`.
- No transparent huge pages: AbhirajTiwari12 `e9056a1`, back to requesting them in `6aaf72c`.

---

## Iteration 3 (2026-11-21): primary sources on input I/O, exit and the sandbox

Sources were opened by a research agent and are linked per claim. Items it could not open are listed at the end of the section.

### 3.1 `background_readahead` asks for 32 MiB per call but the kernel clamps each call

- **Finding.** `posix_fadvise(WILLNEED)` and `readahead(2)` both end in `force_page_cache_ra`, which clamps one call to `max(bdi->io_pages, ra->ra_pages)` pages, submits in 2 MB pieces and does not wait ([mm/readahead.c, v6.6](https://raw.githubusercontent.com/torvalds/linux/v6.6/mm/readahead.c)). `background_readahead` (`parse.rs:338-346`) requests 32 MiB and then advances 32 MiB, so everything past the clamp in each step is never requested. With a clamp in the usual 128 KiB to 1.3 MiB range (the value on the scorer's disk is unknown), the helper pre-reads only a few percent of each file.
- **Fix.** Step by at most the clamp; 512 KiB is safe. This is why cherylaurelia's 2 threads of 512 KiB `readahead` calls worked (entry 1.9). Google's persistent-disk guide recommends I/O sizes of 256 KB or more and, for standard PD, 8 or more parallel sequential streams ([optimizing PD performance](https://docs.cloud.google.com/compute/docs/disks/optimizing-pd-performance)).
- **Expected gain.** Zero with a warm cache; possibly large on a cold first run, where today the parse threads fault the data in themselves.
- **Risk.** None to exactness. `readahead` can block on metadata for files without extents ([man page](https://man7.org/linux/man-pages/man2/readahead.2.html)), which is why it belongs on helper threads.

### 3.2 Mapped file pages are 4 KiB on likely scorer kernels, and exit teardown is on the clock

- Transparent huge pages cover only anonymous memory and tmpfs/shmem ([kernel THP doc](https://docs.kernel.org/admin-guide/mm/transhuge.html)); `filemap_map_pmd` maps a PMD only for a PMD-sized folio ([mm/filemap.c, v6.6](https://raw.githubusercontent.com/torvalds/linux/v6.6/mm/filemap.c)). So the mapped CSVs use 4 KiB PTEs, and in a VM a TLB miss costs up to 24 memory references against 4 native ([Gandhi et al., MICRO 2014](https://scail.cs.wisc.edu/papers/micro14-virtualization.pdf)). That supports the direction of Vibhor7-7's measurement (entry 1.5), not its size: a sequential scan misses the TLB far less than the paper's workloads.
- `MADV_POPULATE_READ` removes the faults but not the PTE setup or the 4 KiB translations ([madvise(2)](https://man7.org/linux/man-pages/man2/madvise.2.html)). Fault-around was measured at about 0.45 µs per avoided fault, 110 ms/GiB, in 2014 ([LWN](https://lwn.net/Articles/588802/)).
- `do_exit` runs `exit_mm()` before `exit_notify()`, so whoever waits on the container sees the exit only after the address space is torn down ([kernel/exit.c](https://raw.githubusercontent.com/torvalds/linux/v6.6/kernel/exit.c)). When PID 1 of a PID namespace exits, the kernel kills and reaps everything in it first ([kernel/pid_namespace.c](https://raw.githubusercontent.com/torvalds/linux/v6.6/kernel/pid_namespace.c)), so handing teardown to a child cannot hide it. This confirms the "fork-and-exit" entry on the `speedups.md` skip list.
- Unmapping a fully mapped 13.8 GB file took 138 to 199 ms and did not improve with threads ([1BRC discussion #138](https://github.com/gunnarmorling/1brc/discussions/138)). Scaled, that is 20 to 27 ms at 20M and 55 to 80 ms at 60M, single-threaded, after the output is written. The audit measured 17 to 24 ms at 20M, which agrees.
- **Consequence for 1.5.** A `pread` path has no file mapping, so it saves this teardown (about 3 to 5% of a 0.6 s run) in addition to any translation gain. That is an argument for the L2-block variant that the mmap-versus-read tally in 2.3 does not capture, since those teams compared in-process time.

### 3.3 If the sandbox is gVisor, mmap is the wrong choice

- The runtime is unknown. Under gVisor a fault on a file mapping makes the sentry allocate memory, copy the file contents into it and issue a host `mmap`, after which the application faults again ([gVisor mm README](https://raw.githubusercontent.com/google/gvisor/master/pkg/sentry/mm/README.md)): a copy plus two faults, where `pread` pays one copy.
- gVisor syscalls that reach the file gofer cost about 45 µs against 0.3 µs under runc, and read throughput is poor at 4 to 16 KB blocks and competitive from 64 KB to 1 MB ([Young et al., HotCloud 2019](https://files.usenix.org/s3fs-public/cdn/hotcloud19-paper-young.pdf)).
- **Here.** `pread` in 256 KB blocks is the one input strategy that is reasonable under both runtimes: about 8,000 calls at 20M. The 1,010 `madvise` calls per run are cheap under runc and of unknown cost under gVisor.
- **Risk.** None to exactness beyond what 1.5 lists. Whether the scorer uses gVisor is not established by any source; Vibhor7-7's notes call the scorer's sandboxes `runc`, without evidence.

### 3.4 Huge pages for the arena

*Corrected by [12.1](#121-correction-to-34-mimalloc-v3-advises-huge-pages-by-default): the mimalloc heap is also advised, so the second sentence of the first bullet below is wrong for this build.*

- GKE documents `madvise` as the default THP mode ([LinuxNodeConfig reference](https://docs.cloud.google.com/python/docs/reference/container/latest/google.cloud.container_v1.types.LinuxNodeConfig.TransparentHugepageEnabled)); upstream defaults to `always`. `Arena::new` (`mem.rs:32`) already calls `MADV_HUGEPAGE`, so both cases are covered. The vectors outside the arena (`Scratch`, `Acc.users`, the Parquet buffers, `KMap` slots) come from mimalloc and get huge pages only under `always`.
- No action unless the output buffers move into the arena, which the queued "reserve or stream Parquet buffers" item would make unnecessary.

Not opened or not verified: the readahead clamp on GCP persistent disk; the THP setting on the scorer image; gVisor's support for `MADV_POPULATE_READ`; any measurement of read against mmap on an n2 VM; huge-page zeroing cost (only a search snippet, so no number is recorded here).

---

## Iteration 4 (2026-11-21): primary sources on per-line decode cost

Opened by a research agent; links per claim. Gain figures are arithmetic on the sources' numbers, not measurements of this code. No single item here is worth more than about 5% of the line.

### 4.1 Three-test quote rule with PEXT (a simpler form of 2.1's test)

- **Idea.** With `S` = separator bits including the line's newline, `q` = quote bits, `starts = (S << 1) | 1` and `ends = S >> 1`, a line is well quoted exactly when all three hold: `q & !(starts | ends) == 0` (no stray quote), `q & starts & ends == 0` (no one-byte `"` field), and `pext(q, starts) == pext(q << 1, S)` (a field opens iff it closes). Bit j of the first PEXT says whether field j is quoted, which gives the branch-free strip.
- **Source.** Derived by the agent, with no published measurement. It was checked by brute force against the `split` rule on all 88,572 strings over `{a , "}` up to 10 bytes with no mismatch, in a Python model of a single mask word; the script is at `/private/tmp/claude-501/-Users-nimaibhat-Documents-databricks-chall/273c3dc9-8aa1-4164-a68d-59b78a9a199a/scratchpad/chk/q.py`. cherylaurelia's borrow-subtraction form (entry 2.1) is an independent statement of the same rule and is in a scored repo.
- **Here.** Replaces the quote-count word loop and the 14 ring lookups per event line at `parse.rs:602-638`. Lines span two or three ring words, so the masks must be assembled into 128 bits per line; keep the current loop for longer lines.
- **Expected gain.** 3 to 5% of the line.
- **Risk.** Medium. The two-word Rust version is untested; extend `fuzz_scan` to N = 7 first.

### 4.2 One branch for F1 and F2

- **Idea.** `parse_ts_fast` branches three ways on a shuffled 58/10/32 mix. Merge F1 and F2 into one path: the same `civil_swar`, with the offset computed from the tail word and masked to zero for `Z`. That leaves a single 68/32 branch against F3.
- **Sources.** A 50% unpredictable branch cost 15 cycles against 3.3 predictable on Skylake ([Lemire, 2019](https://lemire.me/blog/2019/10/15/mispredicted-branches-can-multiply-your-running-times/)). Removing about one miss per two rows cut cycles 15.4% while instructions rose, in [Wuerthinger's 1brc-steps](https://github.com/thomaswue/1brc-steps) v10. Counter-evidence: another 1BRC author cut misses from 148M to 4.1M with no runtime gain ([curiouscoding](https://curiouscoding.nl/posts/1brc/)), and Vibhor7-7's branch-free timestamp experiments 5 and 11 measured 0% (entry in 1.2's source).
- **Here.** `parse.rs:922-941`. This is smaller than the `TSBL` design, which also folds F3 in and whose only measurement so far is invalid (see the finding at the top of this file).
- **Expected gain.** 1 to 3% of the line, sign uncertain.
- **Risk.** Low: the exact `parse_ts` fallback stays, and `ts_fuzz` already covers both forms.

### 4.3 Measured cost of small-set string checks (supports 2.2)

- On Ice Lake, load-8-bytes-and-compare took 2.6 ns per string against 18 ns for direct compares and 7.1 ns for gperf ([Lemire, 2022](https://lemire.me/blog/2022/12/30/quickly-checking-that-a-string-belongs-to-a-small-set/)). The audit priced the type match here at 0.5 ns per line, so the compiler is probably already doing this for `main.rs:622-629`; worth at most 1 to 2%.

### 4.4 Separator index by compress instead of a `tzcnt` loop

- **Sources.** On Skylake-X a `tzcnt` loop took 4.4 cycles per index, simdjson's unrolled loop 3.6, and `vpcompressd` 2.4 with no mispredictions ([Lemire, 2022](https://lemire.me/blog/2022/05/06/fast-bitset-decoding-using-intel-avx-512/)). simdjson's Ice Lake kernel uses one `_mm512_maskz_compress_epi8` per 64-byte block ([`src/icelake.cpp`](https://raw.githubusercontent.com/simdjson/simdjson/master/src/icelake.cpp)).
- **Here.** `parse.rs:524-562`. A block holds about 6 separators, so the gain is at most 2% of the line. The byte form needs VBMI2, which Cascade Lake lacks, and any 512-bit form pays the frequency cost in 4.5.
- **Verdict.** Not worth a runtime-dispatched kernel.

### 4.5 512-bit instructions on Cascade Lake cost more than the scanner can gain

- On Skylake-SP class cores one light 512-bit instruction drops the core from 3.2 to 2.8 GHz, with about 20 µs lost per transition and 680 µs of hysteresis; 256-bit light instructions cause no change ([Downs, 2020](https://travisdowns.github.io/blog/2020/01/17/avxfreq1.html)). Ice Lake client parts lose 100 MHz with one core active and nothing with more ([Downs, 2020](https://travisdowns.github.io/blog/2020/08/19/icl-avx512-freq.html)); server parts were not measured there.
- **Here.** The `XCPU=x86-64-v4` path in `masks64` (`parse.rs:407-415`) saves well under 1% of the line's instructions. The track page says scorers are "Cascade Lake or newer", so the default `x86-64-v3` build is the right one; this closes the AVX-512 scanner question unless the scoreboard shows otherwise.

### 4.6 Closed by this iteration

- **Two lines interleaved per thread.** 1BRC gains came from codes at IPC near 2 and about 20 cycles per row ([curiouscoding](https://curiouscoding.nl/posts/1brc/): 1.34 to 1.23 s with four rows in flight). This line is about 1,400 instructions, so the limit is instruction count, not latency chains.
- **SWAR price parse.** Only 3% of lines reach `parse_price`; under 0.2%.

Not opened: Muła's articles (certificate error), the simdjson and simdcsv papers, Agner Fog's manuals. No source measures licence behaviour on Cascade Lake or Ice Lake Xeon specifically.

---

## Iteration 5 (2026-11-21): Parquet readers, encodings and write syscalls

Opened by a research agent; links per claim. Nothing was built, so size figures are arithmetic from the audit's 42.9 bytes per row and 82.4% null `top_category`.

### 5.1 The likely reader accepts every size-reducing layout

- The repo's own `scripts/compare.py` and `scripts/run_fixtures.py` (identical in all three starter zips) read with `pl.read_parquet`, accept any integer dtype and cast before comparing, so INT32 against INT64 cannot fail there. The starters pin `polars==2.1.0`, `pyspark==4.0.1` and `duckdb==1.5.6`. The hidden scorer may differ from these scripts.
- Polars, Arrow C++, parquet-java, arrow-rs and DuckDB all read PLAIN, dictionary, `DELTA_BINARY_PACKED`, `DELTA_LENGTH_BYTE_ARRAY` and `DELTA_BYTE_ARRAY` ([Parquet implementation status](https://parquet.apache.org/docs/file-format/implementationstatus/); polars integer decoder [source](https://raw.githubusercontent.com/pola-rs/polars/main/crates/polars-parquet/src/arrow/read/deserialize/primitive/integer.rs)). Spark's vectorized reader rejected DELTA pages until 3.3.0 ([SPARK-36879](https://issues.apache.org/jira/browse/SPARK-36879)); the pinned 4.0.1 is past that.
- Stay on v1 data pages: Arrow's docs warn that not all readers support v2 ([Arrow C++ Parquet](https://arrow.apache.org/docs/cpp/parquet.html)).
- **Order of safety:** INT32 PLAIN, then `RLE_DICTIONARY` (readable everywhere, old Spark included), then `DELTA_BINARY_PACKED`.

### 5.2 What each step saves in `write_users` (`out.rs:33`) and `pq::write` (`pq.rs:119`)

| Step | Saving | Risk |
|---|---|---|
| `sessions`, `events` as INT32 PLAIN | 8 B/row, about 19%, and less CPU | Low; already queued |
| `revenue_cents` as INT32 | 4 B/row | Needs a per-file maximum check, since a whale's daily revenue can pass 2^31 cents |
| `top_category` as `RLE_DICTIONARY` | about 2 B/row | Low |
| `DELTA_BINARY_PACKED` on `day`, `sessions`, `events` | about 12 B/row down to 1 to 2 | Low on the pinned readers; fatal on Spark 3.2 or older |
| `user_id` as `RLE_DICTIONARY` | Wins only if a person averages more than about 1.3 rows per file | Measure the ratio first; at 20M it is 2,242,393 rows over about 667,000 people, so it should win |

Total expected gain is 1 to 2% of wall (entry 1.8), so only the INT32 step is clearly worth the risk.

### 5.3 Layout rules a hand-written writer must get right

From [Encodings.md](https://raw.githubusercontent.com/apache/parquet-format/master/Encodings.md) and [parquet.thrift](https://raw.githubusercontent.com/apache/parquet-format/master/src/main/thrift/parquet.thrift):

- **Dictionary chunk.** A dictionary page (`PageHeader.type = 2`, field 7 `DictionaryPageHeader{num_values, encoding}`) with PLAIN values comes first; Arrow C++ rejects a dictionary after the data page and any dictionary-page encoding other than PLAIN or PLAIN_DICTIONARY. The data page is v1 with encoding `RLE_DICTIONARY` (8): definition levels as today, then one bit-width byte, then hybrid runs with no length prefix. Only non-null rows have an index. `ColumnMetaData.dictionary_page_offset` (field 11) points at the dictionary page header, and both total sizes must include both pages and both headers, because polars reads `dictionary_page_offset .. + total_compressed_size`. Encodings list: `[PLAIN, RLE, RLE_DICTIONARY]`.
- **`DELTA_BINARY_PACKED`.** Header: `ULEB(block size, multiple of 128)`, `ULEB(miniblocks per block)`, `ULEB(total count)`, `zigzag(first value)`. Each block: `zigzag(min delta)`, one bit-width byte for every miniblock including unused ones, then each used miniblock padded to its full value count (a multiple of 32). Values are `delta - min_delta`, packed LSB first.
- **Common mistakes that readers reject:** a length prefix on dictionary indices; a missing bit-width byte; counting nulls as indices; sizes that leave out the dictionary page; omitting bit-width bytes of unused miniblocks; not padding the last miniblock.

### 5.4 Review of the current footer

- No spec violation found: required fields are present, STRING and DATE carry both converted and logical types, and definition levels are declared RLE, which old Spark requires.
- One hazard: when thread 0 has no rows, `write_all` (`out.rs:128`) still writes `part-0.parquet` with a zero-row row group. Spark's column reader can throw on a chunk with zero values; writing zero row groups, or skipping the file when another part exists, is safer. This cannot happen at 20M, only on tiny inputs such as fixtures, which already pass with polars.
- Page size: one page per column will be about three times larger at 60M. The spec's only limit is the i32 size field, but polars has an over-allocation guard whose default the agent could not confirm. Read a 60M output back with polars 2.1.0 before the final, or split columns into pages of a fixed row count as cherylaurelia does (16K rows, `cfedecc`).

### 5.5 Write syscalls

- Buffered write cost scales with bytes and pages, not call count: `generic_perform_write` loops per folio ([mm/filemap.c](https://raw.githubusercontent.com/torvalds/linux/master/mm/filemap.c)). One `write` per file is as good as any chunking.
- ext4 buffered writes take the inode lock ([fs/ext4/file.c](https://raw.githubusercontent.com/torvalds/linux/master/fs/ext4/file.c)), so parallel writers need separate files. Keep one file per thread.
- A writer is throttled only past the midpoint of the background and hard dirty thresholds, 10% and 20% of available memory by default ([sysctl/vm](https://docs.kernel.org/admin-guide/sysctl/vm.html)). 100 to 300 MB against 24 GB does not reach it unless the host sets a small `dirty_bytes`; writeback then runs in flusher threads and is not charged to the process.
- Nothing supports `fallocate` or mmap output. The levers are fewer bytes and fewer copies: write column pages straight to the file instead of through the `page` and `file` vectors (`pq.rs:120-175`), which is the queued item.

Not opened: parquet-mr source, polars' `max_page_size` default, any measured write benchmark, anything specific to overlayfs.

---

## Iteration 6 (2026-11-21): primary sources on write-combining (design rules for idea 1.2)

Opened by a research agent; links per claim. The sources support the plan in 1.2 and settle its open parameters.

### 6.1 Stage size: start at 4 cache lines per partition, sweep 2

- Schuhknecht et al. measured the best streamed buffer at 6 lines for 256 partitions, 4 for 512, 2 for 1024 and 1 from 2048 up, and advise fewer than 8 lines ([PVLDB 2015](https://www.vldb.org/pvldb/vol8/p934-schuhknecht.pdf), Table 2; 8-byte tuples, 32 KB L1, 256 KB L2). Streaming was 1.18x faster than buffering alone on the partition step at 512 partitions, and 0.98x to 1.0x at 128 or fewer.
- Wassenberg and Sanders, Balkesen et al. and Polychroniou and Ross use exactly one line ([arXiv 1008.2849](https://arxiv.org/pdf/1008.2849); [TKDE version](https://cs.uwaterloo.ca/~tozsu/publications/other/hash-joins-tkde-final.pdf) §6.9; [SIGMOD 2014](https://www.cs.columbia.edu/~orestis/sigmod14I.pdf) §3.2.1). One line holds only two `ERec`s, so that is too small here.
- **Here.** `N = 8` for `ERec` and `CRec` (4 lines, 128 KB of stage per thread at 512 partitions), with `N = 4` as the sweep point; `N = 8` or `16` for `SRec`. This matches what Vibhor7-7 converged on by experiment (entry 1.2).

### 6.2 Keep the two halves of a line together, and fence once

- Cascade Lake has 10 write-combining buffers and Ice Lake 12 by Intel's manual (§3.6.9, [248966-050](https://web.archive.org/web/2024id_/https://www.intel.com/content/dam/www/public/us/en/documents/manuals/64-ia-32-architectures-optimization-manual.pdf)); a Stack Overflow answer disputes the Cascade Lake figure and says they are shared between hyperthreads ([question 49959963](https://stackoverflow.com/questions/49959963)). The same buffers serve L1 load misses.
- If other traffic closes a buffer before its line is complete, the line leaves as partial writes at a fraction of the data rate (Intel §11.5.5, §3.6.11). So read the whole stage first, then issue the two `_mm256_stream_si256` of each line back to back with no parsing load between them. Whether one 64-byte AVX-512 store avoids this window is not established by any source.
- A fence is needed only before another thread reads the data (Intel §9.4.1.4; [Cordes](https://stackoverflow.com/questions/35516878)). One `_mm_sfence` per thread after `finish`, before the `par_for` task returns; none per flush.
- `_mm256_stream_si256` faults on a destination that is not 32-byte aligned. Arena blocks and the heap fallback in `Arena::block` (`mem.rs:72`, 64-byte alignment) both qualify.

### 6.3 Where not to stream

- Non-temporal stores pay only when the destination would miss L2 and is not read again before eviction ([Agner Fog](https://www.agner.org/optimize/optimizing_cpp.pdf)); do not mix them with ordinary writes to the same area.
- **Here.** Stream the parse scatter (`parse_events`, `parse_clicks`) and dedup pass 2, whose outputs are hundreds of MB. Keep the plain `push` for the `mv` lists in `resolve_partition` (`main.rs:682`) and for the links scatter, which are small and read back soon.
- On Ice Lake, ordinary stores skip the read-for-ownership only near memory-bus saturation, seen at 29 or more cores on a Xeon 8358 ([Hager](https://blogs.fau.de/hager/?p=8997)); with 4 cores that does not apply, so streaming still matters there.

### 6.4 Alternatives closed

- **Prefetch-for-write of the next slot** helps from 4,096 partitions and hurts below 1,024 (Schuhknecht §3.3). Not for 512.
- **Two-pass partitioning.** A single buffered pass stays flat to 2^14 or 2^15 partitions while the buffers fit the cache ([Schuh et al., SIGMOD 2016](https://bigdata.uni-saarland.de/publications/An%20Experimental%20Comparison%20of%20Thirteen%20Relational%20Equi-Joins%20in%20Main%20Memory.pdf); Balkesen §6.9). 9 bits needs one pass.
- **`movdir64b`, `clzero`, `rep movsb`.** The first is absent before Sapphire Rapids and the second is AMD-only ([GCC x86 options](https://gcc.gnu.org/onlinedocs/gcc/x86-Options.html)); `rep movsb` is slower than non-temporal copies and only competitive from 256 bytes.

Not found: any measurement of non-temporal stores under hyperthread contention, in a VM, or Zen against Intel beyond Vibhor7-7's A/B.

---

## Iteration 7 (2026-11-21): sorting, union-find, dedup tables and hyperthreads

Opened by a research agent; links per claim. The primary sources have few numbers at this workload's sizes, so the two strongest leads come from reading `src/` against them.

### 7.1 Sort 8-byte keys per person

- **Idea.** After the counting-sort scatter, every record in a group has the same `person`. Scatter only `(ts as u64) << 32 | pc as u64` into `ev` and sort plain `u64`s.
- **Sources.** Rust's `sort_unstable` uses insertion sort up to 20 elements, and its sorting-network small sort is only used when `size_of::<T>() <= 8` ([core sort source](https://doc.rust-lang.org/src/core/slice/sort/unstable/mod.rs.html), [smallsort.rs](https://doc.rust-lang.org/src/core/slice/sort/shared/smallsort.rs.html)); a 16-byte `SRec` never takes the network path. Sorting networks beat insertion sort by at least 1.76x for 6 to 16 elements of (key, reference) pairs ([Bingmann et al.](https://ar5iv.labs.arxiv.org/html/2002.05599)).
- **Here.** `Scratch.ev` becomes `Vec<u64>`; the scatter at `main.rs:820-828` writes the packed word; `sort_unstable_by_key(|r| r.ts)` at `:895` becomes `sort_unstable()`; the sweep at `:911-943` reads `ts = (w >> 32) as u32` and `pc = w as u32`. `person` already comes from `gkeys[g]` (`:901`). Insertion sort then moves half the bytes, groups of 21 or more get the network path, and `ev` drops to 0.3 to 1 MB per partition, which matters because two hyperthreads share each L2.
- **Expected gain.** 10 to 25% of the sessions phase, so 2 to 4% of wall. This is the agent's estimate, not a measurement.
- **Risk.** None to exactness. Sorting the whole word orders equal-`ts` events by `pc` instead of arbitrarily; events with equal `ts` always share a session, and the count, revenue and category sums do not depend on order.

### 7.2 Whale groups

- At 1M keys vqsort reaches 628 MB/s on AVX2 against 118 MB/s for `std::sort` ([Blacher et al.](https://ar5iv.labs.arxiv.org/html/2205.05982)), but Rust's ipnsort is already about 1.6x pdqsort and catches Intel's AVX-512 sort at 1M ([sort-research-rs](https://github.com/Voultapher/sort-research-rs/blob/main/writeup/intel_avx512/text.md)). With 7.1 a whale is a plain `u64` sort, where vqsort would give roughly 2x on 5 to 10% of events: about 1% of wall for a C++ dependency. `speedups.md` already rejected this; the numbers confirm it.
- A hand-written 3-pass LSD radix on `ts` for groups above about 1,000 elements avoids the dependency; ska_sort falls back to comparison sort below 128 elements ([Skarupke](https://probablydance.com/2016/12/27/i-wrote-a-faster-sorting-algorithm/)). No source measures it at this size; micah-andelman's partition-wide radix gained about 10% of the person stage (entry 1.6).

### 7.3 Union-find: prefetch before changing the algorithm

- **Sources.** Cache misses decide union-find performance, and checking the immediate parent first improves every variant ([Alistarh et al.](https://ar5iv.labs.arxiv.org/html/1911.06347)). Rem's algorithm with splicing averaged 28% faster than rank with path halving on graphs of 64K to 1.5M vertices ([slides](https://api.sambuz.com/doc/experiments-on-union-find-algorithms-for-the-disjoint-set-presentation-696884) for Patwary et al.; the paper itself was not opened).
- **Here.** The serial loop at `main.rs:231-243` costs 40 to 120 ns per edge by the audit's timings. That is high for a `parent` array of about 7 MB: ids are numbered in hash-shard order while `ipairs` are in file order, so each edge makes two random loads. Cheapest first:
  1. Prefetch `parent[a]` and `parent[b]` 8 to 16 edges ahead. Most finds are 0 or 1 hops, so this should hide most of the misses.
  2. Split the `t_uf` timer. It also covers first touch of about 35 MB (`parent`, `all_keys`, `name`) and the random `name[r]` pass at `:249-256`, which may be the larger part.
  3. Only then consider parallel CAS unions; `speedups.md` rejected those and nothing here changes that.
- **Expected gain.** 1 to 3% of wall, less once links run beside the parse (queued).
- **Risk.** None for the prefetch.

### 7.4 Dedup: lookahead prefetch is not a reliable win at 2 to 4 MB

- On a 2 MB table that fits the last-level cache, group prefetching was 32% slower than no prefetching and only AMAC gained (21%); the large gains (2.8x to 4.3x) are at 2 GB ([Kocberber et al.](https://www.vldb.org/pvldb/vol9/p252-kocberber.pdf)). Out-of-order execution already overlaps about 5 probes ([Cimple](https://ar5iv.labs.arxiv.org/html/1807.01624)).
- **Here.** A prefetch in pass 1 of `dedup_partition` is worth 0 to 20% of that phase and could be negative, which agrees with `speedups.md`. cherylaurelia's dedup prefetch (`838452e`, entry 1.7) gained in a design with a far smaller table. The better lever is footprint: the 16-byte keys of idea 1.3, or an 8-byte slot holding a 32-bit tag and a 32-bit record index, verified against the record on a tag match.

### 7.5 Hyperthreads

- Radix joins gain little from SMT because siblings share caches and TLBs, and where two threads share an L2 its effective size per thread is halved ([Balkesen et al., ICDE 2013](https://www.comp.nus.edu.sg/~hebs/course/cs6284/papers/Balkesen-icde2013.pdf)). No source gives a measured sizing rule.
- **Here.** Count 512 KB, not 1 MB, as the per-thread L2 budget on Cascade Lake. That is one more argument for 7.1 and for the smaller dedup records in 1.3, and against raising the write-combining stage past 4 lines (entry 6.1).

Not opened: the Patwary et al. paper, absolute cycle tables in Bingmann et al., single-thread tables in ConnectIt.

### 7.6 Generator source (read directly, `generator.zip`)

- IDs are a one-letter prefix (`e`, `u`, `k`) plus lowercase base 36 of a scrambled 40-bit number (`text.rs` `base36`, `scramble`), so at most 9 bytes; products are `p<n>` and campaigns `c_<n>` (`main.rs` `write_lookups`). Lines are shuffled within each file (`write_part`), so there is no time or user locality to exploit, and a last-value cache is useless while a table keyed by date still works (idea 1.4).
- `ingest_ts` is whole seconds and usually minutes after `ts` (`user.rs` `ingest`), so both timestamps of a line usually share a date. With idea 1.4 the second lookup of a line almost always hits the entry the first one just used.
- Nothing else in the generator supports a new specialised path; the dirt rates are per seed, as the rules say.

---

## Iteration 8 (2026-11-21): fixed serial work at start-up

From reading `src/` against the audit and competitor commits. At a 0.6 s target every 6 ms of serial work is 1%.

### 8.1 Catalog load is about 60M serial instructions before anything parallel starts

- **Evidence.** The audit's callgrind run attributes about 2.5% of 2.395G instructions on the 1M set to catalog load (`read_small_csv`, `Catalog::finish`, SipHash), which is about 60M instructions. `products.csv` is always 50,000 rows and `campaigns.csv` 2,000 whatever the event count (rules, "The generator"), so this cost is fixed: roughly 15 to 20 ms, on one thread, at 20M and at 60M alike.
- **Where it goes.** `read_small_csv` (`parse.rs:382`) allocates three vectors per row. `main.rs:349-360` clones every category name, then does a SipHash `HashMap<Vec<u8>, u16>` lookup per product. `Catalog::insert` (`keys.rs:359`) copies each id into `ids`. `Catalog::finish` (`keys.rs:380-402`) runs `canon_split` over every id twice and builds a `HashMap<Vec<u8>, (usize, u64)>` with a `to_vec` per id.
- **Sources.** [cherylaurelia `2f45a4d`](https://github.com/cherylaurelia/builders-hackathon/commit/2f45a4d): "products/campaigns tables on a side thread while the links are built (7 ms off the serial start)", A/B ratio 0.961. [Th3r0n `81a7e86`](https://github.com/Th3r0n/shopping-sessions/commit/81a7e86): "Faster products.csv loading: no per-row allocations, pre-sized map". [explorer-skp `196faf2`](https://github.com/explorer-skp/buildathon-shopping-sessions/commit/196faf2): "Load the product and campaign tables while the pool parses link lines". Counter-evidence: micah-andelman rejected parallel small-table load, but bundled with smaller link chunks that slowed the link parse.
- **Here.** Two independent steps:
  1. Run the catalog block (`main.rs:348-373`) on a scoped side thread started before `Mapped::open` and `build_links`, and join it before `Ctx` is built at `:389`. `build_links` reads neither catalog. The parse threads need both, so the join stays before phase 2.
  2. Remove the per-row allocations: split lines in place over the file buffer, map category names through a 20-entry linear scan or a small sorted array instead of SipHash, and have `finish` count prefixes without `to_vec` (in generated data there is one prefix per file).
- **Expected gain.** 7 to 20 ms of wall, so 1 to 3% at 20M and under 1% at 60M.
- **Risk.** Low. Step 1 changes no logic. Step 2 must keep the exact semantics that `cat_tests::dense_exact` checks: last row wins for a repeated product id (`overwrite = true`), first row wins for a repeated campaign id, and category ids follow byte-wise name order with `unknown` included, because `flush_user` breaks ties on the smaller category id (`main.rs:1016`).

### 8.2 Loads that are re-read per event or per block

- `p1_bits()` and `p2_bits()` are relaxed atomic loads inside the per-line closures (`main.rs:634`, `:656`, and `:735` in dedup pass 2). The compiler cannot hoist them across the opaque `BList::push` slow path. Read them into locals before `scan_lines`.
- `flat16()` (`parse.rs:528`) is a `OnceLock` read per 64-byte block inside `scan_lines`; hoist it to the top of the function, as `ts_bl()` already is in `parse_events`.
- **Expected gain.** Under 1%; listed because it is free while those lines are being edited for ideas 1.1 and 1.2.
- **Risk.** None.

---

## Iteration 9 (2026-11-21): a drop-in dedup that needs no key table

### 9.1 Two-bit counters instead of a key table

- **Idea.** Only 2 to 4% of event ids repeat, so the work is finding them. Scan the partition against an array of two-bit saturating counters indexed by key hash ("seen once", "seen again"). Scan again and collect the records whose slot was hit more than once. Sort only those by `(event_id, ingest)`; in each run of one id the first wins and the rest are losers.
- **Source.** [Vibhor7-7 `src/dedup.rs`](https://github.com/Vibhor7-7/shopping-sessions/blob/1359a83/src/dedup.rs) (whole file, 90 lines). It sizes the array at 16 slots per record ("keeps chance collisions near 6%"), takes the slot from the hash bits just below the partition bits, and marks a loser by writing a dropped marker into its event record. A winner that is an invalid copy has no event record, and every later copy is still dropped, which is the dedup-before-validation rule. Introduced in `b58dc53` (experiment 14, -1.5% total together with a link-table change). cherylaurelia used the same scheme ("a cache-sized bit filter finds the keys seen twice; those few are sorted", `src/main.rs` header) at 0.24 CPU-s for the phase at 20M before replacing it with the tag-line table (0.19, then 0.149 CPU-s; entry 1.3). micah-andelman `5c2f4b5` is a third instance ("1M-bit filter per bucket; only possibly-repeated event ids go through the exact table").
- **Here.** This replaces only pass 1 of `dedup_partition` (`main.rs:708-728`); the `dead` bitmap and pass 2 stay as they are, so it can ship before idea 1.3.
  - Counters: `(n * 16).next_power_of_two()` slots at 2 bits. For a 42,000-record partition at 20M that is 1,048,576 slots or 256 KB; at 8 slots per record it is 128 KB with about 12% chance collisions. Either fits L2, against the 2 MB `KMap<u64>` now (4 MB at 60M).
  - Slot: `(hk(eid) << p1_bits()) >> (64 - slot_bits)`, the bits below the partition bits.
  - Scan 1 over the block lists updates the counters. Scan 2 pushes `(eid, ingest, i)` for every record whose slot says "seen again". Sort that list by `(eid, ingest, i)`; in each run of equal `eid`, set the `dead` bit of every entry after the first.
  - Sorting on `i` last keeps the first-seen copy among equal `ingest` values, which is exactly what the current `r.ingest < cur` test does (`main.rs:717`), so the output is bit-identical even for copies that tie.
- **Expected gain.** Pass 1 stops missing the cache (the audit charges `dedup_partition` with 24.6% of all L1 read misses for 4% of instructions), at the price of one extra sequential read of the partition. Estimate 30 to 50% of the dedup phase, 3 to 5% of wall, more at 60M where the current table is 4 MB.
- **Risk.** Low. All copies of an id share a slot, so none can be missed; false suspects are harmless because the sort separates them by `eid`. The existing gate (fixtures E-series, dev answer, 20M cross-check at `NCPU` 1, 2 and 8) covers it. One thing to check by hand: a partition where one id makes up a large share of records (cherylaurelia tested "one event_id on 1/3 of all events") makes the suspect list long, which is slow but still correct.

---

## Iteration 10 (2026-11-21): model checks of two proposed rules, and what the scoring formula implies

These are checks of the logic in scratch Python models, not builds or benchmarks of this repo. They show the rules are right; they say nothing about a Rust implementation of them.

### 10.1 Both quote-validity formulas match the dialect rule

- **What was checked.** The reference is the rule `split` implements (`parse.rs:10`): each field has no quote, or is at least 2 bytes with exactly two quotes, first and last. Against it: cherylaurelia's borrow form (entry 2.1) and the three-test PEXT form (entry 4.1), both on masks wider than 64 bits.
- **Result.** 0 mismatches over all 265,720 strings of length 0 to 11 over `{a , "}` plus 300,000 random lines of 60 to 140 bytes (half of them 7-field lines with 30% quoted fields and occasional stray quotes), 565,720 cases in all. Script: `/private/tmp/claude-501/-Users-nimaibhat-Documents-databricks-chall/273c3dc9-8aa1-4164-a68d-59b78a9a199a/scratchpad/chk2/q.py`.
- **Meaning for 2.1 and 4.1.** The mask rule is safe to implement for lines up to 128 bytes. What remains untested is the Rust two-word carry handling (the `<< 1` across words and the borrow between words), which `fuzz_scan` must cover with lines that straddle byte 64.

### 10.2 The 30-minute bin rule matches sort-based sessionization

- **What was checked.** Sessions (start, event count) from sorting and splitting on gaps over 1,800,000 ms, against the bin rule of entry 1.6: one bin never splits, adjacent bins split exactly when `min(later) - max(earlier) > GAP`, bins further apart always split.
- **Result.** 0 mismatches over 200,000 random people of 1 to 40 events, with a quarter of gaps exactly 30:00.000, a quarter 30:00.001, and repeated timestamps. Script: `/private/tmp/claude-501/-Users-nimaibhat-Documents-databricks-chall/273c3dc9-8aa1-4164-a68d-59b78a9a199a/scratchpad/chk2/bins.py`.
- **Meaning for 1.6.** The session boundaries are exact, including the two boundary gaps the generator plants. The untested part is still the purchase-to-session join for `top_category` and revenue.

### 10.3 The score is the middle of three runs, so a cold cache matters less than it looked

- The track page (copied in [cherylaurelia `docs/track.md`](https://github.com/cherylaurelia/builders-hackathon/blob/2e4261f/docs/track.md)) says a run under 20 s "is timed 3 times and scored on the middle time", each in a new container, and the final is timed 3 times, then 5 times for the fastest entries.
- If the dataset stays on the same machine between the repeats, at most the first run sees a cold page cache, and the middle of three is then a warm run. So cold-cache work (entries 1.9 and 3.1) can only matter if the scorer drops caches between runs or the three runs land on different machines, neither of which any source establishes. Eagle Eye assumed cold runs and tuned for them; its best score was 1.01 s.
- **Consequence.** Rank warm-path CPU work above everything cold-cache. The readahead step fix in 3.1 is still worth its three lines because it costs nothing warm.

---

## Iteration 11 (2026-11-21): what other teams learned about the scorer itself

Sources: notes committed in a second batch of public repos (arnabxero, joch44, jklein07, TejasviniChawla, axhiao, niranjankumarnk, sonalrajapaksha), read from shallow clones. These are other teams' reports; none was verified here.

### 11.1 The scorer, as reported from diagnostic submissions

- [arnabxero `STATUS.md`](https://github.com/arnabxero/xero-shopping-sessions/blob/93f1173/STATUS.md) ("What the scorer looks like", from two diagnostic builds): Xeon at 2.80 GHz, Cascade Lake (AVX-512 F/BW/VL/VNNI), 8 CPUs by affinity and by cgroup quota, host with 32 vCPUs and 128 GB, kernel 7.0, THP in `madvise` mode. **`/data` is tmpfs**, so inputs are always in RAM; `/output` and `/scratch` are ext4. On the dev set, about 50 ms pass from container exec to `main` and about 75 ms in the process.
- How they got it: a submission that fails the dev check is the only case in which the track page shows a submission's log ([Vibhor7-7 BENCH.md](https://github.com/Vibhor7-7/shopping-sessions/blob/1359a83/BENCH.md), "The probe"). A rejected submission does not change the best score but uses one of the two hourly slots. This repo's binary already prints per-phase times and `rusage` to stderr, so one deliberately failing submission would return this repo's own phase split on the scorer for the 1M dev set. That is the user's call, not something to do unasked.
- [jklein07 `HANDOVER.md`](https://github.com/jklein07/shopping-sessions/blob/c631cd1/HANDOVER.md): times on a 4-core, 8-thread x86 laptop under Docker tracked the board closely (1.35 s local against 1.44 s on the board), "so the board time is almost all our own compute; the fixed part is small".
- [joch44 `NOTES.md`](https://github.com/joch44/shopping-sessions/blob/0e62970/NOTES.md): on an Ice Lake 8370C VM, a `FROM scratch` container that only reads the 20M input warm takes 0.24 s; a warm 8-thread newline count runs at about 14 to 15 GB/s, which is the memory-bandwidth ceiling from 4 threads up. Their builds score about 12% faster on the grader than on that VM.

### 11.2 What the scorer facts change

- **No cold cache.** With `/data` on tmpfs nothing is ever read from a disk. `background_readahead` (`parse.rs:328`) can only cost a thread; arnabxero removed theirs (`93f1173`: "No readahead thread when every input is mapped"). Entries 1.9 and 3.1 are closed.
- **Reading tmpfs.** A mapping of tmpfs is zero-copy but still 4 KiB PTEs unless the mount uses `huge=`, which nobody reported. `pread` from tmpfs is a memory copy. Measurements, all from other teams' machines:
  - For `pread`: [TejasviniChawla `docs/NOTES.md`](https://github.com/TejasviniChawla/shopping-sessions/blob/cd1ddca/docs/NOTES.md), 2026-11-22 entry: going to 512 KB work units "halve sys time (1.02 to 0.50 s) and cut total about 10%" on the same runner; the same notes record that populating mappings hurt. Vibhor7-7 experiment 10 (entry 1.5). cherylaurelia reads with `pread`.
  - For mmap: arnabxero maps tmpfs inputs deliberately; AbhirajTiwari12, joshuamuellenbach25 and micah-andelman returned to or stayed with mappings (entry 2.3).
  - **Here.** The cheapest decisive test is `IOMODE=4` with the chunk target at 512 KB instead of 4 MB (`main.rs:411-412`), A/B against the default on an x86 VM or the board. The existing `pread` path already keeps an 8-byte margin (`parse.rs:261`).
- **Cascade Lake, confirmed by one team.** The `x86-64-v3` default stays (entry 4.5). L2 is 1 MB per core, shared by two hyperthreads.
- **THP is `madvise`.** The arena is advised (`mem.rs:32`); for the mimalloc heap see iteration 12.
- **Fixed cost.** About 50 ms before `main` on the scorer by arnabxero's log, outside the entrant's control. Start-up work inside `main` (entry 8.1) is the only fixed part that can be cut.

### 11.3 Measured numbers for ideas already in this file

From [joch44 `EXPERIMENTS.md`](https://github.com/joch44/shopping-sessions/blob/0e62970/EXPERIMENTS.md), medians on an Ice Lake 8370C VM with 4 cores and 8 hyperthreads, interleaved A/B, 20M and 60M:

| Change | 20M | 60M | Entry here |
|---|---|---|---|
| Skip product lookup for views and carts (v4) | 3.01 to 2.94 s | 8.81 to 8.49 s | 1.1 |
| 16-byte per-person events, person implied by the group (v20) | 1.76 to 1.73 s | 4.89 to 4.85 s | 7.1 |
| Dedup table of (hash32, index), winners emitted in order (v15) | 1.96 to 1.92 s | 5.53 to 5.35 s | 7.4, 9.1 |
| 1024 dedup partitions with a date cache and one-pass splitter (v5) | 2.94 to 2.66 s; dedup 0.49 to 0.33 s | 8.49 to 7.72 s | bundled |
| Dense campaign counters and 1024 person partitions (v19) | 1.82 to 1.78 s | 5.12 to 4.97 s | none; this repo's counters are already dense |
| 2048 or 4096 dedup partitions | dedup -0.01 s, parse +0.03 to 0.06 s | | rejected there |
| Radix sort for sessions | no gain over counting sort | | rejected there |
| Shared lock-free global dedup table | 1.7x slower than partitioning | | rejected there |
| Software write-combining | "no gain on Ice Lake" | | 1.2 |

Other reports on the same ideas:

- **Write-combining.** arnabxero lists "write-combining scatter stores" under tried and dropped (measured on an EPYC 9554). joch44 found no gain on Ice Lake. Vibhor7-7 measured -12% on the same Ice Lake model (entry 1.2); axhiao (`a77b4dc`), Th3r0n, explorer-skp, cherylaurelia and TejasviniChawla (16-record staging, 512-byte bursts) kept it. The difference may be whether the destination was already cheap to write: joch44's note says it "needs THP", and this repo's arena is on huge pages. No one has published a Cascade Lake number, so this is an A/B, not a sure win; hence its drop from second to fifth.
- **Date cache.** joch44 v10: a last-date cache "missed 93% (shuffled rows)"; v11 replaced it with a 16-slot cache keyed by month. arnabxero (`08cce5d`), niranjankumarnk (`871eec9`), TejasviniChawla and jklein07 ("table-based dates") also cache validated dates. This confirms entry 1.4's design point: key the table by date, never by "last seen".
- **Dedup sketch.** TejasviniChawla: "sketch-based dedup (2-bit counters, 16 slots/record; exact table only for candidates)"; axhiao `124164e`: "bitmap pre-pass so only candidate duplicates use the exact table". With Vibhor7-7, cherylaurelia and micah-andelman that is five teams on entry 9.1's scheme.
- **Global user table at parse time.** TejasviniChawla restructured to a lock-free global user table and scored 2.17 s against 1.79 s before, then reverted (`a805d87`). This supports keeping this repo's per-partition resolve (`b3c1d38`).
- **Mid-size units and kernel time.** jklein07: with one `Vec` per partition "about 20% of all CPU was the kernel zeroing and faulting 4 KB pages"; huge-page arenas cut the kernel share from 15% to 5%. This repo already has that.
- **Exit.** TejasviniChawla added a fork-and-exit wrapper early and later "parallel unmap of the pool before exit" (`cd1ddca`); entry 3.2 explains why the wrapper cannot hide teardown from the clock. jklein07 `1a2371f` and axhiao release consumed buffers in parallel during the run. `DROP=1` and `AREL=1` here were measured as no help in the audit's sandbox, on 2 vCPUs.

### 11.4 Cross-machine comparisons mislead

- TejasviniChawla: "Cross-runner CI comparisons misled; only same-runner A/B from now on" after a restructure that looked faster on CI scored worse. joch44: gains measured on a Zen 5 VM "roughly halve" on the Ice Lake mirror, and memory-layout changes must be A/B'd interleaved on the mirror because drift is 2 to 4%. Vibhor7-7 saw -12%, -6% and -1% for the same change on three CPUs.
- **Consequence for this repo.** Development is on an ARM Mac. Rank changes by instruction count there (ideas 1 to 3 in the table cut instructions or bytes on any CPU), and treat cache and memory-system changes (ideas 4 and 5) as unknown until measured on x86.

---

## Iteration 12 (2026-11-21): the allocator already asks for huge pages; build flags; threads

Opened by a research agent, with the mimalloc claim checked against the vendored source on this machine.

### 12.1 Correction to 3.4: mimalloc v3 advises huge pages by default

- `Cargo.lock` pins `libmimalloc-sys 0.1.49`, whose `build.rs` compiles mimalloc **v3** unless the `v2` feature is set. In `c_src/mimalloc/v3/src/prim/unix/prim.c:413-421` it calls `madvise(MADV_HUGEPAGE)` when `allow_thp` is on (the default) and the size and alignment are 2 MiB multiples; the heap reserves 1 GiB arenas and commits them eagerly when overcommit mode is 0 or 1.
- So under THP `madvise`, `KMap` slots, `Scratch` vectors, `Vec<URow>` and the Parquet buffers should already be on 2 MiB pages. Entry 3.4's statement that they get huge pages only under `always` is wrong for this build. Not verified by a run.
- **How to verify without guessing.** Print `AnonHugePages` from `/proc/self/smaps_rollup` next to the `rusage` line at the end of `main`, or compare `ru_minflt` with heap size: about one fault per 2 MB means huge pages, about 256 per MB means 4 KiB pages. The audit's 12,000 to 14,000 minor faults for 1.27 GB anonymous memory at 20M already points to huge pages.
- **Consequence.** No `HugeVec` wrapper is needed. joch44's and TejasviniChawla's huge-page allocator work (entry 11.3) solved a problem their glibc-malloc builds had and this one does not.
- If overcommit mode is 2 on the scorer, `MAP_NORESERVE` is ignored ([overcommit accounting](https://docs.kernel.org/mm/overcommit-accounting.html)) and `Arena::new` would fail over to per-block heap allocation, correct but slow. arnabxero's diagnostics do not report the mode.

### 12.2 What 2 MiB pages are worth, for calibration

- 10M random inserts into a hash map: 0.709 s on 4 KiB pages against 0.544 s on 2 MiB (-23%), with dTLB misses falling from 19.4M to 6.3K ([Rigtorp](https://rigtorp.se/hugepages/)). That table is far beyond TLB reach; a 2 to 4 MB dedup table is inside the roughly 6 MB reach of a 1,536-entry second-level TLB ([Jones](https://www.evanjones.ca/hugepages-are-a-good-idea.html)), so expect much less there.

### 12.3 Direct compaction can stall huge-page faults

- With `defrag` at its default `madvise`, a fault in an advised region may enter direct reclaim and compaction ([kernel THP doc](https://docs.kernel.org/admin-guide/mm/transhuge.html)). On a fragmented host both the arena and the mimalloc heap can stall, and nothing inside the container can change it.
- **Possible mitigation.** Prefault the first part of the arena from helper threads during phase 0 and 1 with `MADV_POPULATE_WRITE` (Linux 5.14 and later; the scorer is reported on 7.0), ignoring errors, and only for memory that will certainly be used, since populated pages are charged to the cgroup. The arena estimate at `main.rs:382` gives the size.
- **Expected gain.** Unknown; it moves page zeroing (which parse threads pay today at first touch) onto threads that are otherwise idle during the link phase. TejasviniChawla reports `MADV_POPULATE_WRITE` after `MADV_HUGEPAGE` as part of a +5% build. Risk: none to exactness; memory use rises if the estimate is high.

### 12.4 Build flags and threads: nothing worth a submission

- `tune-cpu` is unstable ([rustc codegen options](https://doc.rust-lang.org/rustc/codegen-options/index.html)). `-C target-cpu=skylake` gives Skylake tuning without AVX-512, which mainly removes a false-dependency `xor` before `tzcnt`/`lzcnt` ([LLVM X86.td](https://raw.githubusercontent.com/llvm/llvm-project/main/llvm/lib/Target/X86/X86.td)); no measurement found, expect under 1%.
- Thread spawn is about 5 µs ([Bendersky](https://eli.thegreenplace.net/2018/launching-linux-threads-and-processes-with-clone/)); eight `par_for` phases of 8 threads cost about 0.3 ms. A persistent pool is not worth writing.
- Pinning: no measurement for an 8-vCPU VM. TejasviniChawla included pinning in a batch that was 3 to 10% faster overall, without isolating it.
- `available_parallelism` honours cgroup quota and affinity ([std docs](https://doc.rust-lang.org/std/thread/fn.available_parallelism.html)); `main.rs:331-335` reads `NCPU` first, which the scorer sets to 8.

---

## Iteration 13 (2026-11-21): remaining competitor notes, and two small derived ideas

Sources: [axhiao README](https://github.com/axhiao/Shopping-Sessions/blob/1d1fa1c/README.md), [enkyuan/polaris `PERFORMANCE.md`](https://github.com/enkyuan/polaris/blob/9f20a22/PERFORMANCE.md), commit logs of niranjankumarnk, sonalrajapaksha, toM0dots and SsjRaz.

### 13.1 Leave self-named IDs out of the link shards

- **Idea.** A link shard maps every linked ID to its person's name, including the IDs that are their own name. `Ctx::person_h` and `resolve_partition` already treat a miss as "unchanged" (`main.rs:136`, `:678`), so those entries carry no information. Dropping them shrinks the shards and turns their lookups into early misses.
- **Sources.** axhiao: "A 2 MB bit filter marks the IDs whose person is not themselves, so most rows never probe the table" (README, step 2; commit `a1c934b`). enkyuan/polaris: "The bitmap contains every noncanonical alias. A clear bit proves the ID is absent" (measured 1.08 to 1.07 s, inside its noise). The audit's microbench on the 20M set counts 60.6% of event rows with a user in the link map and 39.2% actually rewritten, so about a third of the hits are self-named.
- **Here.** In the last step of `build_links` (`main.rs:258-268`) the slots are rewritten in place from index to name. Instead, build each shard's final map from only the slots where `name != key`. Shards get smaller by the share of linked IDs that are roots, and `resolve_partition` gets the same answer through the existing miss path.
- **Expected gain.** Resolve is about 8% of wall and its shards are already cache-local, so perhaps 10 to 20% of that phase: about 1% of wall. Smaller than the filters above because this repo probes per partition, not from the parse loop.
- **Risk.** None to exactness: a miss and a hit that returns the key itself are handled identically today.

### 13.2 16-byte click records inside a person group

- **Idea.** The click analogue of 7.1. Inside a group `person` is constant, so a click needs only `ts` (as a 32-bit offset from `window_start - 7 days`), `camp` and `cid`: 16 bytes instead of the 32-byte `CRec`. The sort key `(ts, cid)` is then the high 96 bits of one `u128`, so `sort_unstable` on plain integers replaces the comparator closure at `main.rs:899`.
- **Sources.** axhiao `1d1fa1c`: "packed click keys with side arrays"; niranjankumarnk `c53fa98`: "24-byte click records: time and campaign share one word" and `644e32c`: "order each person's clicks through keys"; joch44 v20 kept 32-byte clicks and gained 2.8% from the event half alone.
- **Here.** `Scratch.cl` becomes `Vec<u128>` (or a `#[repr(C)]` pair of `u64`s); the scatter at `main.rs:862-876` packs `((ts - cws) as u128) << 96 | (cid as u128) << 32 | camp as u128`. Integer order on `cid` equals `key_cmp` only for packed keys, so if any `cid` in the group `is_long`, keep the current comparator for that group (generated data never has one). The click window offset fits 32 bits because the event window already must (`main.rs:344`) and 7 days adds 6.05e8 ms; assert that `we - ws + WEEK_MS < 2^32` and fall back otherwise.
- **Expected gain.** About 1% of wall: clicks are a quarter as many as events, but their comparator is the expensive one and whales have about 100,000 of them.
- **Risk.** Low. The credit tie-break depends on this order (`main.rs:973-980`: first of the equal-`ts` run), so test with the fixtures that pair clicks at the same millisecond and add a unit test that the `u128` order equals `(ts, key_cmp(cid))` on random packed keys.

### 13.3 Confirmations, no new ideas

- axhiao implements idea 1.3 in full: a 16-byte dedup record (id fingerprint, ingest seconds, payload location) per copy, a 24-byte payload per valid copy written straight to its person partition, losers "marked dead in place in the payload buffers", with write-combining buffers on the partition writes. That is a sixth independent instance of the design; its reported time is 1.15 s at 20M under Docker on a Threadripper, so it is not evidence of a top score.
- enkyuan/polaris (board 1.41 s) tried an interleaved two-bit dedup bitmap against separate "seen" and "repeated" bitmaps and found no repeatable difference. Either layout of entry 9.1's counters is fine.
- sonalrajapaksha `0695a92`: "Drop the prefetched link table and the no-free change: both measured worse". niranjankumarnk `3fcf2f2` writes dictionary pages with RLE indices (entry 5.2). toM0dots and SsjRaz are C++ entries without scores or measurements in their logs.

---

## Iteration 14 (2026-11-21): the scanner, against the first x86 profile

`OVERNIGHT_LOG.md` appeared at 00:19 with an x86 profile of `1c4fa8e`. It changes the ranking: the scanner, not the decode, is the largest single cost (490 of 1,040 instructions per event line), and `cherylaurelia`'s build, whose scanner is the line-at-a-time design, runs the same 20M set in 1.38 s against 2.32 s.

### 14.1 A line-at-a-time splitter that scans each byte once

- **Idea.** Keep computing comma, quote and newline masks once per 64-byte block (as `masks64` does), but stop flattening them into `pos`, `nl` and the quote ring. For the line starting at byte `p`, take a 128-bit window of each mask stream at bit offset `p`, find the newline with one `tzcnt`, mask the comma and quote windows below it, and derive validity and all N field (offset, length) pairs from those two words with the arithmetic of entry 2.1.
- **Why not copy cherylaurelia's loop exactly.** Their `scan_lines` calls `masks64` at every line start, so a 77-byte line costs two 64-byte classifications (128 bytes scanned per 77 consumed). Shifting windows out of block masks computed once scans each byte once and is otherwise the same algorithm.
- **Sources.** [cherylaurelia `fast_split`](https://github.com/cherylaurelia/builders-hackathon/blob/2e4261f/src/main.rs) lines 1352 to 1380 (entry 2.1); explorer-skp's measurement that an index-building scanner was 6% slower than a per-delimiter one (`333fd23`); jklein07 `d1036a8` "Per-line 128-byte SIMD splitter" and `a592edd` "Split quoted lines with whole-line masks instead of a per-field quote loop" (board 1.66 to 1.44 s with two other changes); arnabxero `4868fc8` "Line-at-a-time SIMD row scanner"; AbhirajTiwari12 returning to line-at-a-time three times (entry 2.1). Against: micah-andelman's two-stage scanner, and arnabxero listing a "block-structured scanner" as tried with no gain.
- **Algorithm** (per chunk; `cs`, `qs`, `ns` are the block masks, with two zero blocks appended):

  ```
  win(ms, p):  b = p >> 6; o = p & 63
               o == 0 ? (ms[b], ms[b+1])
                      : ((ms[b] >> o) | (ms[b+1] << (64-o)), (ms[b+1] >> o) | (ms[b+2] << (64-o)))
  loop:
    (n0, n1) = win(ns, p)
    e = n0 != 0 ? tzcnt(n0) : n1 != 0 ? 64 + tzcnt(n1) : -> scalar `split` on the line, continue
    (c0, c1) = win(cs, p); (q0, q1) = win(qs, p); clear all bits at and above e
    if popcnt(c0) + popcnt(c1) == N-1:
      b = c | (1 << e)                      # delimiters incl. the newline, as a 128-bit value
      s = (b << 1) | 1;  ends = b >> 1      # field starts, field last bytes
      x = q & s;  y = (q << 1) & b          # opening quotes; closing quotes moved onto their delimiter
      ok = (q & !(s | ends)) | (x & ends) | ((y - x) & b) == 0      # 128-bit subtract with borrow
      if ok:
        commas = byte-packed offsets of the N-1 comma bits (tzcnt/blsr on c0 and c1, merged)
        end_k  = commas with e appended;  start_k = end_{k-1} + 1 (0 for k = 0)
        quoted = pdep(pext(x0, s0) | pext(x1, s1) << popcnt(s0), 0x0101010101010101)
        at_k = start_k + quoted_k;  len_k = end_k - at_k - quoted_k
        call f with the N slices
    p += e + 1
  ```

  Because the three mask streams are only read through `win`, a chunk needs no ring and no carry between segments; compute the masks for a few KB at a time (micah-andelman found 4 to 8 KB batches best, flat from 2 to 64 KB) to stay in L1.
- **Model check.** A Python model of exactly this algorithm, including the window shifts, the 128-bit borrow, the scalar fallback for lines of 128 bytes or more and an unterminated last line, matched a line-by-line `split` reference on 6,000 random buffers for N = 2, 4 and 7 (quoted fields, stray quotes, lone-quote fields, wrong field counts, `\r`, spaces, empty lines, fields up to 90 bytes): 0 mismatches. Script: `/private/tmp/claude-501/-Users-nimaibhat-Documents-databricks-chall/273c3dc9-8aa1-4164-a68d-59b78a9a199a/scratchpad/chk2/split.py`. This checks the logic only; the Rust must be fuzzed again.
- **Instruction estimate.** Classification 22 instructions per 64 bytes, so about 27 per line; three windows about 30; newline, masking and popcounts about 15; validity about 20; comma offsets about 50 for N = 7; quoted, `at` and `len` about 15. Roughly 130 to 170 per line against 490 now. These are counts by hand, not measurements.
- **Here.** `scan_lines::<N>` keeps its signature, so `parse_events`, `parse_clicks`, `build_links` and the microbench do not change. `fields[i]` becomes `from_raw_parts(base + p + at_i, len_i)`. Lines of 128 bytes or more, and the tail after the last newline, go to `split` as they do today (`parse.rs:595-601`, `:648-653`).
- **Expected gain.** 10 to 15% of wall.
- **Risk.** Medium. Before trusting it: extend `fuzz_scan` (`parse.rs:675`) to N = 7 and to lines that straddle offsets 64 and 128, and keep the old scanner behind a toggle for one A/B.

### 14.2 Do the tombstone dedup without moving person resolution into the parse loop

- The log's first queued idea couples two things: (a) a narrow dedup side stream with tombstones, and (b) resolving the person at parse time through a global link table with a prefetch ring. They are separable, and the evidence for them differs.
- **For (a):** six teams (cherylaurelia, explorer-skp, Vibhor7-7, axhiao, toM0dots "deduplicate by reference", enkyuan "deduplicate event versions in place"); entries 1.3, 9.1, 13.3.
- **On (b) the evidence is split, and an earlier version of this entry understated the case for it.** For: the two fastest builds on the log's runner both do it. cherylaurelia (1.38 s) looks the user up in a global ID table with a prefetch issued some lines ahead and a second prefetch three slots past the home slot (`83e9492`). Vibhor7-7 (1.59 s, board 0.82 s) files each surviving event "under its person" at parse; its README says "the link table is larger than the cache; each line asks for its cache line 16 lines before reading it, and the four slots of that line are compared at once", with the linked-or-not choice computed both ways and selected (experiments 4, 6 and 14: -1.7%, -2%, -1.5%; a table twice as large was 1.3% slower). axhiao uses a two-stage prefetch pipeline. Against: this repo's own history (`5cb3bc7` had a prefetch pipeline and `b3c1d38` replaced it with per-partition resolve; the audit measured about 120 ns per line for an unprefetched lookup against the 64 MB map); TejasviniChawla's global-table restructure scored 2.17 s against 1.79 s and was reverted; sonalrajapaksha dropped a prefetched link table as "measured worse"; explorer-skp groups records by link-table slice so lookups stay in cache.
- **What the working versions have in common.** A table laid out so one lookup touches one cache line (Vibhor7-7: four IDs then their four people per line, every probe starting on a line boundary), a prefetch distance of about 16 lines, no unpredictable branch on hit or miss, and a table no larger than needed. The 64 MB of 16-byte slots at load 0.25 to 0.5 in `build_links` is several times larger than that; entry 13.1 (drop self-named IDs) and a higher load factor would shrink it before any prefetch ring is built.
- **Suggested order.** (a) and (b) are separable. Ship (a) alone first: at parse, push the `SRec` into the user-hash partition and a 16-byte `(eid, ingest, loc)` record into the eid partition; dedup marks losers; `resolve_partition` drops tombstones in its existing `compact` closure. That deletes dedup pass 2 with every probe still partition-local, and it is a strict subset of the work for (b). Then build (b) on top and A/B it on x86, because that is the step two teams measured as a loss and two as a win.
- **Record address.** `loc` must survive until dedup. `BList` blocks never move, so block pointer plus index works; Vibhor7-7 uses a global 32-bit record index over slabs (`src/pool.rs`). If write-combining stages are added later, a record's final address is known at push time even while its bytes are still in the stage (cherylaurelia's stream comment, `src/main.rs` lines 345 to 350), but dedup must run only after the stages are flushed.

### 14.3 SIMD ID packing that matches this repo's key format

- **Correction to iteration 1.** "PEXT ID packing" was listed as not worth pursuing on the audit's microbench figure. The log's `perf` profile puts `pack_fast`'s loop at 5.8% of self time (`keys.rs:87-88`), so it is worth doing.
- **Source.** [cherylaurelia `pack_at` and `pack2_at`](https://github.com/cherylaurelia/builders-hackathon/blob/2e4261f/src/main.rs), lines 133 to 212: classify 16 bytes with two `pshufb` nibble lookups, subtract a per-row offset to get 6-bit codes, combine pairs with `pmaddubsw` (multiplier `0x0140`) and quads with `pmaddwd` (`0x0001_1000`), then assemble three 24-bit groups with shifts. `pack2_at` does both IDs of a line in the two halves of one 256-bit register (A/B ratio about 0.98 on top of the single version; `9a37097` 0.93 to 0.96 for the single version with the scanner change). Vibhor7-7 experiment 2 and micah-andelman `3a12af8` use nibble tables with PEXT instead.
- **Adapting it.** Their alphabet has no `-` and no length nibble. This repo's codes are `-` = 0, `0-9` = 1 to 10, `A-Z` = 11 to 36, `_` = 37, `a-z` = 38 to 63, which equal theirs for the 63 shared symbols. Add row 2 to the tables: `HI[2] = 32`, `LO[0xD] |= 0x20` (so `0x2A`), `OFF[2] = 45`. The result `k` holds the first character at bits 54 to 59; this repo's key is `(k << 4) | n` after clearing the places past `n`.
- **Model check.** A Python model of those exact table values and multiply-adds equals `keys.rs` `pack` for every byte value at every position for lengths 0 to 11 and for 400,000 random strings with stray bytes, including the reject-to-`pack_long` decision: 0 mismatches. Script: `/private/tmp/claude-501/-Users-nimaibhat-Documents-databricks-chall/273c3dc9-8aa1-4164-a68d-59b78a9a199a/scratchpad/chk2/pack.py`.
- **Here.** Replace the 10-iteration loop in `pack_fast` (`keys.rs:79-93`); the `p + 16 <= ceil` guard already exists. For the two IDs of an event line use the 256-bit form in `parse_events` (`main.rs:608`, `:631`); note `person` is only needed for valid lines, but computing both unconditionally is cheaper than a second call.
- **Expected gain.** 3 to 5% of wall.
- **Risk.** Low. Keep `keys::tests::order` and add the exhaustive byte-by-position test from the model (micah-andelman's commit lists the same test plan: every byte value by position by length, plus random ids at page-straddling offsets).

### 14.4 Other readings of the profile

- **Hash-map probes are 5.2% of self time** (`keys.rs:206`, `:228`). Those are `get_h` in resolve and click grouping and `entry_h` in dedup and person grouping. Entry 9.1 removes the dedup share; entry 13.1 shortens the resolve share; the person-grouping `gmap` in `process_partition` is partition-local and small.
- **`BList::push` is 3.1%** and kernel page zeroing 2.2% on the EPYC runner. Write-combining can at best remove part of the first; on that CPU Vibhor7-7 measured -1% to -6% for it. Its value on Cascade Lake is still unknown.
- **Yardstick.** By the log, board time is roughly 0.52 to 0.6 of the runner's time, and this build is at 2.32 s there. Items 1 to 5 in the table sum to roughly 25 to 35% if their estimates hold and add up, which would land near Vibhor7-7's 1.59 s; reaching cherylaurelia's 1.38 s also needs the structural change in 14.2.

---

## Iteration 15 (2026-11-21): the timestamp path in detail (230 instructions per line with price)

Source: [cherylaurelia `src/main.rs`](https://github.com/cherylaurelia/builders-hackathon/blob/2e4261f/src/main.rs) lines 984 to 1135 (`civil_at`, `ts_at`, `ts_memo`), read in full. This makes entry 1.4 concrete and adds two cheaper building blocks that help even without the table.

### 15.1 Validate digits and separators in one expression per word

- **Idea.** XOR the 8-byte word with the pattern it should match, written with `0` for digit places (for example `"00:00:00"`). A valid digit byte becomes 0 to 9 and a matching separator becomes 0. Add `0x76` to digit places and `0x7F` to separator places: the high bit of a byte is then set exactly when that byte was out of range. The test is `((x + ADD) | x) & 0x8080808080808080 == 0`. A carry out of a byte can only come from a byte whose own high bit was already set, and `| x` catches that byte, so carries cannot hide an error.
- **Here.** `civil_swar` (`parse.rs:772-785`) spends three mask-and-fill steps, a separate separator XOR and three `digits8` calls (about 7 operations each) on the same job. This form is 4 operations per word.
- **Risk.** Low; `civil_swar_fuzz` covers it unchanged.

### 15.2 Hour, minute and second from one word and one multiply

- **Idea.** Load the 8 bytes `HH:MM:SS` at `p + 11`, XOR with `"00:00:00"`, and compute `pb = x * 10 + (x >> 8)`. Then `h = pb & 0xFF`, `mi = (pb >> 24) & 0xFF`, `sec = (pb >> 48) & 0xFF`. The separator bytes are zero after the XOR, so the neighbouring digits do not mix.
- **Here.** `civil_swar` calls `pairs()` five times across three overlapping words (`parse.rs:786-796`) to get year, month, day, hour, minute and second. With the date coming from the table (15.3), the time of day needs only this one word.

### 15.3 The date table, concretely

- **Design.** A per-thread `[Day; 32]` where `Day = { a: u64 (bytes 0..8), b: u32 (bytes 8..11, i.e. "DDT"), midnight: i64 }`, indexed by `((b & 15) * 10 + ((b >> 8) & 15)) & 31`, which is the day of month. An unused slot holds a `b` that no input can equal.
  - Shape test first: length 20 with `Z` at 19, or length 25 with `+`/`-` at 19 and `:` at 22; anything else goes to the existing paths.
  - Hit (`a` and `b` equal): validate and decode the time of day as in 15.1 and 15.2, add `midnight`, apply the offset for F2.
  - Miss: run the full checked parser. If it returns a value, store `(a, b, midnight of that date)`; if it rejects, store nothing.
- **Why it is exact.** A stored entry proves that those 11 bytes are a valid date with `T`. Whether the whole timestamp is valid then depends only on the time-of-day bytes and the zone, which the hit path checks in full. A date whose first sighting had a bad time of day is simply not stored yet.
- **Model check.** A Python model of this table against an exact F1/F2 reference, sharing one table across 1,500,000 random timestamps: years 2026 to 2028, months including 0, 2 and 13, days 0 to 31 (so February 29 in leap and non-leap years and February 30 collide in the same slot), hours to 25, minutes and seconds to 61, offsets to `+25:61`, and up to two corrupted bytes from an alphabet with `t`, `z`, space, `\r`, comma and quote. 0 mismatches. Script: `/private/tmp/claude-501/-Users-nimaibhat-Documents-databricks-chall/273c3dc9-8aa1-4164-a68d-59b78a9a199a/scratchpad/chk2/ts.py`.
- **Hit rate.** The generator writes `ingest_ts` minutes after `ts` (entry 7.6), and a 14-day window plus 7 days of clicks plus 5 days of late ingest spans at most about 27 dates, so day-of-month collisions are rare and nearly every lookup after the first few hundred lines hits. joch44's month-keyed 16-slot cache is the same idea; its last-date cache missed 93% of the time.
- **Here.** `parse_ts_fast` F1 and F2 branches (`parse.rs:922-941`) and `parse_ingest_s` (`parse.rs:889-912`). For `ingest_ts` the miss path must go through the checked `parse_ts`, not the current unchecked arithmetic, so that only validated dates are stored; after that its hit path is as cheap as today's unchecked one and also validates the time of day. The table lives in the per-thread state passed to `parse_events` and `parse_clicks` (one more `&mut` next to `bs`).
- **Expected gain.** By hand count a hit is about 25 instructions against roughly 65 to 80 for `civil_swar` and about 40 for the unchecked ingest path, so 50 to 70 instructions per event line of 1,040: 5 to 7% of parse, 3 to 4% of wall.
- **Risk.** Low, for the reasons above. Extend `civil_swar_fuzz` and `ts_fuzz` to run every case twice against one shared table.

### 15.4 F3 for any length up to 16 without reading backwards

- **Idea.** Load 16 bytes at the field start as a `u128`, shift left by `8 * (16 - len)` so the digits are right-aligned, OR in ASCII zeros below, validate both halves with 15.1's test and convert with two 8-digit SWAR steps.
- **Here.** `parse_ts_fast` handles 9 to 16 digits by loading the 16 bytes that end at the field end, which needs the `floor` guard and leaves 1 to 8 digits to the scalar loop (`parse.rs:942-953`). Generated F3 values are 13 digits, so this matters only for uniformity: one path, one guard (`p + 16 <= ceil`).
- **Expected gain.** None on generated data. Listed so the F3 branch is not rewritten twice if `parse_ts_fast` is restructured for 15.3.

---

## Iteration 16 (2026-11-21): new competitor measurements since the first pass

Sources: commits pushed after iteration 2 to joch44 (`e71539c`), Vibhor7-7 (`7a5ddaf`), Th3r0n (`6034d47`) and TejasviniChawla (`6925b5e`), pulled into the scratch clones. joch44's numbers are medians on an Ice Lake 8370C VM with 4 cores and 8 hyperthreads, interleaved, which is the closest public stand-in for the scorer; its v20 scored 1.60 s on the board against 1.73 s there.

### 16.1 Mask-based line split, measured in isolation

- [joch44 `e71539c`](https://github.com/joch44/shopping-sessions/commit/e71539c) (v25): "`split_masked` decides the quote rule per line in registers with BMI2 PEXT: per-field start/end quote bits must match, a lone `\"` field is rejected, and no quote may sit elsewhere ... Mirror split-only: 0.217 to 0.078 s." That is the three-test rule of entry 4.1, and a 2.8x reduction of the split stage on Ice Lake.
- [Vibhor7-7 `7a5ddaf`](https://github.com/Vibhor7-7/shopping-sessions/commit/7a5ddaf) (experiment 22, not yet measured in its log): keeps a delimiter index like this repo's `pos`, but takes the four bytes around each of a line's eight delimiters, compares them all with a quote at once, and gets every field's start and length "in a dozen vector instructions; before, it was seven rounds of two loads and three compares, about 145 instructions a line". This is a second route to the same end that keeps the index. AbhirajTiwari12 tried a gather-based split and went back to scalar (entry 2.1), so treat it as unproven.
- **Consequence.** Entry 14.1 (top of the table) now has a direct measurement behind its central step. If the full rewrite is too much for one night, the smaller change is to keep `pos` and replace only `parse.rs:602-638` with the mask rule on the line's two words.

### 16.2 Non-temporal stores gained 7% on the Ice Lake mirror after all

- [joch44 `319a2e6`](https://github.com/joch44/shopping-sessions/commit/319a2e6) (v21): "non-temporal stores for 64-byte scatter records (EventCopy is one cache line)", `sfence` per chunk. Parse events 0.77 to 0.655 s; full A/B 1.79 to 1.66 s at 20M and 4.83 to 4.49 s at 60M. This reverses the "no gain on Ice Lake" line quoted in entry 11.3, which came from an earlier design of theirs.
- The same log rejects padding clicks to 64 bytes for streaming (0.131 to 0.134 s: "clicks are not RFO-bound") and chunk sizes of 16, 4 or 1 MiB (under 5 ms difference).
- **Here.** Their record is exactly one cache line, so each push is one streamed line with no staging. `ERec` is 32 bytes, so this repo needs the stage of entry 6.1 (or, after 14.2, 16-byte records in stages of 4 to 8). Stream events only; leave `CRec` on ordinary stores.
- **Standing.** With Vibhor7-7's -12% this is two measurements on Ice Lake 8370C in favour, against one "no gain" on an EPYC (arnabxero). Still nothing on Cascade Lake. It moves back up to sixth, ahead of `pread`.

### 16.3 SIMD timestamps were worth 8% there

- [joch44 `e9c3e64`](https://github.com/joch44/shopping-sessions/commit/e9c3e64) (v22): "SIMD timestamp parser + ingest_ts fast path (relies on the rules' F1 guarantee)": parse events 0.63 to 0.53 s, total 1.65 to 1.52 s at 20M and 4.56 to 4.17 s at 60M.
- **Here.** This repo already has SWAR timestamps and an unchecked ingest path, so the same 8% is not available; what remains is entry 15's table and cheaper validation. It does show that timestamp work converts to wall time at this stage of optimisation.

### 16.4 Profile-guided optimisation: a measured 3 to 4%, reopening a rejected item

- [joch44 `f05eae2`](https://github.com/joch44/shopping-sessions/commit/f05eae2) (v24): PGO with `-Cprofile-use` and a profile committed to the repo. Interleaved on the mirror: 1.50 to 1.46 s at 20M (-2.7%) and 4.16 to 3.99 s at 60M (-4.1%); on a Zen 5 VM "parse events -8%, clicks -11%, wall -3.6%". arnabxero lists PGO under "tried and dropped (no gain)".
- `speedups.md` set PGO aside on a 1% figure for rustc itself. For a parser whose cost is instruction count and branch layout, one team now measures 3 to 4%.
- **Here.** Mechanics in their repo: instrument with the Dockerfile's exact flags, train on datasets that are never timed, merge with the toolchain's own `llvm-profdata`, commit `merged.profdata`, add `-Cprofile-use=...` to `RUSTFLAGS` in the Dockerfile ([Dockerfile](https://github.com/joch44/shopping-sessions/blob/e71539c/Dockerfile), `tools/pgo.sh`).
- **Expected gain.** 2 to 4% of wall.
- **Risk.** None to exactness: it changes layout and inlining only. The risk is the build: the profile must come from the same rustc version the Dockerfile pins (`rust:1.91-bookworm` here), since a profile from another LLVM version can fail to load, and a failed build is a rejected submission. A stale profile after source changes still builds but helps less. Do it last, after the source stops changing, and rebuild the image from a clean clone before submitting.

### 16.5 Serial sections another team found once its parse got fast

- [joch44 `EXPERIMENTS.md`](https://github.com/joch44/shopping-sessions/blob/e71539c/EXPERIMENTS.md), "no idle core" list for its v22: start-up on one thread about 23 ms (manifest, catalogs, per-file mmap); links on one thread about 0.25 s of CPU; a barrier and thread respawn after the events pool; a serial weighting and sort of person partitions; a serial merge of 8 dense campaign-counter arrays; and "exit teardown in the kernel (about 0.11 s, at the container floor)". Making links parallel was a tie at their current speed (1.507 against 1.514 s) but turns 0.25 s of serial work into 0.08 s parallel.
- **Here, the same places.** Catalog load (entry 8.1); the serial union-find and naming in `build_links` (`main.rs:218-257`, entry 7.3) which runs before parse instead of beside it (queued); the `transpose` calls between phases (`main.rs:432-433`, `:446`, `:464-469`), each building 512 small vectors on one thread; the campaign merge at `out.rs:76-83` (2,001 campaigns by 14 days by 4 counters by 8 threads, about 0.9 MB of adds: negligible); and exit.
- **Exit teardown at 0.11 s is new information.** The audit measured 17 to 24 ms at 20M in a 2-vCPU sandbox; the log says teardown "is inside the 0.2 s container overhead and not yet split out". Two other teams release memory in parallel before exiting (TejasviniChawla `cd1ddca`, jklein07 `1a2371f`), and `AREL=1` / `DROP=1` exist here but were judged on that sandbox. Worth re-measuring on the x86 runner: time from the "done" line to container exit, with and without `AREL=1`.

### 16.6 Smaller items

- [Th3r0n `17f3282`](https://github.com/Th3r0n/shopping-sessions/commit/17f3282): "Dedup table: generation-stamped slots instead of clearing per partition". `KMap::reset` (`keys.rs:175-186`) rewrites every slot key of a 2 to 4 MB table for each of 512 partitions, about 1.5 GB of stores at 20M. Entry 9.1 makes the table 8 to 16 times smaller, which removes most of this; a generation stamp removes the rest but costs a compare per probe.
- [Th3r0n `6034d47`](https://github.com/Th3r0n/shopping-sessions/commit/6034d47): "software-pipelined union-find, prefetched final passes", the same direction as entry 7.3. No number given.
- [TejasviniChawla `6925b5e`](https://github.com/TejasviniChawla/shopping-sessions/commit/6925b5e) added Snappy page compression to its Parquet writer and a CI experiment on tmpfs against bind-mounted output; no result is recorded yet. The scorer's `/output` is reported as ext4 (entry 11.1).

---

## Iteration 17 (2026-11-21): the sessions phase (16% of in-process time) and what the profile closes

### 17.1 Assist increments: 36.9 million scattered adds at 20M

- **Evidence.** The audit's read-back of the 20M output gives `sum(assisted_sessions) = 36,877,579` for 3,402,836 sessions: about 11 assists per session on average, driven by the whales, who have a session in every two-hour slot and about 100,000 clicks each ([joch44 `DATA_PROFILE.md`](https://github.com/joch44/shopping-sessions/blob/e71539c/DATA_PROFILE.md)). Each one is `a.camp_cell(c, di)[3] += 1` at `main.rs:987-989`, a read-modify-write into a 900 KB per-thread array (2,001 campaigns by 14 days by 32 bytes), so it lives in L2, not L1.
- **Idea.** Count per (campaign, day) by intervals instead of per session. Keep, for every active campaign, the number of this person's sessions started today when it became active (`since[c]`). When a campaign leaves the window, or the day changes, or the person ends, add `sessions_today - since[c]` to the cell once. A campaign that stays active across 12 sessions of a day costs one add instead of 12.
- **Sources.** axhiao `1d1fa1c`: "assists batched per active-set change". jklein07 `d1036a8` and AbhirajTiwari12 `b90b316`: "interval assists" / "Assists counted by intervals". This repo's own earlier version did per-campaign interval unions and was replaced by the sliding window in `49305fe`; this keeps the sliding window and only changes when the cell is written.
- **Here.** `main.rs:949-970` already tracks entry (`acnt[c]` going 0 to 1) and exit (1 to 0) of each campaign. Record `since[c] = d_sess_before_this_session` on entry; on exit add `d_sess_before - since[c]` (the leaving campaign did not assist the current session, since exits are processed before the current session is counted; check this ordering against lines 949 to 970, where entries are processed first and exits second for the same `start`). At a day change and at the end of the person, flush every active campaign and reset `since`.
- **Model check.** A Python model of this bookkeeping (entries first, then exits, `since` reset to 0 for active campaigns at a day change, flush on exit, day change and end of person) against plain per-session counting: 0 mismatches over 60,000 random people with click offsets of exactly 7 days, 7 days plus and minus 1 ms, 0 and 1 ms after the start. Script: `/private/tmp/claude-501/-Users-nimaibhat-Documents-databricks-chall/273c3dc9-8aa1-4164-a68d-59b78a9a199a/scratchpad/chk2/assist.py`. In `process_partition` note that `d_sess += 1` (`main.rs:944`) runs before the click loops, so "sessions already counted today" is `d_sess - 1` there.
- **Expected gain.** Tens of milliseconds of CPU at 20M: about 1% of wall. Small; listed because the count is so large.
- **Risk.** Medium for its size: it touches the assist rule's boundaries (inclusive 7-day window at both ends, day of the session start). The fixtures in the A-series cover those; a wrong count shows up in `assisted_sessions` on the dev answer.

### 17.2 Where the sessions phase goes, by reading the code

- Per event: one `gmap.entry` (hash plus probe, `main.rs:792`), one `gid.push`, one scatter copy of 16 bytes. Per person: a sort (median person has 12 events and 3 sessions by joch44's profile, so nearly all sorts are insertion sorts of fewer than 20 elements), the sweep, and a `flush_user` per person-day.
- So the levers are, in order: 8-byte sort keys (entry 7.1, halves the copy and the sort moves); a cheaper hash (entry 1.7; `hk` is two multiplies and the map is L1-resident, so the hash is most of the probe cost); 16-byte click keys (entry 13.2); then assists (17.1). A sorting network or rank sort for small groups is next after those: cherylaurelia's AVX2 rank sort for up to 64 events took its sessions phase from 0.544 to 0.487 CPU-s (`ca9e524`).

### 17.3 Closed by the profile

*Partly retracted by [20.1](#201-output-bytes-may-be-paid-for-after-the-process-exits): the in-process output time is small, but another team measured a cost after exit that grows with output bytes.*

- **Output is 0.048 s of about 2.1 s in-process at 20M** (`OVERNIGHT_LOG.md`), so the CPU cost of encoding is not worth attacking.
- **Thread count, partition counts, chunk size**: three teams swept them and found nothing (entries 1.10, 11.3, 16.2).

---

## Iteration 18 (2026-11-21): the link table that makes a parse-time lookup cheap

The log's first queued change resolves the person in the parse loop. Entry 14.2 lists who measured that as a win and who as a loss; this entry records how the version that works is built, from [Vibhor7-7 `src/links.rs`](https://github.com/Vibhor7-7/shopping-sessions/blob/7a5ddaf/src/links.rs) (1.59 s on the log's runner, board 0.82 s).

### 18.1 One cache line per lookup: four keys, then their four people

- **Layout.** The table is an array of 64-byte lines. Words 0 to 3 of a line hold four ID keys and words 4 to 7 the four people they map to (`id_word(at) = (at >> 2) * 8 + (at & 3)`, value four words on). A key of 0 means a free slot.
- **Home.** `home(id) = (mix(id) >> shift) & !3`: every search starts at the first slot of a line, so the line that was prefetched is the line that is read.
- **Lookup** (`person_at`): one 256-bit load of the four keys, `cmpeq` against the ID and against zero, two `movemask`s. If either mask is non-zero, take the first slot that is the ID or free, load its value, and return `select_unpredictable(found, person, id)`; an unlinked ID therefore costs the same as a linked one and there is no hit-or-miss branch to mispredict. Only a full line of four other keys goes to a cold out-of-line probe of the next line.
- **Prefetch.** `prefetch(id)` returns `home` and issues the prefetch; the parser calls it for each line's user ID and consumes it 16 lines later (README: "each line asks for its cache line 16 lines before reading it").
- **Measured there.** Prefetching the link join -1.7% (experiment 4); forcing the select instead of a branch -2% and -3.2% of parse (experiment 6); the four-plus-four line layout -1.5% (experiment 14); a table twice as large +1.3%, because building it cost more than the lookups saved (experiment 7).
- **Union-find in the same table.** While the table is built, the value word holds the parent; the edge loop prefetches the home lines of the next edge's two IDs before processing the current one (`links.rs:260-261`), which is entry 7.3's prefetch.
- **Here.** The current structure is 512 `KMap<Key>` shards of 16-byte `(key, value)` slots at load 0.25 to 0.5, about 64 MB at 20M, probed with a hit-or-miss branch (`keys.rs:202-214`). For a parse-time lookup the same data wants: one flat table, the 4+4 line layout, a load factor near 0.5 at the line level, self-named IDs left out (entry 13.1), `EMPTY` (`u64::MAX`) or 0 as the free marker, and a ring of 16 pending `(record, home)` pairs per parse thread. At 20M there are about 1.8M linked IDs (892,711 link lines); without the self-named ones roughly 1.1M, so about 2.2M slots or 35 MB at load 0.5. That still exceeds L2 by far, which is why the prefetch ring is not optional.
- **Expected gain.** This is the enabling piece for deleting the resolve pass (0.175 s of about 2.1 s at 20M on the runner, 8%) when combined with 14.2; on its own it adds cost to the parse loop.
- **Risk.** Low for the table itself: a lookup returns the same person as `KMap::get(...).unwrap_or(uid)`. The ring adds a flush at chunk end, where the last 16 records must be drained before the thread's lists are read.

---

## Iteration 19 (2026-11-21): a lower-risk scanner step, and small-group sorting

### 19.1 Keep the separator index; split all fields of a line at once around its delimiters

- **Idea.** A middle path between today's scanner and the rewrite in 14.1. `pos` already holds the offsets of a line's delimiters in order. For each of them load the 4 bytes starting one byte before it: byte 0 is the last character of the field before the delimiter, byte 2 the first character of the field after it. Put the eight 4-byte loads in one 256-bit register, compare with `"` once, and every field's "wrapped in quotes" flag, start and length come out of a few vector operations.
- **Source.** [Vibhor7-7 `src/reader.rs`](https://github.com/Vibhor7-7/shopping-sessions/blob/7a5ddaf/src/reader.rs) lines 86 to 131 (`split::<N>`, experiment 22): eight scalar 32-bit loads inserted into two `xmm` halves (no gather instruction), `cmpeq_epi8` with a quote, a lane permute so each field sees its closing byte, `wrapped = open & close & (len > 1)`, then `start = before + 1 + wrapped` and `len = after - before - 1 - 2 * wrapped` stored as two arrays of 8. The commit message counts the old per-field loop at about 145 instructions per line. No A/B result is logged yet; AbhirajTiwari12 tried a variant with a real gather and reverted it.
- **Validity rule used there.** A line is well formed iff its total quote count equals twice the number of wrapped fields. Brute-force checked here against the `split` rule on all 797,161 strings of length 0 to 12 over `{a , "}`: 0 mismatches (`/private/tmp/claude-501/-Users-nimaibhat-Documents-databricks-chall/273c3dc9-8aa1-4164-a68d-59b78a9a199a/scratchpad/chk2/wrapped.py`). This is the same test as `bad == 0 && seen == qcount` at `parse.rs:636`, without the separate `qa ^ qz` term.
- **Here.** Replace only the loop at `parse.rs:621-635`. The delimiters of the line are `line_start - 1` (the previous newline) followed by `pos[first..first + N]`; `qcount` is still computed from the ring as now. Two edge conditions: the loads read 1 byte before the first delimiter and 2 bytes past the last one, so the first line of a file (nothing readable before offset 0 of a mapping) and a line ending within 2 bytes of the chunk's readable end go to `split`; the existing long-line fallback stays.
- **Expected gain.** The log's `perf` profile puts the per-field quote loop at 7.7% of self time; this removes most of it, so about 5% of wall, without touching how lines and separators are found.
- **Risk.** Low to medium: the line-finding and field-count logic do not change, and `fuzz_scan` compares the result with `split` directly.
- **Relation to 14.1.** Do this first if the rewrite looks too large; 14.1 additionally removes the flatten loop and the ring (the rest of the scanner's 490 instructions), so the two are alternatives for the quote loop but 14.1 goes further.

### 19.2 Branch-free rank sort for small groups

- **Idea.** For up to 8 keys, hold them in one 256-bit register, rotate it 7 times, and count for each lane how many other keys are smaller (ties broken by original position). The counts are the sorted positions, so the sort is `out[rank[i]] = in[i]` with a fixed trip count and no data-dependent branch. For 9 to 64 keys, compare each key against the whole array 8 at a time and popcount the masks.
- **Source.** [cherylaurelia `ranks_small`](https://github.com/cherylaurelia/builders-hackathon/blob/2e4261f/src/main.rs) lines 2463 to 2550; commit `ca9e524`: "AVX2 rank sort for the events and clicks of people with up to 64 of them, vector 7-day window ... sessions-phase cpu 0.544 to 0.487 s", so about 10% of that phase with both parts.
- **Here.** After entry 7.1 the per-person sort at `main.rs:895` is over `u64` words whose top 32 bits are `ts`; a rank sort on the 32-bit `ts` followed by a permuting copy does the same job. The median person has 12 events and 3 sessions ([joch44 `DATA_PROFILE.md`](https://github.com/joch44/shopping-sessions/blob/e71539c/DATA_PROFILE.md)), so most groups fall in the 9-to-64 path, where the cost is quadratic: 12 keys need 24 vector compares.
- **Expected gain.** 3 to 10% of the sessions phase, so at most 1.5% of wall. Do it after 7.1, which is larger and free.
- **Risk.** None to exactness: any order of equal `ts` gives the same output (entry 7.1).

---

## Iteration 20 (2026-11-21, 00:35): output bytes, measured after exit

### 20.1 Output bytes may be paid for after the process exits

- **Source.** [TejasviniChawla `docs/NOTES.md`](https://github.com/TejasviniChawla/shopping-sessions/blob/9338c54/docs/NOTES.md), two entries pushed tonight: "FOUND: wall minus in-container gap depends on OUTPUT BYTES on the bind mount (docker flushes dirty pages at exit): uncompressed parquet (23 MB) costs +0.15 s vs Snappy (9 MB). tmpfs output: no gap." Then: Snappy writer "-5% in-container, -3% wall, exit gap gone"; dictionary `user_id` plus INT32 counts neutral on top of Snappy; and "CI gains translate to the scorer at about 40%: large fixed cost (container start + output flush) on the scorer."
- **Counter-evidence.** Vibhor7-7 experiment 17: writing 222 MB to disk added about 35 ms after the process under GitHub's Docker (entry 1.8). The two measurements differ by more than an order of magnitude per MB, so this depends on the host's filesystem and writeback settings. The scorer's `/output` is reported as ext4 (entry 11.1); nobody has published the gap there.
- **Why it matters here.** This build writes about 96 MB of `user_daily` at 20M (42.9 bytes per row, audit §3) and about 280 MB at 60M. cherylaurelia, the fastest build on the log's runner, writes 24.7 MB at 20M after `DELTA_BINARY_PACKED` and then shrinks `user_id` further with a run dictionary (`88ed99d`, `3458d27`). The log's own numbers leave room for this: 2.32 s for `docker run` against about 2.1 s in-process, with the difference attributed to "container start/exit".
- **How to find out in one runner job.** Record three times per run: process start to the "done" line, "done" to process exit, and process exit to `docker run` returning. Then A/B (a) `/output` on the bind mount against a tmpfs mount, and (b) the normal output against the same binary with the `user_daily` write cut to a tenth of the rows (output is then wrong, so timing only). If the last interval moves with bytes, output size is a first-class cost; if not, entry 17.3 stands.
- **If it does move, the order of work in `out.rs` and `pq.rs`** (bytes per row from entry 5.2; reader support from 5.1, all readable by the polars checker in `scripts/run_fixtures.py`):
  1. INT32 PLAIN for `sessions` and `events`: 42.9 to about 35 B/row. No reader risk.
  2. `DELTA_BINARY_PACKED` for `day`, `sessions`, `events`, `revenue_cents`: about 20 B/row down to about 3 to 4, since rows of one person are adjacent, `day` rises slowly and 82% of rows have zero revenue. Layout rules in entry 5.3.
  3. `user_id`: 4-byte length prefix plus about 9 bytes per row today. `DELTA_LENGTH_BYTE_ARRAY` removes the prefixes (about 13 to about 9.2 B/row); a dictionary over runs of equal IDs removes the repeats (2,242,393 rows for about 667,000 people at 20M, so about 3.4 rows per person: roughly 13 to about 5 B/row).
  4. `top_category` as a dictionary: about 2 B/row to about 0.1.
  Steps 1 to 4 together are roughly 43 to about 10 B/row, in line with cherylaurelia's 24.7 MB.
  A general compressor (Snappy) gets a similar size with one code path, but it needs either a crate or a hand-written encoder and a `codec` change in every column chunk.
- **Expected gain.** Unknown until measured: between nothing (Vibhor7-7's Docker) and several tenths of a second (TejasviniChawla's bind mount, scaled to 96 MB).
- **Risk.** Step 1 none. Steps 2 to 4 are the reader risk discussed in 5.1 to 5.3: read every output back with polars 2.1.0, pyarrow and duckdb, and keep the dev-answer comparison in the gate, since `compare.py` reads through polars.

### 20.3 Measured sizes of the 20M output under each encoding

Measured offline by re-encoding the optimizer's golden 20M `user_daily` (`~/ss-data/gold20`, 2,242,393 rows in 10 files, rows in the order this build writes them) with pyarrow 26 into the scratchpad. This measures bytes only; it does not time anything, and pyarrow's encoders stand in for a hand-written one. Script: `/private/tmp/claude-501/-Users-nimaibhat-Documents-databricks-chall/273c3dc9-8aa1-4164-a68d-59b78a9a199a/scratchpad/pqsize/size.py`.

| Layout (v1 pages, no statistics) | Size | B/row | `user_id` | `day` | `sessions` | `events` | `revenue` | `top_category` |
|---|---|---|---|---|---|---|---|---|
| PLAIN, INT64 counts (as written now) | 96.4 MB | 43.0 | 13.0 | 4.0 | 8.0 | 8.0 | 8.0 | 2.0 |
| PLAIN, INT32 counts | 78.5 MB | 35.0 | 13.0 | 4.0 | 4.0 | 4.0 | 8.0 | 2.0 |
| `DELTA_BINARY_PACKED` on the four integer columns | 42.5 MB | 18.9 | 13.0 | 0.7 | 0.4 | 0.7 | 2.2 | 2.0 |
| plus `DELTA_LENGTH_BYTE_ARRAY` on `user_id` | 33.9 MB | 15.1 | 9.1 | | | | | 2.0 |
| plus dictionary on `top_category` | 30.0 MB | 13.4 | 9.1 | | | | | 0.3 |
| `DELTA_BYTE_ARRAY` on `user_id` instead | 21.7 MB | 9.7 | 3.7 | | | | | 2.0 |
| PLAIN INT32 with Snappy | 22.2 MB | 9.9 | 3.9 | 1.6 | 0.8 | 1.8 | 1.0 | 0.7 |
| Delta, delta-length, dictionary, with Snappy | 14.9 MB | 6.6 | 3.5 | 0.6 | 0.3 | 0.7 | 1.2 | 0.3 |
| PLAIN INT32 with Zstandard | 10.9 MB | 4.9 | 1.9 | 0.7 | 0.3 | 0.9 | 0.6 | 0.4 |

Facts from the same read: 667,450 distinct `user_id`s (3.36 rows per person), 82.4% null `top_category`, largest `revenue_cents` 34,488,578, largest `events` 29,232, largest `sessions` 19.

What this says:

- **One encoder buys most of it.** `DELTA_BINARY_PACKED` on `day`, `sessions`, `events` and `revenue_cents` alone takes the file from 96.4 to 42.5 MB (-56%). It is the simplest of the encodings in entry 5.3 and has no compression dependency.
- **`user_id` is then 70% of what is left.** A dictionary over distinct values does not help (pyarrow's came out at 10.7 B/row). `DELTA_BYTE_ARRAY` does (3.7 B/row), because a person's rows are adjacent and each repeat costs a prefix length and an empty suffix. It is built from the same delta encoder (prefix lengths, then suffixes as delta-length byte arrays). Polars lists it as supported (entry 5.1); it is the least commonly written of these encodings, so it needs the read-back check more than the others.
- **Delta integers plus `DELTA_BYTE_ARRAY` plus a `top_category` dictionary would be about 8 B/row, roughly 18 MB**: a fifth of today's bytes with no compressor. Snappy on plain INT32 pages lands at the same size with one mechanism but needs an encoder.
- `revenue_cents` fits in 32 bits for this dataset by a factor of 60, but a whale's day can exceed any assumed bound at 60M, so it stays INT64 (delta packing makes the width irrelevant anyway).

### 20.4 A reference byte layout for the delta encodings, accepted by three readers

- **What exists.** `/private/tmp/claude-501/-Users-nimaibhat-Documents-databricks-chall/273c3dc9-8aa1-4164-a68d-59b78a9a199a/scratchpad/pqsize/refwriter.py`: a 130-line Python port of `src/pq.rs` (same Thrift footer, one row group, one v1 page per column) with `DELTA_BINARY_PACKED` (encoding 5) for INT32 and INT64, `DELTA_LENGTH_BYTE_ARRAY` (6) and `DELTA_BYTE_ARRAY` (7) for strings, including the optional string column. It is a specification to port, not code for the repo.
- **Checked.** Files of 0, 1, 2, 31, 32, 33, 127, 128, 129, 130, 257, 1,000 and 40,000 rows, in four encoding mixes, with values that include negatives, `2^31 - 1`, `2^62` and empty strings: pyarrow 26.0.1, polars 2.1.0 (the version the starters pin) and duckdb 2.0.1 all return exactly the rows written. On the first golden 20M `user_daily` file (218,405 rows) the layout with delta integers and `DELTA_BYTE_ARRAY` `user_id` is 2,093,250 bytes against 9,376,877 now (42.9 to 9.6 B/row), and polars reads it equal to the original.
- **The layout, as it passed.**
  - Header: `uleb(128)`, `uleb(4)`, `uleb(total values)`, `zigzag(first value)`.
  - Then blocks of up to 128 deltas (wrapping in the column's width): `zigzag(min delta)`, four bit-width bytes (0 for unused miniblocks), then each used miniblock as 32 values of `delta - min` packed LSB first at its width, the last one padded with zeros to 32 values. Unused miniblocks have no body.
  - A column with zero values is just the header with a zero first value.
  - `DELTA_LENGTH_BYTE_ARRAY`: the lengths as a delta-packed INT32 stream, then all bytes concatenated.
  - `DELTA_BYTE_ARRAY`: prefix lengths as a delta-packed INT32 stream, then the suffixes as a delta-length byte array.
  - Page header and column metadata only change in the encoding id (`PageHeader.data_page_header.encoding`, and the `encodings` list `[RLE, 5]` and so on); the definition levels of the optional column stay as they are, in front of the values.
- **Where it goes.** `pq.rs:130-159` (the three `match` arms that build `page`) and `:216-218` (the encodings list). `out.rs:48-50` would pass `sessions` and `events` as 32-bit.
- **Not checked.** Spark's reader (the notes in 5.1 say 4.0.1 supports these); pages larger than about 10 MB with these encodings.

### 20.2 Other new commits

- [Vibhor7-7 `80df6c5`](https://github.com/Vibhor7-7/shopping-sessions/commit/80df6c5) (experiment 23, no number yet): `ingest_ts` validity checked with ordinary compare-and-branch instead of a folded flag, because unlike `ts` its form is always F1 and the branches predict; its whole seconds come straight from the parse "instead of from multiplying by 1000 and dividing again". This repo already keeps ingest in seconds (`parse_ingest_s`).
- TejasviniChawla `99963ee`: a benchmark of parse stages isolated with `black_box`; `9338c54`: an `EXIT_NOW` switch and a CI job that measures container overhead. No results recorded yet.
- In this repo: `e7a116f` (00:22) implemented entries 1.1 and 8.2 and the log (00:35) records it as kept at -2.3%. The log also notes that on its second runner (Xeon 8573C) container start and stop alone took about 0.5 s against about 0.2 s on the first; that fixed part is where a cost after exit (20.1) would hide.

## Next iteration

Each cycle: re-read `OVERNIGHT_LOG.md` and `git log` for new experiments, pull the 18 competitor clones for new commits, then one deep read from this queue:

- Vibhor7-7's reader (`src/reader.rs` from line 161): how 256 KB blocks carry partial lines, for the `pread` A/B.
- micah-andelman `scan2` and joch44 `split_masked`: two more implementations of the mask split to compare with 14.1.
- New repos and scores from the public search API.
