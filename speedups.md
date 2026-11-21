# Fix the dead timestamp path first

The entry is already fast and structurally sound, so the remaining gains are constant factors in the per-line decode, and only one of them has been measured. A local profile of HEAD `3e5cebd` shows about **3.5 s wall at 20M events on 2 vCPUs**, with **parse at about 50% of wall and instruction-bound** (about 1,400 instructions per event line, 2.3% simulated L1 miss rate), so every instruction removed from the parse path converts almost directly into wall time. The single verified win is a bug: `civil_swar` in `src/parse.rs` always returns `None` because one mask misses a colon, so every F1 timestamp falls through to the scalar parser; fixing three constants cut parse CPU by about 9% (163 to 147 ns per event line single-threaded) with identical output. Everything else in this report is an estimate from instruction shares or phase timings, ranked by expected gain per unit of risk for the roughly 36 hours left before Sunday 2026-11-22 11:00 AM EST. The literature agrees with the profile: once the scan is SIMD, type conversion dominates, and the partitioned architecture already matches what the join and aggregation papers recommend. Most of the famous techniques (parallel union-find, Bloom filters, perfect hashing, io_uring, BOLT) target costs this program does not have and are listed under "do not spend time on". One research note suggests folding event dedup into the per-person sort scan; that is wrong under `rules.md` and must not be implemented. All timings come from a 2-vCPU sandbox without `perf`; scaling to 8 hyperthreads, cold-cache behaviour and 60M events are unmeasured, and at a few seconds of run time the fixed container start and exit cost and the page-cache state can outweigh anything the entrant controls.

Local evidence is cited as "audit §N", meaning section N of [repo_audit_and_profile.md](../research_notes/Shopping%20sessions%20pipeline%20speedups/repo_audit_and_profile.md). Rules are cited from [rules.md](../rules.md). "Measured" means measured in the sandbox on 2026-11-20. "Estimate" means derived from callgrind instruction shares or phase timings with no prototype behind it.

## The profile: half the wall time is parsing at 1,400 instructions per line

The binary builds cleanly, passes 36/36 fixtures, matches the dev answer exactly, and produces identical output at 1, 2 and 8 threads (audit §1, §4). At 20M events (21.6M event lines, 1.66 GB; 5.1M click lines) with a warm page cache and `NCPU=2`, the phases are: links 0.18 s, **parse 1.79 s**, dedup 0.35 s, resolve 0.30 s, sessions 0.55 s, output 0.39 s, total 3.56 s, peak RSS 3.5 GB (audit §1). That is parse about 50%, sessions about 16%, dedup 10%, resolve 8%, output 6 to 11%, links 5%. CPU cost is linear: 0.33 to 0.37 CPU-microseconds per event from 1M to 40M (audit §1). Run-to-run noise is about ±0.4 s on a 3.5 s run, so a single-run difference under about 10% means nothing.

Parse is the target because of where its time goes. Single-threaded over the event files (audit §2):

| Stage | ns per line | GB/s |
|---|---|---|
| `memchr` newline count | 7 | 10.9 |
| `scan_lines::<7>` only (SIMD split and quote check) | 35 | 2.2 |
| scalar split | 87 | 0.88 |
| full `parse_events` (decode plus 512-way scatter) | 163 to 193 | 0.40 to 0.47 |

The SIMD scan is 35 ns of a 163 to 193 ns line. The other 130 to 160 ns is field decoding and the scatter. Callgrind on the 1M dev set attributes 61.7% of all instructions to `parse_events` and 8.8% to `parse_clicks`, with a simulated L1 data miss rate of only 2.3% (audit §2). The specific hot spots inside the decode, as shares of all instructions (audit §3): the quoted-field loop in `scan_lines` 10.8%, the scalar timestamp path 8.1% plus 1.6% wasted in the failing SWAR pre-check, the scalar `pack` loop 7.5%, and a per-event libc `memcmp` call in `Catalog::get` about 1%. The built-in microbench puts the product lookup at 37 to 40 ns per line, far more than a 200 KB dense table should cost (audit §2).

Outside parse, three things stand out. Dedup is the only cache-bound phase: 4% of instructions but 24.6% of L1 read misses, and its per-partition map grows from 2 MB at 20M to 4 MB at 60M because `P1` is fixed at 512 (audit §3). Phase 4 shows 50 to 100 ms stalls in partitions whose sort and assist work takes 0 to 1 ms, best explained by reallocation of the unreserved `Vec<URow>` (audit §3). The output phase takes more than half of all kernel time (0.68 of 1.08 s sys) and copies each row three times through unreserved vectors (audit §2, §3).

Memory and the time cap are not risks. Anonymous memory grows about 76 MB per million events, which extrapolates to 5 to 5.5 GB at 60M against a 24 GB limit, and the projected 60M run is 20 to 22 CPU-seconds, or about 4 to 5 s wall on 8 hyperthreads if the page cache is warm (audit §1, §5). Both figures are extrapolations: 60M could not be generated in the sandbox.

## The one measured gain: three constants in `civil_swar`

In `civil_swar` (`src/parse.rs:709-746`) the third 8-byte word covers bytes 12..20 of the timestamp (`H:MM:SSZ`). It contains colons at byte 13 and byte 16, but the mask `SC` neutralizes only byte 16, so `digits8(c2)` fails on every input and the function always returns `None`. Output stays correct because `parse_ts_fast` falls back to the scalar parser, which is also why the existing `ts_fast_matches` test cannot see the bug (audit §4).

Apply exactly this (the diff below is the complete verified change; paths are relative to the repo root):

```diff
--- repo/src/parse.rs
+++ repo_copy/src/parse.rs
@@ -712,11 +712,11 @@
     let c = ld8(unsafe { p.add(12) });
     const SA: u64 = 0xFF00_00FF_0000_0000;
     const SB: u64 = 0x0000_FF00_00FF_0000;
-    const SC: u64 = 0x0000_00FF_0000_0000;
-    let sep = ((a & SA) ^ 0x2D00_002D_0000_0000) | ((b & SB) ^ 0x0000_3A00_0054_0000) | ((c & SC) ^ 0x0000_003A_0000_0000);
+    const SC: u64 = 0x0000_00FF_0000_FF00;
+    let sep = ((a & SA) ^ 0x2D00_002D_0000_0000) | ((b & SB) ^ 0x0000_3A00_0054_0000) | ((c & SC) ^ 0x0000_003A_0000_3A00);
     let a2 = (a & !SA) | 0x3000_0030_0000_0000;
     let b2 = (b & !SB) | 0x0000_3000_0030_0000;
-    let c2 = (c & 0x00FF_FFFF_FFFF_FFFF & !SC) | 0x3000_0030_0000_0000;
+    let c2 = (c & 0x00FF_FFFF_FFFF_FFFF & !SC) | 0x3000_0030_0000_3000;
     if sep != 0 || !digits8(a2) || !digits8(b2) || !digits8(c2) {
         return None;
     }
```

