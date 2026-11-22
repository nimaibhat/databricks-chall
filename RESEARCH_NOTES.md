# Research notes: opaque-ID path (`src/v3b.rs`)

Written for the coding agent. These notes cover the rules as changed on 2026-11-21 (opaque variable-width IDs, final on about 60M events). Everything written for the old ID format was removed from this file; it is in git history up to `0f4ee43` and none of it is evidence for the new path.

Read against `7de923f` (upstream `7c69e3e` plus the input release and the two constants) and `OVERNIGHT_LOG.md` up to experiment 31. `sess_thread` and `person` are unchanged since `fcd9075`; line numbers below for those two are for `7de923f` (`person` starts at line 1732, `sess_thread` at 1550). Older sections further down still carry `fcd9075` line numbers and are marked closed where the log closed them.

**Numbers:** "log" means measured by the coding agent and quoted from `OVERNIGHT_LOG.md`. "estimate" means my arithmetic, not a measurement. I cannot run the binary.

**Which size to tune for:** the practice board runs 20M, the final runs about 60M. Where a setting helps one size and hurts the other, the 60M result is the one that decides the final score.

## Top five untried (after experiment 31)

Experiment 31 closed my whole first table (see the log at the bottom). What it left is one hard number: at 20M the sessions stage is 1,632 thread-ms, of which `person` is 852 and the grouping loop 578 (log). Counting the operations in `person` by hand I can account for about 300 of the 852. So the first item is a measurement, and the next three are the candidates it would choose between.

| # | Idea | Where | Expected at 20M | Row risk |
|---|---|---|---|---|
| 1 | Turn the cycle counters in `person` back on for one diagnostic run and paste `[v3b] st` | `person`: `c0`…`c4` are all the constant `0u64` now, so "person Mcycles" prints zeros | No gain itself; splits 852 thread-ms into buckets / rows and purchases / click sort / click walk | None (diagnostic build only) |
| 2 | Bitmask assists for people with at most 64 sessions | `person`, the `brute` branch and the loop over `t.sess` | Up to −20 ms if the click walk is most of the unexplained 550 thread-ms (estimate, depends on 1) | Moderate: new logic on `assisted_sessions` |
| 3 | Category totals without the 32-wide table per row | `person`, the `if !t.sb.pp.is_empty()` block | −3 to −5 ms (estimate) | Low |
| 4 | One 32-byte entry per ID in `KTab` instead of separate `keys` and `vals` arrays | `KTab` (struct, `get`, `insert`, `grow`) | −3 to −8 ms off the 578 thread-ms grouping loop (estimate) | None |
| 5 | Confirm the fallback path at 60M (output, time, memory) | one run with a forced fallback | Safety, not speed | — |

### 1. The measurement

`person` has timing points `c0`, `c1`, `c2`, `c3`, `c4` that feed `prof[13..15]` and `st[8..10]`, but each is `let cN = 0u64;`. Put `unsafe { core::arch::x86_64::_rdtsc() }` back for one build and paste the `[v3b] prof` and `[v3b] st` lines at 20M. The useful fields:

- `prof[13]` bucket pass, `prof[14]` rows and purchases, `prof[15]` everything to do with clicks.
- `st[9]` click sort, `st[10]` click walk including brute-force assists.
- `st[5]` brute-force inner iterations, `st[6]` assists actually added, `st[7]` people on the non-brute path, `st[11]` timestamp ties.

My hand count for the 852 thread-ms, all estimates: bucket pass 80, bucket scan and session rows 85, name text 27, purchases 40, click sort about 60, leaving roughly 550 that I can only place in the click walk and the accumulator updates. Experiment 31 supports that reading indirectly: turning brute force off entirely (`XL_PB0=0`) cost 15 ms of wall, so the other assist method is dearer still, and assists are expensive by either method.

### 2. Bitmask assists (do after 1, only if `st[10]` is large)

For a person with at most 64 sessions (nearly everyone; whales keep the existing non-brute path):

