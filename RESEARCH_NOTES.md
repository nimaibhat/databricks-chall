# Research notes: opaque-ID path (`src/v3b.rs`)

Written for the coding agent. These notes cover the rules as changed on 2026-11-21 (opaque variable-width IDs, final on about 60M events). Everything written for the old ID format was removed from this file; it is in git history up to `0f4ee43` and none of it is evidence for the new path.

Read against `d9788a0` (upstream `c7ebaf9` plus the input release and the two constants) and `OVERNIGHT_LOG.md` up to experiment 34. Line numbers in the current sections are for `src/v3b.rs` at `d9788a0`. Sections under "Earlier analysis" carry older line numbers and are closed.

**Numbers:** "log" means measured by the coding agent and quoted from `OVERNIGHT_LOG.md`. "estimate" means my arithmetic, not a measurement. I cannot run the binary.

**Which size to tune for:** the practice board runs 20M, the final runs about 60M. Where a setting helps one size and hurts the other, the 60M result is the one that decides the final score.

**State (log, experiment 45):** 20M 402.0 ms median, stages parse 170, dedup 70, clicks 27, links 0.2, p1 76, p2 42, write 7. 60M about 1.25 s, peak 2.5 GB. Code is `5f2fda8` (upstream `1cda738` plus the 768 KiB partition target); `v3b.rs` grew from 4,099 to 6,909 lines. **Line numbers and function names in the sections below this one are for `60f2fae` and have moved**; I am re-reading the changed stages now and will replace them.

## 1. Probable output defect at 1,024 user partitions: a linked alias can lose its link (by reading, `5f2fda8`; not run)

**This outranks every speed item. It is rare (my estimate: under 1 in 10,000 datasets at 20M), it does not apply at 60M with the current settings, and the fix is one line. I found it by reading and have not seen it happen; the test below settles it in one run.**

- **Where:** `p1_thread`, the loop that enters each moved ID of the partition into the user table (line 4927 on; the call is `ut.entry(mix42(e.0.lo) & rmask)` at line 4929), and `UTab::entry` (line 4395).
- **What goes wrong.** `UTab::entry` reads the home slot and returns it if `slot >> 32 == rem`. An empty slot is `u64::MAX`, whose top half is `0xFFFF_FFFF`. When there are 1,024 user partitions (`b2 == 10`, so `rb == 32`) a remainder can be exactly `0xFFFF_FFFF`, and then an **empty** slot looks like a match. `entry` returns `u64::MAX`, the caller takes the low half as the ID (`0xFFFF_FFFF`), the test `id == lkix.len()` is false, and the link is skipped without any error.
- **Upstream knows about this value:** five lines later (line 4939) `if rb == 32 { ut.slow(0xFFFF_FFFF, true) }` inserts that remainder so that the event and click loops never see an empty slot match. But it runs **after** the link loop, and it inserts the ID as an ordinary, unlinked user. So an alias with that remainder is treated as its own person: its events produce their own `user_daily` rows under the alias ID instead of being merged into the canonical user, and its sessions are credited separately in `campaign_daily`.
- **When it applies.** Only when `b2 == 10`. `b2` is 11 once the events file is over about 3.4 GB (about 47M events), so **60M is not exposed**. 20M is exposed **because of our 768 KiB partition target**: with upstream's 384 KiB the 20M set gets 2,048 partitions and the remainder has 31 bits. Any input under about 47M events (23M with upstream's default), including every small fixture, has 1,024 partitions.
- **How likely.** It needs a moved ID whose mixed value has all 32 low bits set: one chance in 2^32 per moved ID. With a few hundred thousand moved IDs at 20M that is roughly 1 in 10,000 to 1 in 20,000 per dataset (estimate; I do not have the moved-ID count, it is `moved=` on the `[v3b] links:` line). The gate cannot find it by chance.
- **Test (one small fixture).** The short user ID with value `2599643504811` is `x69cme23` in base 36, so the text ID is `ux69cme23`; it mixes to partition 1 with remainder `0xFFFF_FFFF` (I checked the arithmetic against `MIXC`). Make a fixture where:
  - `ux69cme23` has a few valid events;
  - an identity link joins `ux69cme23` to an ID that sorts before it, for example `ua`, which also has events;
  - nothing else is needed.
  The reference puts all rows under `ua`. If I am right the current build writes separate rows for `ux69cme23`. If the outputs are equal, I have misread something and this section should be deleted. Other IDs that hit the same case: `nsv2d0a3`, `efgs3mi3`, `522hu8q3` (partitions 2, 3, 4).
- **Fix.** In the link loop, do not use the fast lookup for that one remainder:
  `let rem = mix42(e.0.lo) & rmask; if rem == 0xFFFF_FFFF { ut.slow(rem, true) } else { ut.entry(rem) }`
  `slow` tests for an empty slot before comparing, so it inserts properly, and the later line 4939 then finds the existing entry. No other caller of `entry` exists. The event and click loops are safe because the value is always in the table by the time they run.
