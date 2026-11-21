# RESEARCH_NOTES.md

Research-only notes for the optimizing session. Nothing here has been built or benchmarked by me; every number is quoted from the source named next to it. Line numbers refer to `src/` as read on 2026-11-21 (HEAD `1c4fa8e` plus uncommitted edits to `keys.rs`).

`OVERNIGHT_LOG.md` did not exist when iteration 1 ran, so "tried or rejected" was taken from `git log`, `speedups.md` and `repo_audit_and_profile.md`.

## Top five untried ideas (ranked by expected gain per unit of correctness risk)

| # | Idea | Where | Expected gain | Risk to exactness | Entry |
|---|---|---|---|---|---|
| 1 | Look up the product category only for purchase rows (3% of events) | `parse_events`, `main.rs:632` | Removes one dense-table load + digit conversion from 97% of valid event lines; audit put this lookup at 37 to 40 ns of a 163 ns line before `1c4fa8e` | None: the category byte is read only when `price != 0` | [1.1](#11-product-lookup-only-for-purchases) |
| 2 | Software write-combining with non-temporal stores for the 512-way scatters | `BList::push` in `mem.rs:128`, callers in `parse_events`, `parse_clicks`, `dedup_partition` | Competitor measured -12% total, -16% parse on an Ice Lake Xeon (the scorer's class); -6% on Zen 4 | Low: a flush bug loses or duplicates records, which the 20M cross-check catches | [1.2](#12-write-combining-buffers-with-non-temporal-stores) |
| 3 | Route valid events straight to their user partition at parse; dedup only marks losers | `parse_events`, `dedup_partition`, `resolve_partition` | Removes dedup pass 2 (a second 512-way scatter of every survivor); competitors report a 19 to 26 ms dedup phase at 20M | Medium: restructures phase 3; invalid copies must still take part in dedup | [1.3](#13-dedup-that-marks-losers-in-place) |
| 4 | Known-date table for F1/F2 timestamps and `ingest_ts` | `civil_swar` and `parse_ingest_s`, `parse.rs:772` and `:889` | Estimate 2 to 5% of parse CPU; three competitors ship it | Low: exact 11-byte compare, full parser on a miss | [1.4](#14-known-date-table-for-timestamps) |
| 5 | `pread` in L2-sized blocks instead of mapping the input (a new variant of `IOMODE=4`) | `chunk_data`, `parse.rs:246` | Competitor measured mmap 7.5% slower than `read` on a cloud VM; unmeasured here at 8 threads | Low to medium: line seams and the `floor`/`ceil` guards | [1.5](#15-pread-in-l2-sized-blocks) |

Runners-up, in order: sessions from 30-minute bins instead of a sort ([1.6](#16-sessions-from-30-minute-bins)), one-multiply hash ([1.7](#17-one-multiply-hash)), smaller Parquet ([1.8](#18-smaller-parquet-output)), `readahead(2)` tuning for a cold cache ([1.9](#19-cold-cache-read-ahead)).

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

## Next iteration

- Re-read `OVERNIGHT_LOG.md` if it exists and drop anything it marks tried.
- Primary sources still to open: non-temporal store behaviour and buffer sizing (Intel optimization manual; Balkesen et al. ICDE 2013 on software-managed buffers), `read` against `mmap` under virtualization (EPT page-walk cost), and the remaining competitor repos (Th3r0n `src/arena.rs`, AbhirajTiwari12, joshuamuellenbach25) for anything the five read so far do not cover.