1. Sessions are already in start order. Build one 64-bit mask per day the person has sessions on (`day_mask[d]`, at most 15).
2. Walk the clicks in time order (`k64` is already sorted). For each click, the sessions it can assist are the contiguous index range from the first session starting at or after the click to the last session starting within 7 days of it. Both ends only move forward as clicks advance, so two indices suffice.
3. OR that range into `mask[campaign]`. Use the existing `stamp`/`serial` trick to reset a campaign's mask the first time this person touches it, and keep a short list of the campaigns touched.
4. For each touched campaign and each day: `acc.assist(slot(c, day), popcount(mask[c] & day_mask[day]))` when non-zero.

Cost is about `clicks + touched campaigns × days`, against `sum over sessions of clicks in the window` now. The credited campaign (latest click, smallest `click_id` on a tie) is untouched: keep that code as it is.

Exactness: a session is assisted by campaign `c` exactly when some click of `c` has `start − 7d <= click.ts <= start`, which is the same as the session index lying in that click's range; the OR over clicks is "at least one"; the popcount per day is the number of such sessions that day. Boundary cases to test are the ones the generator plants: a click exactly 7 days before, 7 days + 1 ms before, exactly at, and 1 ms after a session start.

### 3. Category totals

Today, for every person with a purchase, `sb.cs` is resized to `rows × 32` zeroed words and then every row with revenue scans all 32. Most people have one or two purchases. Cheaper: sort `sb.pp` entries by `(row, category)` after mapping each to its row (it is a handful of items), then one pass sums runs and keeps the best per row; ties already resolve to the lowest category index because the scan order is ascending. For people with many purchases keep the table. Row risk is low but not none: `top_category` ties must still go to the lowest index.

### 4. `KTab` layout

Each event does `slots[s]` then `keys[i]` then `vals[i]`: three dependent loads from three arrays. Storing `(Key, u32)` together in one 32-byte, 32-aligned entry makes it two loads and one cache line. The table holds about 700 IDs per partition, so everything is in L1 or L2 already and the gain is only the shorter dependency chain; hence the small estimate.

### What I do not have an idea for

The grouping loop (578 thread-ms, 29 ns per event per logical CPU) is three passes: look up the person, count, scatter. I looked for a way to fold the scatter and the bucket pass into one probe per event; the per-partition table it needs costs a sort that cancels the saving. I have no proposal there beyond item 4.

## Earlier analysis (experiments 27 to 29), kept for reference

### What the log said about where time goes

From experiments 27 and 28 (log), 20M, `k1s`:

- Stages: parse 273, dedup 87, join 72, sessions 205, write about 10. Wall 671 to 681.
- Wall is about 35 ms above the in-process total.
- Profile: `sess_thread` 26.8%, stage closures 11.1 + 9.3 + 3.7%, `kernel_init_pages` 9.4%, `ev_batch` 8.8%, `ev_loop` 8.7%, `ev_lf` 5.1%.
- Peak memory 3.9 GB at 20M, 11.0 GB at 60M.
- Partition-count knobs (`XL_V3T1/M1/T2/M2`, `XL_EK`) were swept; nothing wins at both sizes.

Three things stand out and drive the table:

1. **35 ms outside the stages.** Setup is about 3 ms (old log). The rest is the kernel tearing down 3.9 GB at exit, on one thread.
2. **`kernel_init_pages` at 9.4%.** That is about 0.5 CPU-seconds of zeroing fresh pages, roughly 60 ms of wall if spread evenly. Every lane byte is written twice: once by the kernel, once by the record store.
3. **`sess_thread` at 26.8%.** That is about 70 ns per event per logical CPU (estimate: 26.8% of 5.4 CPU-s over 21M events). The operations I can see in the source should cost a third of that, so something in there is slower than it looks. The `[v3b] prof` and `[v3b] st` lines would say what; see "Measurements I need".

## Done: release each input chunk after parsing it (experiment 29)

**What.** In `parse_thread` (line 923), after the `match f.kind { ... }` block finishes a chunk, call `madvise(MADV_DONTNEED)` on the page-aligned inside of the chunk's byte range, exactly as the old path does at `src/parse.rs:2903–2907`:

```rust
let a = (f.off + s + 4095) & !4095;
let b = (f.off + e) & !4095;
if b > a { unsafe { madvise(g.region.add(a) as *mut u8, b - a, MADV_DONTNEED) }; }
```

