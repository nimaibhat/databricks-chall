# For Nimai — origin/main `941879e`

Paste this into a fresh Claude session on this tip. `git fetch origin` on 21 Nov 2026: `origin/main` is still `941879e265c272239d869043c9f6a5ed5a6acaf5` (`single pipeline binary; smaller output files, stale ones cleared`). Nothing landed after it. Do not merge `saran/work`. Do not vendor another tree into this repo.

Public chat, as a bound only: AlexTheGreat7164’s “67” scored 0.30 s, and HonestAbe314 guessed a 0.29 s floor. That floor is the same arithmetic as ~0.21 s in-process plus container start/exit (your note in `72e11ba`). It is not a new algorithm. PythagorasRule6818 asked about AVX-512 VNNI and nobody answered. Do not treat VNNI as a finding. VNNI is an int8 dot-product. This parser does not use it (`k_free.rs` uses AVX-512 masks, `tzcnt`, and `pext`).

`jason-scheffel/express-lane` HEAD `92cf412` (`perf: write smaller output files and clear stale ones`) has the same blobs as this tip for `src/`, `Dockerfile`, `Cargo.toml`, and `.cargo/`. It adds nothing this branch does not have. `automata` (`33bb411`) is a game bot. `nap-time` (`fa5ddde`) is the other track. Ignore them.

## What this binary does

`main` (`src/main.rs` 383–399) is `setup` → `parse::stage1` → `links::stage2` → `events::stage3` → `out::stage45` (the overlap path; `XL_OVL=0` is the old `complex::stage4` then `stage5`). The hot parser is `k_free::Free` (`src/main.rs` line 26), not `src/generic/` (`--generic` is the fallback in `run_generic`).

Warm in-process time on this shape is about 205–215 ms: parse ~160 ms, events ~35 ms, write ~10 ms. `q1` is wall/parse/events/write. The second number on `q1x` is RSS in MB. `q3` labels `h`, `hn`, `rc`, `dev`, `cg`, `p1` are file and pid-1 clocks, not speedups.

## Already in this branch — do not redo

- **Parse.** `k_free::ev_fast` → `ev_s` / `ev_d` (`src/k_free.rs` ~1281–1414). Two 64-bit comma masks, `tzcnt`, `pext` into a date cache (`date_idx` ~658, the two `pext` lookups in `ev_s` ~1329–1331), then one AVX-256 multiply-add that writes both `ts` and `ing` (`ev_d` ~1365–1376).
- **Ids.** Base-36 inverse is `keyrec::BINV` (`src/keyrec.rs` `inverse` / `BINV`). `parse::ev_vec` unscrambles user and event ids with that inverse (`src/parse.rs` ~647–710).
- **Records and dedup.** `Rec` is 16 bytes (`src/main.rs` `Rec`, ~88–92). `ev_vec` packs ingest seconds into `w1` (`ing_s << 16`, ~710). `events::scatter_chunk` (~234–242) keeps the copy with the smaller ingest when the slot generation matches. That compare is the dedup.
- **Sessions.** `GAP_MS` is 1_800_000 (`src/main.rs` ~71). The live path is `events::stage3` → `stage3a` unless `nd > 16`, `BB > 15`, or `XL_OLD3=1` (`stage3` ~2252–2274). `pass_b_step` (~1470–1528) cuts a session on a user change or a gap over `GAP_MS` with an AVX-512 compare. It does not sort the common case. A run that is not in time order is marked special and falls through to `process_run` → `simple_run`, which `sort_unstable_by_key`s only then (`simple_run` ~52–71, the special branch ~1924–1955).
- **Links overlapped with the event tail.** Default queue order is clicks, links, then events (`QORD_DEFAULT` 2, `src/parse.rs` ~1799 and `work_queue_build` ~1938–1942). Once that prefix is done, `stage1` finishes the user table and calls `links::job` between event chunks (~2106–2118). `links::stage2` still runs after parse.
- **Person check.** `pass_b_step` tests the alias bit against the user’s class mask (~1508–1510). `walk_runs` does the same for a simple run (~1644–1646).
- **Shard.** `shard` is a golden-ratio multiply (`src/main.rs` ~287–289).
- **Packaging.** `Dockerfile` is `cargo build --release` on Alpine, then a scratch image with the static binary (`Dockerfile` lines 1–11). `.cargo/config.toml` is `target-cpu=cascadelake`, static relocation, branch padding. `Cargo.toml` `[profile.release]` is fat LTO, one codegen unit, `panic = "abort"`. Allocator is `sys::Alloc` (`src/main.rs` 22, `src/sys.rs` `Alloc` ~653), turned on unless `XL_ALLOC=0` (`main` 319). Huge pages default on (`sys::HUGE`, `alloc_enable` ~670–677). Non-temporal chunk stores default on (`parse::NT`, `line_out` `vmovntdq` ~317–323; `main` 354 sets `XL_NT` default true). Exit is `sys::_exit(0)` (`main` ~539). Parquet is handwritten dictionary/RLE (`src/pqw.rs` `E_RLE` / `E_RLE_DICT` ~167–168, `RLE_KEYS` default on ~677). Stale output is cleared in `out::clear_stale` (~376) before `stage45` (~487).
- **`XL_PREFAULT` is already a knob, default off.** `main` ~325–327 sets `sys::PREFAULT` only when the env var is `1`. Workers then call `prefault_chunk` (`src/sys.rs` ~302–304, ~684–699). Do not reimplement it. Keep it off unless a same-run A/B shows both sizes faster.

