# RESEARCH_NOTES.md

Research-only notes for the optimizing session. Nothing here has been built or benchmarked by me; every number is quoted from the source named next to it. Line numbers refer to `src/` as read on 2026-11-21 (HEAD `1c4fa8e` plus uncommitted edits to `keys.rs`).

`OVERNIGHT_LOG.md` did not exist when iteration 1 ran, so "tried or rejected" was taken from `git log`, `speedups.md` and `repo_audit_and_profile.md`.

## Top five untried ideas (ranked by expected gain per unit of correctness risk)

| # | Idea | Where | Expected gain | Risk to exactness | Entry |
|---|---|---|---|---|---|
| 1 | Look up the product category only for purchase rows (3% of events) | `parse_events`, `main.rs:632` | Removes one dense-table load + digit conversion from 97% of valid event lines; audit put this lookup at 37 to 40 ns of a 163 ns line before `1c4fa8e` | None: the category byte is read only when `price != 0` | [1.1](#11-product-lookup-only-for-purchases) |
| 2 | Software write-combining with non-temporal stores for the 512-way scatters | `BList::push` in `mem.rs:128`, callers in `parse_events`, `parse_clicks`, `dedup_partition` | Competitor measured -12% total, -16% parse on an Ice Lake Xeon (the scorer's class); -6% on Zen 4 | Low: a flush bug loses or duplicates records, which the 20M cross-check catches | [1.2](#12-write-combining-buffers-with-non-temporal-stores) |
| 3 | Route valid events straight to their user partition at parse; dedup only marks losers | `parse_events`, `dedup_partition`, `resolve_partition` | Removes dedup pass 2 (a second 512-way scatter of every survivor); competitors report a 19 to 26 ms dedup phase at 20M | Medium: restructures phase 3; invalid copies must still take part in dedup | [1.3](#13-dedup-that-marks-losers-in-place) |
| 4 | Sort 8-byte `(ts << 32) \| pc` keys per person instead of 16-byte `SRec`s | `process_partition`, `main.rs:814-830` and `:893-896` | Estimate 10 to 25% of the sessions phase, 2 to 4% of wall; halves the scratch buffer | None: `person` is constant within a group and equal-`ts` order does not affect any output | [7.1](#71-sort-8-byte-keys-per-person) |
| 5 | Known-date table for F1/F2 timestamps and `ingest_ts` | `civil_swar` and `parse_ingest_s`, `parse.rs:772` and `:889` | Estimate 2 to 5% of parse CPU; three competitors ship it | Low: exact 11-byte compare, full parser on a miss | [1.4](#14-known-date-table-for-timestamps) |

Runners-up, in order: splitting each line from its own masks ([2.1](#21-split-each-line-from-its-own-masks), 3 to 8% but it rewrites the routine that decides which lines are malformed), `pread` in L2-sized blocks ([1.5](#15-pread-in-l2-sized-blocks), demoted in iteration 2: three teams measured mmap faster, two measured `read` faster), sessions from 30-minute bins instead of a sort ([1.6](#16-sessions-from-30-minute-bins)), one-multiply hash ([1.7](#17-one-multiply-hash)), smaller Parquet ([1.8](#18-smaller-parquet-output)), `readahead(2)` tuning for a cold cache ([1.9](#19-cold-cache-read-ahead)).

## A finding to act on before trusting the TSBL toggle

`parse_ts_bl` (`parse.rs:819`) still has the pre-fix mask: `SC = 0x0000_00FF_0000_0000` and fill `0x3000_0030_0000_0000` at lines 829, 834 and 837. That is the same bug `5b17560` fixed in `civil_swar`: the colon at byte 13 is not neutralized, so `digits8(c2)` is false, `civ_ok` is false, and every F1 and F2 input falls through to the scalar `parse_ts`. Tests pass because the fallback is exact. Any timing of `TSBL=1` so far measured the scalar parser plus wasted SWAR work, so a "TSBL is slower" verdict is not evidence about the branch-free design. The fix is the same three constants (`SC = 0x0000_00FF_0000_FF00`, separator `0x0000_003A_0000_3A00`, fill `0x3000_0030_0000_3000`) plus a test that asserts the non-fallback branch is taken.

`parse_ingest_s` (`parse.rs:894-897`) has the old constants too, but it is correct: it only reads the seconds pair from that word, and the stray colon cannot carry into it.

## What is already tried, rejected or queued (do not re-propose)

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

- **PEXT ID packing.** Vibhor7-7 experiment 2 measured -8.6%, but against a byte loop. `pack_fast` already removes the serial chain, and the audit prices both packs at about 10 ns per line. cherylaurelia's two-IDs-in-one-256-bit-op version gained about 2% (`e3bdd51`).
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

## Next iteration

- Re-read `OVERNIGHT_LOG.md` if it exists and drop anything it marks tried.
- Primary sources still to open: non-temporal store behaviour and buffer sizing (Intel optimization manual; Balkesen et al. ICDE 2013 on software-managed buffers), `read` against `mmap` under virtualization (EPT page-walk cost), and the remaining competitor repos (Th3r0n `src/arena.rs`, AbhirajTiwari12, joshuamuellenbach25) for anything the five read so far do not cover.