**Why it should help.** The new path never releases the input mapping, so about 2 GB of file pages (6 GB at 60M) stay mapped until `_exit`, and the kernel unmaps them page by page on a single thread while the scorer's clock is still running. The old path released chunks as it went: its log shows RSS at exit of 305 MB and the process gone 1.5 to 2 ms after `exit at`. Releasing per chunk moves that work into the eight parse threads, where it runs in parallel.

**Estimate and what it rests on.** The 35 ms gap is from the log. The split is mine: about 3 ms setup, about 2 ms process start, about 30 ms teardown, of which the 2 GB file mapping is the larger part because it is mapped with 4 KiB pages (about 500,000 page-table entries) while the anonymous lanes are mostly huge pages. I put the gain at 15 ms at 20M and 45 ms at 60M. Experiment 26 is consistent: wall minus stage sum was 31 ms at 20M and 124 ms at 60M.

**Risk.** None to output. The mapping is read-only and file-backed, so a page touched again after release is simply mapped again from the page cache. `odd_key` reads the input only while its own chunk is being flushed, before the release.

**Result (log, experiment 29).** Kept as `m1`. 20M: 682.6 to 668.2 median (−14). 60M: 2084 to 2047 (−37). Peak RSS 3.9 to 2.0 GB at 20M and 11.0 to 5.0 GB at 60M. The exit gap is gone; parse grew by 10 ms in-process because the unmapping now happens there, in parallel.

## Closed: recycle finished event-lane chunks for the user lane (done upstream in `7c69e3e`, experiment 30)

**What.** During dedup, once a thread has finished partition `p` (after the final `emit_rec` loop, line 1251), every full 4 KiB chunk listed in `me.ssl` and `me.wsl` for that partition is dead: nothing reads it again. Push those chunk addresses (only entries with length 512 words, which are whole chunks; not the partial last chunk and not the staging area) onto a per-thread free list. In `WBins::chunk` (line 77), pop from that free list before carving a new chunk from `anon(BLOCK)`. Only `t.u2` needs the free list; the simplest form is a raw pointer field on `WBins` that is set for `u2` and null for the others.

**Why it should help.** The user lane (`u2`) is about the same volume as the compact event lane (16 bytes per event each). Today `u2` takes fresh memory, about 0.35 to 0.4 GB at 20M, and the kernel zeroes it first. With recycling, after each thread's first partition nearly all of `u2` lands in chunks that are already mapped and need no zeroing.

**Estimate and what it rests on.** `kernel_init_pages` is 9.4% of samples (log), about 0.5 CPU-s for roughly 1.4 GB of fresh anonymous memory (my count of lanes, `u2`, moved lanes and session buffers). That is about 3.5 GB/s per thread, which matches the VM's measured write bandwidth of 30 GB/s over 8 threads (old log). Removing 0.4 GB of it saves about 0.11 CPU-s, about 14 ms of wall at 20M and about 40 ms at 60M. Peak memory drops by the same 0.4 GB (1.1 GB at 60M), which also shortens teardown.

**Risk.** Low, and of one kind only: a chunk recycled while something still points into it. The pointers into a partition's chunks are `me.ssl`, `me.wsl`, `me.cptr` and `me.cands[..].rec`, all owned by the thread processing that partition and all finished by line 1251. Recycle after that line and not before. `WBins::flush` needs chunks 4 KiB-aligned, which lane chunks are. The recycled chunks are written with non-temporal stores and `u2.finish()` already issues the `sfence`.

**Test.** Gate as usual, plus NCPU 1 and 2 (with one thread the recycling lag is the most extreme). Watch `minflt` in the `[v3b] pa` line: it should drop.

## Closed: largest partitions first (experiment 31: sessions stage has no idle tail at 20M)

**What.** Before the join `run_par`, compute each second-stage partition's size (sum over threads of `u2.b` `nstages` for that partition, plus the click lane's), sort partition indices by size descending into a `Vec<u32>`, and have `join_thread` and `sess_thread` claim `order[cx.next.fetch_add(1)]` instead of the raw index. Dedup partitions are split by event ID and are even, so they need nothing.