- **Alternative without touching upstream's code:** go back to upstream's 384 KiB target, which costs the 9 ms the 768 setting gains at 20M and still leaves inputs under about 23M events exposed. The one-line fix is better.
- **Worth telling upstream's author**, since the defect is in their code and affects their entry for inputs under about 23M events.

**Links retry (my item of 00:22): overtaken.** Upstream raised `LkT::PROBES` from 64 to 2048 in this version (line 2668 at `5f2fda8`), which is the fix I proposed. Whether a retry was the cause of the rare 50 ms wait was never checked against the `slots=` line, so it stays a hypothesis; what can be checked is whether the wait still occurs: count `links` laps above 5 ms over 40 or more runs at 60M on the new build.

**Experiment 42 (readings, nothing changed):** 8 threads is the right count on 8 CPUs (7, 10, 12 and 16 are all slower). With transparent huge pages set to `never` the 20M run takes about 630 ms instead of 478, so about 150 ms depends on the scorer honouring `madvise`; `always` is the same as `madvise`. Nothing in the notes can change that setting; if the scorer turned out to be `never`, the fresh-memory items (experiment 41's packing, fewer chunks touched) would be worth several times what they measured, and that is the one case in which to put them back.

## Top five untried (after experiment 39)

Upstream moved four commits and overtook most of my previous table:

- **Chunk recycling into later lanes** (my item 4): done upstream. Free chunks left over after dedup are handed to the `p1` block lane (`run`, the `XL_PMH` block near line 3973, on by default).
- **Presized row columns** (half of my item 3): done upstream ("presized columns").
- **Assist and credit loops:** replaced by vector passes (`credit16`, `credit_sorted`), so experiment 36's changes are gone with the code they touched.
- **Link union-find:** now a flat table on its own thread (`links_start`), joined after dedup. At 20M the join waits 0.2 ms, so it is off the critical path there.

**Experiment 40 answered three of my four items** (nothing changed in the code):

- **Fallback at 60M: safe.** 10.2 s, 11.8 GB peak, output equal to the reference. Closed.
- **Fresh memory:** 670 huge-page faults per 20M run (1.34 GB, was 1.6 GB); zeroing is 9.5% of cycles (was 10.7%). My count of what must be written is about 1.1 GB, so about 0.25 GB of overhead is left.
- **Links wait at 60M:** 0.2 to 0.3 ms in 38 of 40 runs, 23 and 49 ms in the other two.

**Experiment 41 built the staging-array packing** and did not keep it: 106 MB less zeroed, 2 to 4 ms at 20M, which is what I estimated but is inside run-to-run noise and is one more patch to carry across upstream changes. It is a safe small gain to put back once upstream stops moving.

## Previous table (after experiment 44; overtaken by upstream `1cda738`)

Experiment 44 closed the staging-array packing for good (1 to 2 ms at both sizes over 31 and 11 rounds) and corrected experiment 43: the links wait does happen with shipped defaults, about once in 40 runs at 60M, about 50 ms when it does. It asked whether I see a cheap cause. I see one candidate, and it can be confirmed from a line the binary already prints.

| # | Idea | Where | Expected | Row risk |
|---|---|---|---|---|
| 1 | The slow runs may be the link table being built twice. Check, then raise the probe limit | `LkT::PROBES` (line 1746), the retry loop in `build_links` (line 1825) | Nothing in a typical run. If confirmed, removes the roughly 50 ms wait in about 1 run in 40 at 60M. (hypothesis from reading; not measured) | None |

### 1. A retry in `build_links` would look exactly like this

- **What the code does.** `links_fast` puts short IDs in a table with `s` slots per partition, where `s = 2 × pairs / np` (line 1820). `LkT::slot` gives up after `min(64, s)` probes and returns 0; `links_fast` then returns `false`, and `build_links` doubles `s` and **starts the whole union-find again** (line 1825). Everything done so far is thrown away, so a retry roughly doubles the links thread's time.
- **Why it would be rare and random.** Each link pair brings up to two IDs, so a partition's region is between half full and full depending on how many IDs repeat across pairs. Whether some ID needs more than 64 probes depends on the order IDs are inserted, and that order depends on which thread parsed which link chunk. Same input, different order, occasionally one long probe run. That fits "1 run in 40, same data, same settings" better than scheduling does: a ninth thread on 8 CPUs loses a little time on every run, not 50 ms on one.
- **How to confirm, no new code.** The `[v3b] links:` timing line prints `slots=` and `uf=` (line 2017). In a slow run, `slots` will be twice its usual value and `uf` about twice as long (the timers add up across retries). If `slots` is the usual value in a slow run, this hypothesis is wrong and the cause is scheduling. `pairs=` and `ids=` on the same line give the fill: `ids / (np × slots)`.
- **The fix if confirmed.** Raise `PROBES` from 64 to something that only trips when a region is really full, for example 1024. The limit is only a give-up threshold; a long probe run costs a few hundred nanoseconds once, against tens of milliseconds for a retry. Alternatively start with a larger `s` (the `XL_LKS` knob already overrides it, so `XL_LKS` at 1.5 and 2 times the printed value can be tried without a rebuild), but that makes the table bigger for every run, so the probe limit is the better fix.
- **Risk to rows:** none. The probe limit decides only when to give up and retry; which slot an ID ends up in does not affect the result, and `XL_LKCHK=1` compares the link lists and bitmap against the plain method.
- **How to test.** Count slow runs over 40 or more at 60M before and after, and confirm `slots=` never doubles after. The typical run should not move.

**Where the fresh memory goes now (log, experiment 41 census, 20M):** event lanes 459 MB, click lanes 130 MB, pass-one blocks 145 MB, with 185 MB of released chunks idle on the free list at that point. The event and click lanes match my count of what must be written (410 + 50 and 110 MB), so there is no slack there. The idle 185 MB cannot serve the click lanes, which are written during parse before anything is released. I see no further memory item worth more than the 2 to 4 ms that experiment 41 just declined.

**Where the rest of the time is (log, experiment 40, 20M thread-ms):** dedup table 386, p1 short lane 448, p1 group 193, p1 gather 144, p2 walk 398. Parse is 213 of 468 ms wall and its hottest instructions are the loads of input bytes. I have now read every stage of the current version (parse sink, event lanes, dedup, links, p1, p2, credit) and have no further proposal I would put a number on. The parse sink is already a vector classify step followed by write-combined 64-byte lines with non-temporal stores; what remains in parse is the scanner in `k_free.rs` reading input bytes, which upstream is changing every cycle.

### Exactness review of the parse sink and event lanes (by reading, `60f2fae`)

No defect found. Points checked:

- **Bit packing of the 16-byte event record** (`ev_batch`, line 1343 on): remainder of the event ID, ingest second (21 bits) and the top 11 bits of the user ID in one word; relative time (31 bits), the low 31 bits of the user ID and the invalid flag in the other. It is exact only if the remainder fits 32 bits and the relative time fits 31. Both are enforced in `run`: `b1` is at least 10 (line 3813) and a window over 2^31 ms falls back (lines 3807 to 3815). `dedup_thread` unpacks with the same shifts.
- **Which events take the compact lane:** short event ID, ingest time a whole second inside the 21-bit range, and either invalid or a short user ID and not a purchase. Everything else goes to the 32-byte or 64-byte lane with full keys. Long IDs with a zero high half fall back instead of colliding with short ones.
- **Duplicate bitmap index** is computed the same way for the same event ID in all three lanes (`(a >> 35) & BMM`, the same expression on the 32-byte record, and `x_index` on the 64-byte one), so "seen once" is reliable; "seen twice" only nominates candidates, which are then compared on full keys.
- **Deferred line flush:** a full line may be flushed either by the next push to that partition or by `drain`; both check the fill count first, so a line cannot be written twice or lost. The queue has room for a whole batch past the drain threshold (`EQCAP = EDRAIN + NB + 8`).
- **Skewed IDs** (not from this generator) would unbalance the partitions because event partitioning uses raw ID bits, but that costs time only, not rows.

### Exactness review of the new credit code (by reading, `60f2fae`)

I traced `credit16`, `credit_sorted`, `tie_camp` and the purchase and row code in `finish_person`. I found no defect. Points checked:

- **Window bounds.** A click counts for a session when its time is in `[start − 7 days, start]`, both ends inclusive, in all three paths (16 or fewer clicks, 17 to 64, more than 64). `cbase` is `ws − 7 days` and every session starts at or after `ws`, so `start − cbase` is never negative and the relative times fit 31 bits.
- **Last click wins, ties by click ID.** The vector paths take the maximum of `time << 32 | campaign` inside the window, then test whether any other click has the same time and a different campaign; only then is `tie_camp` called, which compares full click IDs (short or from the side array). Padding lanes are all-ones and can neither enter the window nor trip the tie test, and the tie test is guarded by "window not empty".
- **Assists, 16-click path.** One entry per session per distinct campaign with a click in the window: the conflict mask picks the last click of each campaign as its representative and tests whether any click of that campaign is in the window.
- **Assists, sorted path.** Each click covers the sessions starting in `[time, time + 7 days]`; per campaign the covered range only moves forward (clicks are in time order), so the stamp-and-`lidx` scheme counts each session once per campaign. The per-day vector add relies on a person's sessions for one day being contiguous, which holds because sessions are in time order.
- **Buffer sizes.** `Walk.asst` has room for a full 16-lane store per session beyond the flush threshold, so the unchecked compress-store cannot run past its capacity.
- **Sort keys above 64 clicks** pack time, campaign (14 bits, guaranteed by the campaign bound check in `setup.rs`) and index (19 bits) without overlap; above 2^19 clicks a tuple sort is used.

This is a reading, not a proof. The gate is the stronger evidence.

### Closed

- **Sort-and-walk for small people** (experiment 38): bin fill is 43 thread-ms; nothing to win. My estimate of −10 ms was wrong.
- **Occupancy mask:** done upstream in a better form (a 64-byte vector test per occupancy block in `finish_person`).
- **Scratch vector presizing in `B3`:** not tested. `es` is reserved once per partition; the rest grow by doubling but are reused across partitions, so the loss is one-off per thread. Left out until item 1 says memory still matters.
- **The click part of `finish_person`:** rewritten upstream as vector passes; I have nothing to add.

### Done: experiment 32's changes (experiment 36)

The log says they "do not apply to the rewritten back half and are dropped for now". The code they touched is still there in the same shape: the stamp test in the brute-force assist loop (taken 77% of the time, so worth making branch-free) and the `rows × 32` category table that is cleared and scanned even for a person with one purchase. Both were gate-clean and moved the stage the same way at both sizes.

### Exactness review of the new back half (by reading, `d9788a0`)

I traced `urec_put` and `ckrec_put` through `p1_thread`, `group`, `lane`, `ev_block`, `ck_block`, `p2_thread` and `finish_person`. I found no defect. The points I checked specifically:

- **Short user keys** are stored as partition number plus a remainder of `42 − b2` bits in the top half of a word. That is exact only if the remainder fits 32 bits, which needs `b2 >= 10`; `run` enforces it with `.max(10)` (line 2804).
- **Lane streams carry no padding words** (`VB` counts valid words), so the purchase word that follows a purchase event is found by position, including across a chunk boundary (`pend`).
- **`group`** orders each lane fully by user key (radix on the top bits, then a sort inside any bucket holding more than one key), and `lane` merges events, purchases and clicks with the same tuple ordering.
- **"No links" shortcut:** a person is finished in `p1` only if the ID is neither a moved ID in this partition's link table nor flagged in the representative bitmap. The bitmap can give false positives, which only send a person the long way round; it cannot give false negatives because every representative sets its bit with the same index function the lookup uses.
- **Clicks without events** are dropped for unlinked IDs and skipped in `p2` for linked people with no events, which is right: no sessions, nothing to credit.
- **Click tie-break** still compares full click IDs, with long and interned IDs carried in a side array and re-indexed in `p2`.

This is a reading, not a proof. The gate is the stronger evidence.

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
| Counters in `person` | 32 | Done; gave the split used above | — |
| One-purchase shortcut for category totals | 32 | Kept with a branch-free assist loop: sessions 202.3 to 198.6 at 20M, 616.5 to 600.2 at 60M | −3 to −5 ms |
| Bitmask assists | 32 | Not built: visits would fall only from 15.2M to about 13M steps | up to −20 ms; too high |
| Experiment 32's two changes, re-applied to the new back half | 36 | Kept: −4 ms at 20M, −14 ms at 60M | −3 ms, −12 ms |
| Sort-and-walk for small people | 38 (diagnostic only) | Not built: bin fill is 43 of 572 thread-ms | −10 ms; wrong |
| Recycle chunks into later lanes; presize row columns | 39 (upstream) | Done upstream; peak at 60M 4.3 to 3.4 GB | −5 ms and part of −7 to −11 ms; not separable from the rest of the upstream change |
| Raise the link table probe limit | 45 (upstream) | Done upstream (64 to 2048); effect on the rare wait not yet counted | removes about 50 ms in 1 run in 40 at 60M (hypothesis) |
| Pack staging arrays, more rounds | 44 | Closed: 1 to 2 ms at both sizes | −3 to −4 ms; high |
| Link lists built in `p1_thread` | 43, corrected in 44 | Not built: wait is rare (1 run in 41 at 60M with defaults, about 50 ms) | about −2 ms on average at 60M; nothing to recover |
| Pack staging arrays into shared blocks | 41 | Not kept: 106 MB less zeroed, 2 to 4 ms, inside noise | −3 to −4 ms, about 100 MB |
| Fallback path at 60M | 40 | Safe: 10.2 s, 11.8 GB, output equal | safety check |
| Links wait at 60M | 40 | 0.2 ms typically, 23 and 49 ms in 2 of 40 runs | a reading |
| `KTab` layout | — | Dropped: upstream `c7ebaf9` removed the per-event lookup it targeted | −3 to −8 ms |