Add these two tests inside `mod swar_tests` in the same file. The first proves equality with the scalar parser on 3,000,000 random and corrupted strings; the second proves the fast path is actually taken, which is the check the old test lacked.

```rust
#[test]
fn civil_swar_fuzz() {
    let mut x: u64 = 88172645463325252;
    let mut rnd = |n: u64| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x % n };
    let mut hits = 0;
    for i in 0..3_000_000u32 {
        let mut s = format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", 1990 + rnd(60), rnd(14), rnd(33), rnd(26), rnd(62), rnd(62)).into_bytes();
        if i % 3 == 0 { let k = rnd(20) as usize; s[k] = b"0123456789:-TZtz +/.9\r,\""[rnd(24) as usize]; }
        let mut buf = vec![b'x'; 16]; buf.extend_from_slice(&s); buf.extend_from_slice(&[b','; 16]);
        let f = &buf[16..36];
        let r = parse_ts_fast(f, buf.as_ptr(), unsafe { buf.as_ptr().add(buf.len()) });
        assert_eq!(r, parse_ts(&s), "{:?}", String::from_utf8_lossy(&s));
        if f[19] == b'Z' { let c = civil_swar(f.as_ptr()); assert_eq!(c.map(|v| v * 1000), parse_ts(&s), "{:?}", String::from_utf8_lossy(&s)); hits += c.is_some() as u32; }
    }
    assert!(hits > 100_000);
}
#[test]
fn civil_swar_hits() {
    let b = b"2027-12-18T22:22:54Z,xxxxxxxxxxxx";
    assert_eq!(civil_swar(b.as_ptr()).map(|v| v * 1000), parse_ts(&b[..20]));
    assert!(civil_swar(b.as_ptr()).is_some());
}
```

**Measured result** on the 20M set, `NCPU=2`, three alternating before/after pairs: parse-phase user CPU 3.74 to 4.12 s before versus 3.36 to 3.63 s after; total wall 3.44 to 4.34 s before versus 3.32 to 3.40 s after; single-thread event parse 163 to 147 ns per line; microbench 173.8 to 158.3 ns per line. With the fix all 6 unit tests pass, fixtures are 36/36, the dev answer matches, and the 20M output is identical to the unfixed binary's (audit §4). Call it about 9% of parse CPU and roughly 4 to 6% of total wall. The wall figures overlap the noise band; the per-line and CPU figures are the trustworthy ones. This is the only number in this report that comes from an implemented change.

The fix fits the literature. Lemire's SIMD timestamp parser validates and converts a timestamp in about 65 instructions against about 700 for `strptime` ([Lemire, 2023](https://eklausmeier.neocities.org/lemire/blog/2023/07-01-parsing-time-stamps-faster-with-simd-instructions)), and each event line here carries two timestamps inside a 1,400-instruction budget. F1 is about 58% of `ts` values and 100% of `ingest_ts` values ([rules.md](../rules.md)), so the fast path now serves most timestamp parses.

## Ranked work list for the coding agent

Ranking is by expected gain divided by correctness risk, given 36 hours and 2 submissions per hour. A wrong table scores zero, so a 5% estimate that touches the line scanner ranks below a 2% estimate that touches a `with_capacity` call. Only row 1 is measured.

| # | Change | Where | Expected gain | Evidence | Risk |
|---|---|---|---|---|---|
| 1 | Fix `civil_swar` masks, add 2 tests | `src/parse.rs:715-719`, `mod swar_tests` | 9% parse CPU, about 4 to 6% wall | **Measured** | Very low: scalar fallback still guards every rejected input |
| 2 | Reserve `Acc.users`; reserve or stream Parquet buffers | `src/out.rs:25`, `src/main.rs:1013`, `src/pq.rs:120,129` | Removes 50 to 100 ms stalls in phase 4; trims output (6 to 11% of wall) | Estimate from STATS timers and phase times | Low |
| 3 | Fall back to `MADV_WILLNEED` when `MADV_POPULATE_READ` fails | `src/parse.rs:241-246` (`chunk_data`) | None on a 5.14+ kernel; avoids a sys-time blowup on an older one | Risk removal; mode 0 measured at sys 2.86 s versus about 1.0 s | Very low |
| 4 | Replace per-event `memcmp` prefix compare | `Catalog::get`, `src/keys.rs:372` | About 1% of instructions, possibly more of the 37 to 40 ns product lookup | Estimate from callgrind | Low |
| 5 | Process phase 4 partitions largest first | `process_partition` dispatch in `src/main.rs` (phase 4, lines 739-988) | Tail risk of 30 to 50 ms | Estimate from partition timings | Low |
| 6 | SWAR or SIMD `pack` | `src/keys.rs:52-67` | Up to 7.5% of instructions | Estimate from callgrind | Medium: order-preserving key semantics must be bit-exact |
| 7 | Cheaper quoted-line handling | `scan_lines`, `src/parse.rs:531-575` | Up to 10.8% of instructions | Estimate from callgrind | Medium to high: this routine decides which lines are malformed |
| 8 | Build links concurrently with parse | `src/main.rs:143-272` and `400-453` | Hides 0.15 to 0.45 s (measured phase length at 20M to 40M on 1 to 2 threads) | Estimate | Medium: restructures `Ctx` |
| 9 | SWAR path for F2 timestamps | `parse_ts_fast`, `src/parse.rs:753` | Small; F2 is 10% of `ts` | Estimate | Low to medium |
| 10 | Scale `P1` with input size | `P1_BITS`, `src/main.rs:303` | Unknown; targets dedup at 60M | Unmeasured | Medium; needs 8-thread hardware |
| 11 | AVX-512 scan kernel with runtime dispatch | `masks64`, `src/parse.rs:367` | At most a few ns of the 35 ns scan | Unmeasured | Medium; SIGILL and frequency risks |

Realized gains will be lower than instruction shares suggest. The audit's own estimate is that rows 1 to 4 together give 8 to 15% at 20M with almost no correctness exposure, and rows 6 and 7 together could give a similar amount again while touching the two most correctness-critical routines (audit §6).

### Tier A: do tonight, submit as one batch (rows 1 to 5)

Row 1 is specified above. Commit it alone first.