**Why it should help.** Second-stage partitions are split by person. The rules give 5 whales with 1 to 2% of all events each, so a whale's partition holds about 300,000 events at 20M against about 10,000 for an average partition (900,000 at 60M). At the sessions stage's measured rate that is about 23 ms of single-thread work at 20M and 70 ms at 60M (estimate from the stage time in the log). Partitions are claimed in index order, so if a whale partition is claimed near the end, one thread works on it while seven sit idle. Largest-first makes that impossible.

**Estimate.** With 5 whales and a 23 ms tail in a 205 ms stage, the chance one lands in the tail is roughly 45%, and the average loss when it does is about half the tail: about 5 ms on average at 20M, 15 ms at 60M. The larger benefit is that a seed with an unlucky whale position no longer costs 20 to 70 ms. A `--features tl` run would show the idle time per stage directly.

**Risk.** None. Partition order affects only which thread's row buffer a row lands in, and row order does not matter.

## Closed: `XL_PB0` sweep (experiment 31: flat from 256 to 4096, worse outside)

**What.** `person` (line 1596) has two exact ways to count assists. The brute-force one (lines 1764–1777) runs when `clicks × sessions <= KN[0]` (default 4096, set by `XL_PB0`): for every session it walks every click in the 7-day window. The other (lines 1783–1823) sorts by campaign and uses binary search.

**Why I suspect it.** Activity is Zipf-like, so the sum of `clicks × sessions` over people is dominated by heavy users, and the brute path accepts people with, say, 64 clicks and 64 sessions. `st[5]` in the `[v3b] st` line is the total number of inner iterations. If it is in the tens of millions, this loop is a real part of the 26.8%.

**Test.** Paste the `[v3b] st` line, then try `XL_PB0` at 256, 1024 and 16384 at both sizes. No rebuild needed.

**Risk.** None: both paths already run on every dataset (whales take the second) and pass the gate.

**If `st[5]` is large,** there is a cheaper exact method for people with at most 64 sessions, described in idea 6.

## Moot: account for the anonymous memory (peak is 4.0 GB at 60M after chunk reuse)

**What.** After experiment 29 the input no longer counts toward peak RSS, so the 2.0 GB peak at 20M (log) is all anonymous memory. My count of what `v3b` writes is about 1.3 GB: compact event lane 0.26, wide event lane 0.16, click lane 0.14, user lane 0.36, moved lanes 0.15, rows 0.1, partial chunks about 0.15. That leaves roughly 0.5 to 0.7 GB unexplained. `kernel_init_pages` at 9.4% of samples points the same way: about 0.5 CPU-s of zeroing is more than 1.3 GB should cost at the VM's write bandwidth.

**Where to look.** Setup still builds everything the old path needed before `v3b::run` starts: the two arenas (`src/setup.rs:1138–1139`), `utab`, `claim`, `csr_off`, `csr` and `edges` (lines 1188–1196). These are lazy mappings and cost nothing unless something touches them. The question is whether any helper job, populate call or initialisation loop still touches them when `v3b::ON` is set. The `tl` feature records allocations by call site (`mp::add`), which would answer this directly; so would comparing `minflt` in the `[v3b] pa` line against the lane chunk counts printed beside it.

**If it is confirmed,** skip that work when `v3b::ON` is set. Zeroing 0.6 GB costs about 0.17 CPU-s, about 20 ms of wall (estimate).

**If the count is simply wrong** (for example the wide lane is larger than I assumed), the per-site table says where the memory really goes, which is what idea 1 and any further lane shrinking need anyway.

**Risk.** None: this removes work whose results the new path never reads.

## Closed: smaller staging block in `WBins` (experiment 31: 8 and 56 both slower than 24)

**What.** `SW` (line 48) is 24 words, so each partition's staging block is 256 bytes with its header. With 2,048 second-stage partitions that is 512 KB per thread per lane, and two hyperthreads share a 1 MB L2. Try `SW = 8` (128 bytes per partition, one cache line per flush) and `SW = 12`. `CW % SW == 0` must hold: 504 divides by 8 and 12, not by 16.

