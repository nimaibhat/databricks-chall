# Findings (2026-11-21)

What we learned while trying to beat Tark305 at 0.63 s, and what is actually worth submitting.

## Submit this, not `saran/work`

**Commit `0a06317` on `main`.** It is the newest code commit. Later commits on `main` are notes only.

Do not submit `saran/work`. On the same x86 runner, `saran/work` at `6ff3e77` took 2.459 s (20M) and 7.067 s (60M). `main` at `0a06317` took 2.257 s and 6.342 s. See `OVERNIGHT_LOG.md` experiment 10.

## What the board scores mean

Our submissions on this branch:

| Commit | Board |
|---|---|
| `35753a0` | 1.61 s |
| `6c300bd` | 1.29 s |
| `40f157f` | 1.32 s |

The 1.32 s is the same code as 1.29 s plus a bitmap dedup prefilter. The difference is scorer noise, not a real regression. Keep `6c300bd` only if something forces a pick from `saran/work`. Prefer `0a06317` on `main`.

Laptop wall time is a bad predictor. This laptop (i7-13700H, 8 threads, 20M events) ran `6c300bd` in about 0.78 s and the board returned 1.29 s. A GitHub Actions x86 runner, timing the whole `docker run` the way the scorer does, is the measurement that matches the board. On that runner Vibhor's build (board 0.82–0.96 s) takes about 1.59 s, so the board is roughly 0.52–0.6× the runner time. Under 0.65 s on the board means about 1.2 s on that runner. `main` is at 2.26 s. Cheryl's public build, the fastest measured next to ours, is at 1.38 s / 3.82 s (20M / 60M).

## What is already on `main` and measured

Each row is a same-run before/after on x86. Full tables are in `OVERNIGHT_LOG.md`.

| Change | 20M | 60M |
|---|---|---|
| Product lookup only for purchases | −2.3% | −2.4% |
| Line-at-a-time scanner plus person resolved during parse | −2.5% to −4.8% | −3.2% to −6.3% |
| SIMD id packing | −3.0% | −3.0% |
| Write-combining non-temporal scatters | −2.4% | −2.1% |
| Date table for timestamps | −3.6% | −3.5% |
| Sessions phase: 8-byte keys, interval assists, largest partition first | −2.1% | −2.1% |

Together, experiments 2–5 were −14% against the parent on an Intel runner. `pread` instead of mmap was about −1% and was not switched on. Parse is still most of the time: on a 4-vCPU run, parse 0.67 s, sessions 0.21 s, links 0.08 s, dedup 0.06 s, output 0.04 s, and about 0.5 s of the docker wall is container start and stop.

## What `saran/work` tried

Kept, and verified here (tests, 36 fixtures, dev answer, 5M and 20M reference outputs):

- Event person lookup folded into dedup, with prefetch. Dropped a separate pass. Local 20M median about 1.06 s to 0.84 s. This is `e6301e3`, inside the 1.29 s submission.
- Event records packed to 28 bytes. Small. Inside `6c300bd` (1.29 s).
- Bitmap so only possible duplicate event ids enter the hash table. About 30 ms locally. Board 1.32 s, i.e. no gain. `40f157f`.
- Sessions phase from the agent work: 8-byte packed events, 30-minute bins for people with more than 16 events, insertion sort for the rest. Single-thread phase 4 on 5M events 189–207 ms down to 145–169 ms. At 8 threads on 20M, phase 4 about 130 ms down to 100 ms and the whole run about 780 ms down to 740 ms. Commit `f042e9f` on `saran/work`. It is a real local win on the old tree, and it is still slower than current `main`, which already rewrote this phase.

Tried and not kept:

- One-shuffle (links first, each event written once into its person partition, dedup only marks losers). Correct, but about 100 ms slower. The person lookup misses cache. Left behind `ONESHUF=1`, default off.
- Fewer partitions, fewer link threads, largest-partition-first on the old phase 4, and the branch-free timestamp parser. No win on this laptop.
- A huge-page copy of the links table. No win.
- Write-combining and `pread` as defaults (`6ff3e77`). Correct, but the laptop was on battery and too noisy to measure, and the x86 runner measured this commit slower than `main`. `main` already has its own write-combining, measured at −2.4%.

## The four agents

They were stopped by a model usage limit before they finished. Two left commits; the rest is uncommitted and was not merged.

- Phase 4 (`e3eac23`, now `f042e9f` on `saran/work`): the sessions win above. Merged onto `saran/work` only.
- Smaller event records (`7f9f63d`): dedup about 274 ms to 253 ms on a loaded single thread, plus less system time. Not merged. It conflicts with the sessions rewrite and the gain is small.
- Parse and links agents: unfinished. Nothing taken.

## What would have to change to reach 0.63 s

`main` is about 1.6× Cheryl's runner time (2.26 s vs 1.38 s) and about 1.9× the 1.2 s runner time that maps to a 0.65 s board score. The remaining gap is mostly parse instructions per line, then the sessions phase, then links. Dedup is already small (0.06 s). Container startup is about 0.5 s of runner wall and is not our code.

Next measured steps, in the order `OVERNIGHT_LOG.md` already queued: links without a separate naming pass, a `pread` path that never maps the input, and cutting instructions on the per-line parse. Rank new ideas against `RESEARCH_NOTES.md` and only keep a change when a same-run x86 docker measurement beats its parent on both 20M and 60M.