Row 2. `Acc::new` in `src/out.rs:25` creates `users: Vec::new()` and `src/main.rs:1013` pushes into it; the vector reaches 2.24M rows (72 MB) at 20M and 4.44M rows at 40M, so each doubling copies tens of MB and faults fresh pages (audit §3). Reserve it up front. The measured ratio is 2,242,393 user rows per 21,593,809 event lines, about 0.104 rows per line, so a total estimate of `event_bytes / 77 / 8` rows is safely above it; give each thread `2 * total / threads` because partition scheduling is dynamic. Untouched reserved pages cost nothing. The sizing formula is this report's suggestion, not something the audit tested; the alternative the audit names is storing `URow` in arena block lists. In `src/pq.rs`, `file` (line 120) and `page` (line 129) start as `Vec::new()`; compute the exact byte size before building and reserve it, or write header and pages straight to the file. As a separate, optional step, write `sessions` and `events` as INT32: `rules.md` accepts any integer width, and the audit estimates this cuts `user_daily` from about 43 to 35 bytes per row. Any change to the Parquet writer needs a read-back with pyarrow and duckdb before submission; the scorer's reader is unknown.

Row 3. `chunk_data` passes advice 22 (`MADV_POPULATE_READ`) and ignores the return value. That call exists only since Linux 5.14 ([Hildenbrand, man-page patch](https://lore.kernel.org/linux-mm/20210823120645.8223-1-david@redhat.com/T/)); on an older scorer kernel it returns `EINVAL` and the program silently runs as mode 0, which measured 4.58 s with sys 2.86 s against about 3.6 s for modes 1 to 3 (audit §2). Check the return value and, on error, call `madvise(..., MADV_WILLNEED)` on the same range and latch the mode to 2 for the rest of the run. Modes 1, 2 and 3 were within noise of each other, so this costs nothing measured.

Row 4. `Catalog::get` runs `s[..pl] == self.prefix[..]`, which compiles to a libc `memcmp` call per event for a 1-byte prefix (audit §3). Special-case short prefixes: for `pl == 1` compare one byte; for `pl <= 8` compare a masked 8-byte load. Keep the generic path for longer prefixes. Benchmark with the built-in `BENCH=2` microbench and watch the "+product lookup" line.

Row 5. Sort partition indices by descending event count before `par_for` hands them out in phase 4, so a whale partition (23 to 37 ms each at 40M) cannot be the last one running alone. Morsel-style dynamic scheduling with the largest units first is the standard answer to skew ([Leis et al., 2014](https://15721.courses.cs.cmu.edu/spring2016/papers/p743-leis.pdf)). Row order in the output does not matter under `rules.md`, so this cannot change the result.

### Tier B: Saturday, one change per submission (rows 6 to 9)

Row 6. `pack` in `src/keys.rs:52-67` does one table lookup and shift per byte for two roughly 9-byte IDs per event line; it is 7.5% of all instructions (audit §3). Replace the loop with a word-parallel version for IDs of at most 10 bytes: load the bytes ending at the field end (the same `floor`-guarded backward load `Catalog::get` already uses), classify bytes against the `-0-9A-Z_a-z` alphabet, and combine the 6-bit codes with multiplies. The result must equal the scalar `pack` bit for bit, including the length nibble and left alignment, because link naming depends on key order. Any byte outside the alphabet or any length over 10 must still go to `pack_long`. Required test: compare against the scalar version for every length 0 to 11, all 256 byte values at every position, plus several million random IDs. The construction is a design sketch from this report; no source in the notes benchmarks it.

Row 7. The generator quotes 20% of fields independently, so 1 - 0.8^7 = 79% of event lines and 59% of click lines contain a quote and take the per-field loop in `scan_lines` (`src/parse.rs:531-575`); the `qcount == 0` fast path serves only 21% of event lines (audit §3). The 1BRC write-ups show how much a badly predicted branch costs: restructuring one length branch took a solution from 2.4 s to 1.8 s as mispredictions per row fell from 0.657 to 0.084 ([Topolnik, 2024](https://questdb.com/blog/billion-row-challenge-step-by-step/)). The direction suggested by the CSV note is to validate quotes with bitmask arithmetic on the masks the scanner already has (every quote bit must sit next to a separator or line boundary on the correct side) and strip quotes without a data-dependent loop. The exact rule to preserve is the one the code implements now: a line passes only if every quote sits at both ends of a field of length at least 2 and the count of such quotes equals the line's quote count; `""` is an empty field; a lone `"` or a quote inside a field makes the line malformed (audit §4, [rules.md](../rules.md)). The existing fuzz test against the scalar splitter is the safety net; extend it with quote-dense inputs before touching the code. This is the largest estimated share and the riskiest edit in the list. If Saturday runs short, skip it.

Row 8. Phase 1 (links) runs before parse even though parse never reads the link maps; they are first used in phase 3b (`src/main.rs:446-453`). Starting link building on its own thread at the same time as event and click parsing, and joining before phase 3b, hides the links phase including its serial union-find (34 to 109 ms at 20M) behind the parse (audit §3, §6). On 8 busy hyperthreads the link work competes with parse threads, so the realized gain is smaller than the hidden wall time.

Row 9. Reuse the fixed `civil_swar` for the 19-byte civil prefix of F2 timestamps and parse the `+HH:MM` or `-HH:MM` offset separately, enforcing hours 00 to 23, minutes 00 to 59 and the colon. Extend the fuzz test to F2. Low value; do it only if the rest is done.

### Tier C: only with an 8-thread x86 machine to measure on (rows 10 and 11, plus experiments)

Row 10. At 60M a dedup partition holds about 127,000 records and its map is 4 MB, several times a Cascade Lake core's 1 MB L2 shared by two hyperthreads (audit §3). The join literature says partition fan-out should follow data size and that a wrong bit count can cost up to 2.5x in the partitioning-bound case ([Schuh et al., 2016](https://faculty.cc.gatech.edu/~jarulraj/courses/4420-s19/papers/19-hashjoins/schuh-sigmod2016.pdf)), and the partitioning note's arithmetic lands on 2^11 to 2^12 partitions for 60M 32-byte records. The counterweights are local: 512 write heads already equal a 32 KB L1D and cost 0.86 simulated L1 write misses per event line, and partial-block memory scales with threads times partitions times 32 KiB per list family (audit §3, §5). This is a real candidate for the final 60M run and it cannot be evaluated in the sandbox. Do not ship it unmeasured.

Row 11. The scan is 35 of roughly 165 ns per line, so even the 30 to 60% gains simdjson saw from an AVX-512 kernel on Tiger Lake ([Lemire, 2022](https://eklausmeier.neocities.org/lemire/blog/2022/05-25-parsing-json-faster-with-intel-avx-512)) would shave a few nanoseconds. On Skylake-SP class cores, which include Cascade Lake, any 512-bit instruction drops the core to a lower frequency license, and Lemire's rule is that light AVX-512 use must beat roughly a 15% downclock to pay ([Lemire, 2018](https://eklausmeier.neocities.org/lemire/blog/2018/09-07-avx-512-when-and-how-to-use-these-new-instructions)). If attempted at all: keep `target-cpu=x86-64-v3`, add one `#[target_feature]` kernel selected once with `is_x86_feature_detected!`, gate it on VBMI2 so Cascade Lake stays on AVX2, and go from compare to `u64` mask directly ([Rust 1.89 release notes](https://blog.rust-lang.org/2025/08/07/Rust-1.89.0/); [nietras, 2025](https://nietras.com/2025/05/09/sep-0-10-0)). Never build with `target-cpu=native` or `x86-64-v4`: a SIGILL on the scorer is a zero.

Three cheap experiments belong here too, because each is a one-line switch that only real hardware can judge. First, thread count per phase: on a 4-core, 8-thread box one 1BRC entry ran 1.36x faster at 8 threads than at 4 on a cache-friendly dataset and 3% slower on a cache-bound one ([1BRC discussion #138](https://github.com/gunnarmorling/1brc/discussions/138)), so parse should want 8 threads and dedup might want 4. Second, chunk size: the code uses 4 MB chunks (`src/main.rs:403-404`), while Instant Loading found the best chunk size between 0.25x and 1.0x of L3 divided by thread count ([Mühlbauer et al., 2013](https://www.vldb.org/pvldb/vol6/p1702-muehlbauer.pdf)). Third, two row cursors per parse thread, which two 1BRC entries credit for fuller pipelines ([1BRC discussion #710](https://github.com/gunnarmorling/1brc/discussions/710); [#138](https://github.com/gunnarmorling/1brc/discussions/138)); no source isolates its gain, and it competes with hyperthreading for the same execution resources.

## The regression gate and submission discipline

No change is submitted until it passes all of the following, in order. A change that fails any step is reverted, not patched under time pressure.

Build exactly as the Dockerfile does: `RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --release`. Without that flag the AVX2 `masks64` is compiled out and the scalar fallback runs, so both timing and test coverage are wrong (audit §1). Run `cargo test --release` with the same `RUSTFLAGS`. Then run `python3 scripts/run_fixtures.py fixtures -- target/release/sessions` and require "36/36 fixtures passed". Then run the binary on the dev set and require `python3 scripts/compare.py OUT dev-answer` to print MATCH (117,038 and 27,400 rows). Then the large-set check: generate at least one 20M set with `generate --seed 7 --events 20000000 --clicks 5000000 --output DIR`, produce a reference output once with the unmodified `3e5cebd` binary, and require `compare.py` MATCH between the candidate's output and that reference. `check.sh` already chains fixtures, dev and the big-set comparison when `S` points at a directory holding `dev/`, `ans/dev-answer`, `big/` and `ref_big/`. Run the candidate at `NCPU=1`, `2` and `8` and require all three outputs to match. After any change to `src/pq.rs` or `src/out.rs`, read both tables with pyarrow and duckdb and check that summed `sessions` and `revenue_cents` are equal across the two tables and that summed `assisted_sessions` is at least the non-organic sessions, which is the self-check in `rules.md`.

Two limits of this gate must be understood. There is no independent reference answer at 20M: the large-set check proves the output did not change, and correctness at scale rests on the fixtures, the dev answer and determinism across thread counts (audit §4). And dirt rates, window dates and file layout vary by seed ([rules.md](../rules.md)), so use a second seed for the reference when a change touches `scan_lines`, `pack` or timestamp parsing.

For benchmarking, do not compare single wall-clock runs. Use at least three alternating before/after pairs on the 20M set and compare parse-phase user CPU and the per-line figures from the built-in `BENCH=2` microbench; those are steadier than total wall. The audit's `BENCH=3` and `STATS=1` instrumentation lives only in the sandbox copy and is not in the repo.

For submissions: commit row 1 alone and submit it after the gate. Batch rows 2 to 5 into one submission. Submit each Tier B change separately so a scoreboard regression has one suspect. The sandbox and the dev machine cannot predict 8-thread behaviour, so the scoreboard is the only 8-thread benchmark available; with 2 submissions per hour it is also slow and probably noisy, so treat a single scoreboard delta under about 10% as inconclusive. The contest page says the final rebuilds your chosen commit, which is your best practice score unless you pick another, so a slower or rejected submission cannot hurt you, but every submitted commit must stay in the public repo. Stop making risky changes Saturday night and keep Sunday morning for a verified resubmission of the best known-good commit.

## What must not change: dedup stays global on `event_id`, before validation

One research note proposes folding event dedup into the per-person sort scan, on the assumption that duplicates of an event share a user and a timestamp and therefore land next to each other. **That is not safe under `rules.md` and must not be implemented.** Dedup is on `event_id` across the whole dataset and comes before validation: the kept copy is the one with the smallest `ingest_ts`, and if that copy is invalid the event is dropped and a later valid copy does not rescue it. The generator exercises both failure modes deliberately: in 2% of duplicated events only the earliest copy has a bad `ts`, and for about 1 in 64 duplicated events the later copies name a different user ([rules.md](../rules.md)). A per-person scan never sees the invalid earliest copy (it has no usable timestamp and no place in a sorted person stream) and never sees a copy filed under another person, so it would keep events that must be dropped and double-count events across people. The current design is the correct one: every parsed event line, valid or not, becomes an `ERec` partitioned by `event_id` hash, with `pc = INVALID` marking invalid copies, and validation is applied to the dedup winner in pass 2 (audit §3).

Three smaller suggestions from the notes fail for the same reason and are also excluded. Making rejected rows "not advance the output cursor" in the parser would discard invalid copies that dedup needs. Validating `ingest_ts` without converting it would break both the keep-smallest rule and the 24-hour late test. Skipping calendar math through a last-date cache gains little here because rows are shuffled across files over a 14-day window, so consecutive rows rarely share a date; that last point is this report's inference from the generator description, not a measurement.

Leave phase 4 semantics alone. The audit checked them against the rules and found them all matching: gap strictly over 1,800,000 ms starts a session, the credit tie-break is the byte-wise smallest `click_id` among clicks at the latest `ts`, the 7-day window is inclusive at both ends, the credited click's campaign also counts as an assist, and skipping click dedup is correct because click copies are identical (audit §3, §4). The union-find note describes an alternative click ordering and assist structures; they are not faster than what exists and every rewrite of that code is a chance to move a boundary.

## What the literature confirms, and what it says to skip

The published results line up with the profile on the main point. In the newest peer-reviewed CSV parser, counting records ran at 188 GB/s and converting all 16 TPC-H lineitem columns at 40 GB/s on the same machine, a 4.7x drop caused entirely by type conversion ([Ellmann and Neumann, 2026](https://www.vldb.org/pvldb/vol19/p3579-ellmann.pdf)). Sep finds rows at 7 to 25 GB/s per core and falls to 0.7 to 1.4 GB/s once a typed object per row is built ([Sep README](https://www.nuget.org/packages/Sep)). The repo shows the same shape: 2.2 GB/s for the scan, 0.40 to 0.47 GB/s with decoding. Fully typed single-thread parsing in that paper is about 2.1 GB/s on a recent EPYC, so the repo's per-thread rate sits a factor of four to five below the best published typed parser, on a different CPU and field mix; that gap is the room the Tier A and B decode fixes work in. No source benchmarks this field mix, so there is no published target number for it.

The architecture needs no rethink. Radix partitioning beats non-partitioned processing for narrow tuples and large inputs and loses its edge above 32-byte tuples ([Bandle et al., 2021](https://db.in.tum.de/~bandle/papers/bandle-partitionVsNonPartition.pdf); [Balkesen et al., 2013](https://www.comp.nus.edu.sg/~hebs/course/cs6284/papers/Balkesen-icde2013.pdf)); per-thread chunked partition output, which is what the arena block lists are, was about 20% faster than scattering into shared contiguous output ([Schuh et al., 2016](https://faculty.cc.gatech.edu/~jarulraj/courses/4420-s19/papers/19-hashjoins/schuh-sigmod2016.pdf)); writing parsed tuples straight into per-task partition buffers is the Instant Loading design ([Mühlbauer et al., 2013](https://www.vldb.org/pvldb/vol6/p1702-muehlbauer.pdf)); and partition, sort, then a forward merge is how DuckDB implements as-of joins ([Wesley, 2023](https://duckdb.org/2023/09/15/asof-joins-fuzzy-temporal-lookups.html)). The repo does all of these already. Huge pages are also already in use: the arena is `MADV_HUGEPAGE` and 1.24 of 1.27 GB anonymous memory was on huge pages at measurement time (audit §2).

### Do not spend time on

| Technique | Why it does not apply here | Source |
|---|---|---|
| Parallel or concurrent union-find, Afforest, ConnectIt sampling | The serial union-find is 34 to 109 ms at 20M. Published parallel wins are on graphs with tens of millions to billions of edges on 20 to 72 cores, and a sequential variant was among the fastest measured. | audit §3; [Dhulipala et al., 2021](https://arxiv.org/pdf/2008.03909); [Sutton et al., 2018](https://www.mosix.cs.huji.ac.il//pub/Parallel_Graph_Connectivity.pdf); [Alistarh et al., 2019](https://arxiv.org/abs/1911.06347) |
| Bloom, xor or binary fuse filters | For dedup, 96 to 98% of keys are new and must be stored anyway, and xor and fuse filters need the full key set up front. For the link lookup, 60.6% of event rows hit, the shards are already partition-local, and the break-even rule (filter cost below miss rate times saved work) fails. | audit §3; [Lang et al., 2019](https://ir.cwi.nl/pub/29441/29441.pdf); [Graf and Lemire, 2022](https://arxiv.org/abs/2201.01174) |
| Minimal perfect hashing (PtrHash), k-perfect static tables | Resolve is 8% of wall and already probes cache-local shards. PtrHash's best case is 8 to 12 ns per query at RAM latency, still needs key verification for unknown IDs, and adds build time. The k-PHF table only wins from about 30M keys. | audit §1, §3; [Groot Koerkamp, 2025](https://arxiv.org/abs/2502.15539); [Groot Koerkamp et al., 2026](https://arxiv.org/abs/2607.07257) |
| io_uring, O_DIRECT | Docker's default seccomp profile blocks the io_uring syscalls. O_DIRECT forces device reads when the cache is warm. Kernel time is 16% of CPU and more than half of that is output, not input. | [Docker seccomp docs](https://docs.docker.com/engine/security/seccomp/); [moby #47532](https://github.com/moby/moby/issues/47532); audit §2 |
| BOLT | Needs llvm-bolt and relocation-aware linking; the verified gains read were 2 to 5% on rustc's LLVM, a far larger binary than a 2,800-line hot loop. The BOLT paper itself was not read. | [Beránek, cargo-pgo, 2023](https://kobzol.github.io/rust/cargo/2023/07/28/rust-cargo-pgo.html) |
| PGO | Verified gains read were about 1% for rustc; a profile from a small dataset can mislead at 60M; doubles a fat-LTO build. Optional and last, behind a build ARG. | [Beránek, cargo-pgo, 2023](https://kobzol.github.io/rust/cargo/2023/07/28/rust-cargo-pgo.html); [rustc book](https://doc.rust-lang.org/rustc/profile-guided-optimization.html) |
| Switching the default to `pread` | One note recommends it; the local measurement disagrees. `IOMODE=4` ran 4.22 s against 3.55 to 3.74 s for the mmap modes (one run each, warm cache). Keep it as the low-RSS fallback. | audit §2 |
| Fork-and-exit trick, manual `munmap` avoidance | Exit teardown measured 17 to 24 ms at 20M and the code already calls `process::exit(0)`. THP teardown costs about 1.7 ms per GB. As PID 1 in a container the child would still be reaped on the clock (unverified). | audit §2; [LKML RFC, 2026](https://lkml.iu.edu/2607.2/06062.html); [1BRC #138](https://github.com/gunnarmorling/1brc/discussions/138) |
| Radix-sort rewrite of the per-person sort, vqsort or x86-simd-sort over FFI | Sort time is 0 to 1 ms in ordinary partitions and 14 to 29 ms in whale partitions at 40M; the phase 4 stalls are vector growth, not sorting. | audit §3; [Blacher et al., 2022](https://arxiv.org/abs/2205.05982) |
| Swiss table or hashbrown, AMAC, coroutine probing | A flat linear-probing table with a multiplicative hash at low load is the recommended design for this shape, and prefetch machinery loses when the table is already in cache. | [Richter et al., 2015](https://core.ac.uk/download/357589082.pdf); [Kocberber et al., 2015](https://core.ac.uk/download/43718028.pdf) |
| Quote-state machinery (carry-less multiply, speculative chunk starts) | In this dialect no value holds a newline, so every `\n` ends a record and any offset can be aligned by scanning to the next one. | [rules.md](../rules.md); [Mühlbauer et al., 2013](https://www.vldb.org/pvldb/vol6/p1702-muehlbauer.pdf) |
| Non-temporal write-combining retrofit onto arena blocks | Its measured wins are for scatter into large contiguous arrays; block lists consumed right after already have the TLB-friendly property. | [Müller et al., 2015](https://core.ac.uk/download/524940031.pdf); [Schuh et al., 2016](https://faculty.cc.gatech.edu/~jarulraj/courses/4420-s19/papers/19-hashjoins/schuh-sigmod2016.pdf) |
| Image slimming, allocator swaps, thread pinning, persistent thread pool | Warm `docker run` cost was flat at 554 to 568 ms across 3 to 155 MB images; mimalloc on a static binary is already in place; `strace` showed 19 `clone3` calls per phase and nothing else significant. | [Khan, 2026](https://arxiv.org/html/2602.15214v1); audit §2 |

## Caveats that bound every number above

Every local timing comes from a sandbox with **2 vCPUs** (Sapphire Rapids class, one thread per core), a 6.5 GB memory cgroup and **no `perf`**; hot spots are callgrind instruction counts and wall-clock deltas, not cycle-accurate samples, and callgrind's last-level cache was sized to the host's 260 MB L3, so its LL numbers are meaningless (audit, header and §2). **Scaling to 8 hyperthreads is unmeasured**: the `NCPU=4` and `NCPU=8` runs were oversubscribed and show only overhead. The 4 to 5 s projection for 60M assumes 4.5 to 5.5 effective cores and a warm cache and is an extrapolation from two points; the 40M run hit the sandbox memory limit and is usable for memory scaling only (audit §1, §5).

**Cold-cache behaviour is unmeasured and could dominate.** The first run after generating the data took 5.60 s with 3.88 s of sys time against about 3.5 s and 1.0 s warm (audit §1); a 1BRC author saw the same pattern, 5.0 s cold against 1.9 s warm ([van Kooten, 2024](https://www.dannyvankooten.com/blog/2024/1brc/)). Whether the scorer's `/data` is in the page cache at start is unknown. **Fixed container overhead is outside the entrant's control**: one preprint measured warm `docker run` at about 0.55 to 0.57 s regardless of image size ([Khan, 2026](https://arxiv.org/html/2602.15214v1)); it is a single-author, non-peer-reviewed study on Azure VMs, and the scorer's harness may differ. Against a program that runs in a few seconds, that fixed cost plus page-cache state can be as large as everything in the work list combined.

The Docker image build was not tested, and a run under the scorer's flags (`--read-only`, uid 1000, 64 MB `/tmp`) was not exercised; code reading says the binary writes only under `OUTPUT_DIR` (audit §1). The sandbox used rustc 1.97.0 while the Dockerfile pins `rust:1.91-bookworm`. The path in `check.sh` suggests development on macOS, where the AVX2 `masks64` path does not run at all; if so, the regression gate must be run on x86-64 (the Docker build under `linux/amd64`, or a cloud VM) before each submission. That is an inference from the script, not something the audit established. The scorer's Parquet reader is unknown; polars, pyarrow and duckdb accept the files (audit §4).

**Huge pages carry an RSS risk.** Under THP every touched 2 MiB region commits in full, so sparse touching inflates resident memory ([kernel THP documentation](https://www.kernel.org/doc/html/v6.12/admin-guide/mm/transhuge.html)). The current footprint is safe, but anything that multiplies partial blocks by threads times partitions (raising `P1` to 4,096 with 32 KiB blocks touches about 1 GB per list family at 8 threads) moves toward the 24 GB limit, and an OOM kill scores zero (audit §5). Under `defrag=madvise` a huge-page fault can also stall in compaction on a fragmented host; that is unmeasured.

On the literature side: none of the join, aggregation or sorting papers benchmark a 4-core, 8-thread machine, so their ratios and thresholds transfer and their absolute throughputs do not. Most PDF sources were read through an automated summary, so page-level figures should be spot-checked before being quoted elsewhere. The Polychroniou and Ross results come from a slide deck, and only the abstract of Patwary et al. was accessible. The following were named in the research brief or the notes but **not fetched, so no numbers are attached to them here**: Mison (Li et al., PVLDB 2017), ParPaRaw (Stehle and Jacobsen, PVLDB 2020), Ge et al. (SIGMOD 2019), Balkesen et al. "Sort vs. Hash Revisited" (PVLDB 2013), Chen et al. (ICDE 2004), Papagiannis et al. (ATC 2020), vmcache (SIGMOD 2023), Didona et al. (SYSTOR 2022), Axboe's io_uring paper, Ingens, HawkEye, Temeraire, the mimalloc technical report, BOLT (CGO 2019), Muła's SIMD integer-parsing articles, ska_sort, Khayyat et al. (IEJoin), and the theory papers on set union and window aggregation that the notes list as bibliographic only. A "Lemire and Keiser" paper on timestamp parsing in Software: Practice and Experience could not be confirmed to exist; the timestamp technique is cited from Lemire's 1 July 2023 blog post instead. SMT numbers are all from AMD or older Intel desktop parts; no Xeon-specific measurement for SIMD parsing was found.

## Conclusion

The research changed the question. It started as "which technique from the literature makes this faster" and ends as "which hundred instructions per line can be deleted without changing a byte of output". The program is past the point where algorithms matter: cost is linear, skew is absorbed, memory is a quarter of the cap, and the one phase the literature would redesign (dedup at 60M) is 10% of wall and unmeasurable without the scorer's hardware. The highest-value finding was a silent bug in an existing optimization, found by a test that asserts the fast path is taken instead of only asserting the answer is right. That pattern deserves a second look elsewhere in the decode: any fast path with a correct fallback can be dead without any test noticing.

The other shift is in what "faster" can mean here. At a few seconds per run, an unknown but material share of the scored time is container start, exit and page-cache state that the entrant does not control, the scoreboard is the only 8-thread benchmark available, and each look at it costs 30 minutes. That argues for taking the measured 4 to 6% and the low-risk Tier A batch tonight, spending Saturday on at most two of the medium-risk decode changes behind the full gate, and treating a correct submission in hand as worth more than any estimate in this report.

## References

Sources actually read during the research, grouped by topic. Local sources first.

Local

- Repo audit and profile (build, per-phase timings, callgrind, correctness review): [research_notes/Shopping sessions pipeline speedups/repo_audit_and_profile.md](../research_notes/Shopping%20sessions%20pipeline%20speedups/repo_audit_and_profile.md)
- Contest rules and generator description: [rules.md](../rules.md)
- Verified SWAR fix and tests: `/home/claude/work/swar_fix_and_tests.diff`

CSV and field parsing

- Langdale, Lemire. "Parsing Gigabytes of JSON per Second." VLDB Journal 28(6), 2019. https://arxiv.org/html/1902.08318v7
- Mühlbauer, Rödiger, Seilbeck, Reiser, Kemper, Neumann. "Instant Loading for Main Memory Databases." PVLDB 6(14), 2013. https://www.vldb.org/pvldb/vol6/p1702-muehlbauer.pdf
- Ellmann, Neumann. "One Pass to Parse Them All: Fused Parallel CSV Processing." PVLDB 19(11), 2026. https://www.vldb.org/pvldb/vol19/p3579-ellmann.pdf
- Nielsen (nietras). "Sep 0.10.0." Blog, 9 May 2025. https://nietras.com/2025/05/09/sep-0-10-0
- Sep README (benchmark tables). https://www.nuget.org/packages/Sep
- Topolnik. "The Billion Row Challenge (1BRC): Step-by-step from 71s to 1.7s." QuestDB blog, 2024. https://questdb.com/blog/billion-row-challenge-step-by-step/
- Topolnik. "1BRC: merykitty's Magic SWAR." QuestDB blog, 2024. https://questdb.com/blog/1brc-merykittys-magic-swar/
- Donisan. 1BRC discussion #710, 2024. https://github.com/gunnarmorling/1brc/discussions/710
- lehuyduc and others. 1BRC discussion #138, 2024. https://github.com/gunnarmorling/1brc/discussions/138
- lehuyduc. 1brc-simd repository. https://github.com/lehuyduc/1brc-simd
- Vanlightly. "1BRC on a Threadripper 9980X." Blog, 1 July 2026. https://jack-vanlightly.com/blog/2026/7/1/1brc-on-a-threadripper-9980x
- Morling. "1BRC: The Results Are In!" 2024. https://morling.dev/blog/1brc-results-are-in
- van Kooten. "One Billion Rows Challenge in C." 2024. https://www.dannyvankooten.com/blog/2024/1brc/
- Lemire. "Parsing time stamps faster with SIMD instructions." Blog, 1 July 2023. https://eklausmeier.neocities.org/lemire/blog/2023/07-01-parsing-time-stamps-faster-with-simd-instructions (also listed at https://isocpp.org/blog/2023/07/parsing-time-stamps-faster-with-simd-instructions-daniel-lemire)
- Lemire. "Parsing integers quickly with AVX-512." Blog, 22 September 2023. https://eklausmeier.neocities.org/lemire/blog/2023/09-22-parsing-integers-quickly-with-avx-512
- Lemire. "Faster bitset decoding using Intel AVX-512." Blog, 10 May 2022. https://eklausmeier.neocities.org/lemire/blog/2022/05-10-faster-bitset-decoding-using-intel-avx-512
- Lemire. "Parsing JSON faster with Intel AVX-512." Blog, 25 May 2022. https://eklausmeier.neocities.org/lemire/blog/2022/05-25-parsing-json-faster-with-intel-avx-512
- Lemire. "AVX-512: when and how to use these new instructions." Blog, 7 September 2018. https://eklausmeier.neocities.org/lemire/blog/2018/09-07-avx-512-when-and-how-to-use-these-new-instructions
- Downs. "Ice Lake AVX-512 Downclocking." Blog, 19 August 2020. https://travisdowns.github.io/blog/2020/08/19/icl-avx512-freq.html
- Neri, Schneider. "Euclidean affine functions and their application to calendar algorithms." Software: Practice and Experience 53(4), 2023 (abstract only). https://arxiv.org/abs/2102.06959
- Rust 1.89.0 release announcement. https://blog.rust-lang.org/2025/08/07/Rust-1.89.0/
- Rust `std_detect` x86 feature list. https://doc.rust-lang.org/stable/src/std_detect/detect/arch/x86.rs.html
- `rfc3339-fast` crate (located, not benchmarked). https://docs.rs/rfc3339-fast

Partitioning, hashing, sorting, aggregation

- Balkesen, Teubner, Alonso, Özsu. "Main-Memory Hash Joins on Multi-Core CPUs: Tuning to the Underlying Hardware." ICDE 2013. https://www.comp.nus.edu.sg/~hebs/course/cs6284/papers/Balkesen-icde2013.pdf
- Schuh, Chen, Dittrich. "An Experimental Comparison of Thirteen Relational Equi-Joins in Main Memory." SIGMOD 2016. https://faculty.cc.gatech.edu/~jarulraj/courses/4420-s19/papers/19-hashjoins/schuh-sigmod2016.pdf
- Polychroniou, Ross. "A Comprehensive Study of Main-Memory Partitioning and its Application to Large-Scale Comparison- and Radix-Sort." SIGMOD 2014 (slides only). https://scispace.com/pdf/a-comprehensive-study-of-main-memory-partitioning-and-its-4w6rslctmt.pdf
- Wassenberg, Sanders. "Faster Radix Sort via Virtual Memory and Write-Combining." 2010. https://arxiv.org/abs/1008.2849
- Müller, Sanders, Lacurie, Lehner, Färber. "Cache-Efficient Aggregation: Hashing Is Sorting." SIGMOD 2015. https://core.ac.uk/download/524940031.pdf
- Bandle, Giceva, Neumann. "To Partition, or Not to Partition, That is the Join Question in a Real System." SIGMOD 2021. https://db.in.tum.de/~bandle/papers/bandle-partitionVsNonPartition.pdf
- Leis, Boncz, Kemper, Neumann. "Morsel-Driven Parallelism: A NUMA-Aware Query Evaluation Framework for the Many-Core Age." SIGMOD 2014. https://15721.courses.cs.cmu.edu/spring2016/papers/p743-leis.pdf (also https://db.in.tum.de/~leis/papers/morsels.pdf)
- Richter, Alvarez, Dittrich. "A Seven-Dimensional Analysis of Hashing Methods and its Implications on Query Processing." PVLDB 9(3), 2015. https://core.ac.uk/download/357589082.pdf
- Kocberber, Falsafi, Grot. "Asynchronous Memory Access Chaining." PVLDB 9(4), 2015. https://core.ac.uk/download/43718028.pdf
- Psaropoulos, Legler, May, Ailamaki. "Interleaving with Coroutines: A Practical Approach for Robust Index Joins." PVLDB 11(2), 2017. https://core.ac.uk/download/148033241.pdf
- Groot Koerkamp. "PtrHash: Minimal Perfect Hashing at RAM Throughput." SEA 2025. https://arxiv.org/abs/2502.15539
- Groot Koerkamp, Hermann, Sanders, Walzer. "Non-Minimal k-Perfect Hashing: Tight Lower Bounds and an Application to Fast Static Hash Tables." ESA 2026. https://arxiv.org/abs/2607.07257
- Khuong, Morin. "Array Layouts for Comparison-Based Searching." ACM JEA 22, 2017. https://arxiv.org/abs/1509.05053
- Graf, Lemire. "Xor Filters: Faster and Smaller Than Bloom and Cuckoo Filters." ACM JEA 25, 2020. https://arxiv.org/abs/1912.08258
- Graf, Lemire. "Binary Fuse Filters: Fast and Smaller Than Xor Filters." ACM JEA 27, 2022. https://arxiv.org/abs/2201.01174
- Lang, Neumann, Kemper, Boncz. "Performance-Optimal Filtering: Bloom Overtakes Cuckoo at High Throughput." PVLDB 12(5), 2019. https://ir.cwi.nl/pub/29441/29441.pdf
- Axtmann, Witt, Ferizovic, Sanders. "Engineering In-place (Shared-memory) Sorting Algorithms." ACM TOPC 9(1), 2022. https://arxiv.org/abs/2009.13569
- Blacher, Giesen, Sanders, Wassenberg. "Vectorized and performance-portable Quicksort." 2022. https://arxiv.org/abs/2205.05982
- Bergdoll, Peters. ipnsort write-up, 2024. https://www.github.com/Voultapher/sort-research-rs/blob/main/writeup/ipnsort_introduction/text.md
- DuckDB. "Redesigning DuckDB's Sort, Again." 2025. https://duckdb.org/2025/09/24/sorting-again.html
- rdst README. https://github.com/nessex/rdst
- voracious_radix_sort README. https://github.com/lakwet/voracious_sort
- hashbrown README. https://docs.rs/crate/hashbrown/latest/source/README.md
- rapidhash README. https://docs.rs/crate/rapidhash/4.4.1/source/README.md

Union-find, sessionization, attribution

- Alistarh, Fedorov, Koval. "In Search of the Fastest Concurrent Union-Find Algorithm." OPODIS 2019. https://arxiv.org/abs/1911.06347 (full text https://ar5iv.labs.arxiv.org/html/1911.06347; https://drops.dagstuhl.de/entities/document/10.4230/LIPIcs.OPODIS.2019.15)
- Dhulipala, Hong, Shun. "ConnectIt: A Framework for Static and Incremental Parallel Graph Connectivity Algorithms." PVLDB 14(4), 2021. https://arxiv.org/pdf/2008.03909
- Sutton, Ben-Nun, Besta, Hoefler. "Optimizing Parallel Graph Connectivity Computation via Subgraph Sampling." IPDPS 2018. https://www.mosix.cs.huji.ac.il//pub/Parallel_Graph_Connectivity.pdf
- Patwary, Blair, Manne. "Experiments on Union-Find Algorithms for the Disjoint-Set Data Structure." SEA 2010 (abstract only). https://unpaywall.org/10.1007%2F978-3-642-13193-6_35
- Jayanti, Tarjan. "Concurrent Disjoint Set Union." Distributed Computing 34, 2021. https://arxiv.org/abs/2003.01203
- papaya BENCHMARKS.md. https://rust-digger.code-maven.com/src/papaya-0.1.4/BENCHMARKS.md
- Akidau et al. "The Dataflow Model." PVLDB 8(12), 2015. https://www.vldb.org/pvldb/vol8/p1792-Akidau.pdf
- Wesley. "DuckDB's AsOf Joins: Fuzzy Temporal Lookups." DuckDB blog, 15 September 2023. https://duckdb.org/2023/09/15/asof-joins-fuzzy-temporal-lookups.html
- Wesley. "Planning AsOf Joins." DuckDB blog, 19 February 2025. https://duckdb.org/2025/02/19/asof-plans.html
- ClickHouse JOIN documentation. https://clickhouse.com/docs/sql-reference/statements/select/join
- Wesley, Xu. "Incremental Computation of Common Windowed Holistic Aggregates." PVLDB 9(12), 2016. https://www.vldb.org/pvldb/vol9/p1221-wesley.pdf
- Tangwongsan, Hirzel, Schneider, Wu. "General Incremental Sliding-Window Aggregation." PVLDB 8(7), 2015. https://www.vldb.org/pvldb/vol8/p702-tangwongsan.pdf

I/O, memory, build, container

- Crotty, Leis, Pavlo. "Are You Sure You Want to Use MMAP in Your Database Management System?" CIDR 2022. https://www.pdl.cmu.edu/ftp/Database/p13-crotty.pdf (project page https://db.cs.cmu.edu/mmap-cidr2022/)
- Haas, Leis. "What Modern NVMe Storage Can Do, And How To Exploit It." PVLDB 16, 2023. https://www.vldb.org/pvldb/vol16/p2090-haas.pdf
- Hildenbrand. madvise(2) man-page patch for MADV_POPULATE_READ and MADV_POPULATE_WRITE. https://lore.kernel.org/linux-mm/20210823120645.8223-1-david@redhat.com/T/
- Docker documentation, seccomp security profiles. https://docs.docker.com/engine/security/seccomp/
- moby/moby issue #47532 (io_uring blocked by default seccomp profile). https://github.com/moby/moby/issues/47532
- LWN. "Stabilizing per-VMA locking." 2023. https://lwn.net/Articles/937943
- Linux kernel documentation, Transparent Hugepage Support (v6.12). https://www.kernel.org/doc/html/v6.12/admin-guide/mm/transhuge.html
- Jones. hugepagedemo. https://github.com/evanj/hugepagedemo
- Sharma. "[RFC PATCH 0/7] mm: defer address-space teardown of large exiting processes to a kthread." LKML, July 2026. https://lkml.iu.edu/2607.2/06062.html
- baarse. "Slow termination of JVM app with very large heap." https://baarse.substack.com/p/slow-termination-of-jvm-app-with
- mechanical-sympathy thread on page-fault cost, 2014. https://groups.google.com/g/mechanical-sympathy/c/1Fy9iDavk2c
- Babcock. "Default musl allocator considered harmful (to performance)." 2025. https://nickb.dev/blog/default-musl-allocator-considered-harmful-to-performance
- mimalloc environment options. https://microsoft.github.io/mimalloc/environment.html
- Beránek. "How to improve Rust compiler's CI in 2023." https://kobzol.github.io/rust/rustc/2023/07/30/optimizing-rust-ci-2023.html
- Beránek. "Optimizing Rust programs with PGO and BOLT using cargo-pgo." 2023. https://kobzol.github.io/rust/cargo/2023/07/28/rust-cargo-pgo.html
- rustc book, "Profile-guided Optimization." https://doc.rust-lang.org/rustc/profile-guided-optimization.html
- Khan. "Decomposing Docker Container Startup Performance: A Three-Tier Measurement Study on Heterogeneous Infrastructure." arXiv 2602.15214, 2026 (preprint, not peer reviewed). https://arxiv.org/html/2602.15214v1