**Why it might help.** `WBins` feeds `u2` (dedup), the click lane (parse) and the moved lanes (join). The event lanes already use a 64-byte stage per partition (`ELane`), which suggests upstream found small stages better there.

**Why it might not.** More padding words when a 3- or 5-word record does not fit the remainder, and more flush calls. I cannot predict the sign.

**Risk.** None: `push` pads generically and the widest record is 5 words.

## Lower-ranked ideas

**6. Bitmask assists for people with at most 64 sessions (only if `st[5]` is large).** Sessions are already in time order. For each click, in time order, the sessions it assists are a contiguous index range (first session at or after the click, up to the last session within 7 days), found with two pointers that only move forward. OR that range into a 64-bit mask kept per campaign (the existing `stamp` array can mark which campaigns are live). Then for each campaign, AND its mask with each day's session mask and add the popcount to `acc.assist`. Cost is about `clicks + campaigns × days` instead of `clicks × sessions × 0.4`. Row risk is moderate because it is new logic on the assist column; it needs the gate and a 20M and 60M comparison.

**7. Release dead anonymous memory in parallel before exit.** Experiment 29 reports the exit gap gone on the VM, so this is closed there. It would matter only if the scorer has transparent huge pages off, which I cannot see.

**8. Pre-size the row buffers.** `Rows` vectors and `names` grow by doubling, and the custom allocator's realloc is allocate-copy. Reserving from an estimate avoids the copies and some fresh-page zeroing. Estimate: 2 to 4 ms. No row risk.

**9. The 23% of lines with a long ID.** One in eight distinct IDs is long, so 1 − (7/8)² = 23.4% of event and click lines carry at least one. Each of those leaves the assembly loop through `ev_cold` (`src/k_free.rs:1650–1672`), which saves and restores eight mask registers, and then `ev_long` (line 1438) recomputes the comma, quote and newline masks for 128 bytes. I cannot size this without a profile; `ev_long` is absent from the top symbols in experiment 28, which suggests it is not large. `XL_V3SKIP=1` (parse with the event sink disabled) would give the scanner's share of the 273 ms.

## Already done upstream (do not redo)

These were on my list after reading `93e9168` and are all in `ff78d19`: 16-byte compact event records; write-combined non-temporal lane stores; dedup that only builds a hash table for bitmap-flagged candidates; join that moves only the records whose person lives in another partition.

## Measurements I need

All of these are printed by a normal `XL_TIMING=1` run; they were not pasted into the log.

1. The `[v3b] ms`, `[v3b] setup ms … exit at`, `[v3b] pa Mcycles`, `[v3b] prof`, `[v3b] st`, `[v3b] rows` and `[v3b] links` lines for `k1s` at 20M and 60M.
2. Parse time with `XL_V3SKIP=1` at 20M (scanner alone).
3. `--features tl` busy and idle per stage, if that feature still builds.
4. `perf annotate` hot lines inside `sess_thread`, or failing that the `ss.gather / ss.group / ss.clicks / ss.person` split from the `prof` line.

## Two safety checks worth one run each

1. **The fallback at 60M.** `v3b` calls `fallback()` on several conditions (bad `ingest_ts`, a long ID whose high half is zero, a window over 2³¹ ms, a fault signal). Confirm once that a forced fallback on the new 20M set produces the reference output, and record its time and peak memory at 60M. It must stay under 3 minutes and 24 GB, since a timed-out run scores nothing.
2. **Campaign index width: checked by reading, no action needed.** `ckrec_put` (line 513) packs the campaign index into 14 bits and `CBOUND` is 2¹⁴. Setup calls `fallback("campaigns.csv is not canonical")` for any campaign number at or above `CBOUND`, a repeated number, or more than 250 channels (`src/setup.rs:589–610`), so an out-of-range campaign is slow, not wrong.

## Exactness review of the new sink and lanes (by reading, `fcd9075`)

I traced every record form from `ev_batch` / `ev_wide` through `w_unit`, `cand_key` and `emit_rec` to `urec_put`, and the person lookup in `sess_thread`. I found no case where two spellings of one ID get different keys, where a field's bits overlap another's, or where a duplicate could escape the candidate table:

- Compact form: remainder of the event ID in bits 32–63, ingest seconds in 11–31 (guarded by `is < 2²¹`), user ID split 11 + 31 bits (guarded by `uid < 2⁴²`), time in 33–63 of the second word (guarded by the 2³¹ ms window check), invalid flag in bit 1.
- All three wide forms and the compact form produce the same dedup key for a short event ID (`e_rem`), and long and interned IDs cannot collide with it (`hi != 0`, or bit 63 set).
- Ingest times are compared on one base (`g.ing_base`) in every form.
- The 30-minute bucket method in `person` is exact: two events in one bucket are under 30 minutes apart, non-adjacent occupied buckets are always more than 30 minutes apart, and adjacent ones are split on `min(next) − max(prev) > GAP`.
- A click is matched only to a person who has events, and events are grouped before clicks, so the "skip" marker cached for an ID cannot hide a later event.

This is a reading, not a proof. The gate (fixtures, both dev answers, 20M and 60M against the independent old implementation) is the stronger evidence.

## What the generator guarantees, and what it only appears to

I read the new generator source (`generator.zip` in the repo root).

**Stated in `rules.md` and safe to rely on:** `ingest_ts` is always valid F1; one in eight distinct IDs has a 32-character hex suffix and the rest are short lowercase base 36; the generator never writes `\r`; file counts and Zipf sizes; rows shuffled across files.

**True in the generator source but not stated in the rules:**

- A short ID is base 36 of a value below 2⁴⁰, uniform over that range (a Feistel bijection on 40 bits), so at most 8 characters. `v3b` uses this for speed only: it partitions short event IDs by their bits 28 to 39 (`e_part`, line 348). If the hidden generator differed, short values at or above 2⁴² would take the wide lane and non-uniform values would skew partition sizes. Output stays exact either way.
- Upper-cased IDs appear on events and clicks at about 1 in 200,000 rows and go through the interned "odd" path. Fine.
- The window is 14 days. `v3b` falls back above 2³¹ ms (24.8 days), so a longer window is slow, not wrong.

**Exploitable, but I recommend against it:** both halves of a long ID are an invertible mix of the same 40-bit value, so a long ID could be reduced to 40 bits with a per-ID check and a fallback, which would let long IDs use the compact lane. It would be output-safe. But the rules now say IDs "must" be treated as opaque strings "rather than … invertible generator numbers", and the organisers changed the format specifically to stop this. Whether to use it is the user's decision, not a technical one; I have not ranked it.

## Public entries and literature

- **Other entries:** a web search for this task's table names and rules found no public entry or write-up (2026-11-21). The only outside implementation I know of is the upstream repository this code is taken from.
- **Partitioned or global table for grouping:** "Global Hash Tables Strike Back!" (arXiv 2505.04153, 2025) reports that a concurrent global hash table built for aggregation can match partitioned aggregation. I do not recommend acting on it here: the dedup, join and session stages all need the same person's records together, which partitioning gives for free, and experiment 28 shows the partition counts are already on a flat optimum. The same literature (Bandle et al., "To Partition, or Not to Partition", SIGMOD 2021) says partitioning gains fall off quickly away from the tuned fan-out, which matches experiment 28.

## Log of what was tried from these notes

| Idea | Experiment | Result | My estimate |
|---|---|---|---|
| Release each input chunk after parsing | 29 (found independently by the coding agent) | Kept: −14 ms at 20M, −37 ms at 60M | −15 ms, −45 ms |
| Recycle event-lane chunks | 30 (done upstream, `7c69e3e`) | Part of a −40 ms step with long IDs in the event loop; not separable | −14 ms |
| Largest partitions first | 31 | Not built: thread-ms ÷ 8 equals the stage wall, so no idle tail at 20M | −5 ms average. Wrong at 20M; untested at 60M |
| `XL_PB0` sweep | 31 | Flat 256 to 4096; 0 and 262144 are 15 ms worse | unknown |
| `SW` 8 | 31 | Slower (632 to 638 median); 12 does not build | unknown sign |