`src/` on this tip: `main.rs`, `k_free.rs`, `k_ref.rs`, `parse.rs`, `keyrec.rs`, `links.rs`, `events.rs`, `core.rs`, `complex.rs`, `ckix.rs`, `iface.rs`, `out.rs`, `pqw.rs`, `setup.rs`, `sys.rs`, `generic/{mod,core,parse,util}.rs`. Build files: `Dockerfile`, `Cargo.toml`, `.cargo/config.toml`, `scripts/docker_run.sh`, `scripts/run_fixtures.py`, `scripts/compare.py`. No PGO script.

## Still absent, and large enough

1. **PGO on this source, with the plain release build as fallback. This is the only one.** `Dockerfile` lines 6–7 are a plain `cargo build --release`. No `-Cprofile-generate`, no profdata, no second build. Parse is the ~160 ms stage, and this shape of the binary has not been profiled. Tejas measured about −5% on their tree. joch44 measured −2.7% / −4.1%. On a ~210 ms process that is about 10–30 ms, which is the whole remaining gap from a correct 0.30 s board score down to the guessed 0.29 s floor. Train on a generated set. Same-run Docker A/B of the plain image against the profiled image, then `scripts/run_fixtures.py` and the dev answer. If the profiled binary is not faster, or not exact, ship the plain build.

Tejas’s newer commits (`af79e72` and the `v3:` series under it) are a different, slower pipeline (two-round person resolve, notes). Do not port that. Vibhor’s newer commits (`604f10b`, experiment 43) are another 256-bit timestamp pass on a tree whose submitted scores in those messages are 0.68 s and 0.69 s. This parser already fuses both timestamps. Do not port it. Cheryl is still `2e4261f`. Th3r0n’s `14b4668` drops the event sort for 30-minute buckets; the ordered path here already does that. His `SS_PROBE` commits are measurement probes. Do not copy them.

## Looked at, and not a to-do

- **Lazy `ingest_ts`.** Not implemented as a skip, and it should not be. `ev_s`/`ev_d` decode `ts` and `ingest` in one pass because every row needs `ingest - ts <= DAY_MS` (`ev_vec` ~699) and the dedup word (`ing_s` in `w1`). `scatter_chunk` only compares those seconds when a slot is already occupied. A “decode only on the second copy” change would accept unique rows that are outside the day window. The extra work left in the fast path is the second `pext` date lookup, inside a pass that already runs. Drop it.
- **30-minute bins as a new stage.** The session cut is already `GAP_MS` in `pass_b_step`, with no sort on the ordered path. The sort in `simple_run` is the unordered fallback. The events stage is ~35 ms. Not a rewrite.
- **VNNI, a new scanner, a SIMD dedup table, mimalloc.** Dedup is the direct slot in `scatter_chunk`. The allocator is already `sys::Alloc`. VNNI does not parse this CSV.

The 0.30 s run is near the floor the chat guessed. The next correct gain is PGO on `941879e`, not a new pipeline.
