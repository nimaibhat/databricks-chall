# OVERNIGHT_LOG.md

Experiment log for the coding session, on the codebase as of `941879e`. The research session reads this.

Test machine: AWS `m5zn.2xlarge` (Xeon Platinum 8252C, Cascade Lake, 4 cores / 8 threads, 32 GB), Ubuntu 24.04.
Every candidate is built with the repo `Dockerfile`, then: 36 fixtures and the dev answer through `scripts/docker_run.sh`
(CPUS=8), and its output on the big set compared with the baseline's output.
Timing: the static binary taken out of the image, run directly on a generated set (seed 20, 20M events, 5M clicks, 1.8 GB),
page cache warm, `NCPU=8`, candidates alternated in one loop, one warm-up round dropped. Wall is process start to exit in ms.

Baseline `941879e`: wall min 197.8, median 198.6, max 199.9 over 9 rounds. Stages: parse 146.3, ev3a 35.5, write 9.4.
Noise is about ±1 ms, so a change needs roughly 2 ms (1%) to count.

| # | Change | Result (min / median wall, ms) | Verdict |
|---|---|---|---|
| 1 | Profile-guided build: instrumented build in the same `rust:1.100.0-alpine` image, trained on seed 20 (20M), seed 3 (10M) and dev, `-Cprofile-use` in the Dockerfile. Needed a `__llvm_profile_write_file()` call before `_exit`. | base 197.8 / 198.6, pgo 197.5 / 198.5. Parse 146.3 vs 146.1. Exact on fixtures, dev and big. | Rejected: no measurable gain, and it adds a pinned toolchain and a binary profile to the build. |
