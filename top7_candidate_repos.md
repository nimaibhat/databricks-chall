# Shopping Sessions: likely top-7 repos

As of 2026-11-20, about 11:45 PM ET. Leaderboard at that time: Mahalas 0.65 s, Tark305 0.68 s, Unlimited PTO 0.69 s, Wix 0.71 s, ND 0.75 s, Euphoria 0.80 s, Tyke 0.85 s.

**No repo names itself as any of these seven teams.** I searched all 107 hackathon repos (READMEs, `TEAM.md`, commit messages, file contents, owner GitHub profiles) for the seven names and got zero hits. The list below is inferred from how optimized each repo is and what scores it records for itself. None of it is confirmed, and no repo can be tied to a specific team name.

## Candidates

Ordered by how likely I think each is to be a top-7 entry.

| # | Repo | Language | Commits | Recorded board score | Why it is on the list |
|---|---|---|---|---|---|
| 1 | [cherylaurelia/builders-hackathon](https://github.com/cherylaurelia/builders-hackathon) | Rust | 61 | none | Hand-written Parquet writer, AVX2 dedup tables, read-ahead tuning. Latest commit logs a local "gate warm 0.769". Team setup with a cloud agent. Owner is at Waterloo. |
| 2 | [explorer-skp/buildathon-shopping-sessions](https://github.com/explorer-skp/buildathon-shopping-sessions) | Rust | 48 | none | Still pushing at 04:32 UTC: write-combining partition stages with streaming stores, block scanner A/B tests on x86. Owner builds low-latency systems professionally. |
| 3 | [Th3r0n/shopping-sessions](https://github.com/Th3r0n/shopping-sessions) | Rust | 27 | none | 64-byte structural-mask tokenizer, Parquet pages written straight from column buffers, huge pages, background unmap. `TEAM.md` name left blank. |
| 4 | [AbhirajTiwari12/shopping-sessions-pipeline](https://github.com/AbhirajTiwari12/shopping-sessions-pipeline) | Rust | 49 | none | High commit count over 3.5 hours, but no timing notes anywhere, so this is the weakest of the "no score recorded" guesses. |
| 5 | [Vibhor7-7/shopping-sessions](https://github.com/Vibhor7-7/shopping-sessions) | Rust | 33 | 0.96 s | Lowest score any repo records for itself. `BENCH.md` logs 1.05 s then 0.96 s, with 15+ experiments after that. Could be under 0.85 s by now. `TEAM.md` name blank. |
| 6 | [micah-andelman/shopping-sessions](https://github.com/micah-andelman/shopping-sessions) | Rust | 32 | 1.37 s | Fastest local time found: 0.495 s on 20M (v5.9, not yet scored). Declares team "5-hour limit reached", so it is only a top-7 team if the board name differs from `TEAM.md`. |
| 7 | [joshuamuellenbach25/Databricks---Shopping-Sessions](https://github.com/joshuamuellenbach25/Databricks---Shopping-Sessions) | Rust | 27 | 1.15 s | Still pushing at 04:41 UTC. Declares team "BackwardsDeployeds" and notes "board leader 0.65 s", so it is not Mahalas. Long shot. |

## Added on request

| Repo | Team | Language | Commits | Recorded board scores |
|---|---|---|---|---|
| [shanmukha-here/eagle-eye-shopping-sessions](https://github.com/shanmukha-here/eagle-eye-shopping-sessions) | Eagle Eye | Rust | 7 | 2.06 s, 1.60 s, 1.50 s, 1.01 s |

- The four scores come from the calibration points hard-coded in `localboard.py` ("four scored native releases", commit at 01:49 UTC).
- One more commit landed after that (02:08 UTC, cold-read and candidate compaction work), and nothing since.
- Its best recorded score is 1.01 s, so by its own numbers it is not in the top 7. "Eagle Eye" is also not one of the seven names.
- It ships a local leaderboard simulator (`localboard.sh`, `board_model.py`) that predicts board time from local wall and CPU time.

## Ruled out by their own recorded scores

| Repo | Last recorded board score |
|---|---|
| jklein07/shopping-sessions (team "fortnite pirate token") | 1.42 s |
| enkyuan/polaris | 1.44 s |
| TejasviniChawla/shopping-sessions | 1.69 s |
| whyMiso/shopping-sessions-dev | 1.69 s |
| joch44/shopping-sessions | 1.97 s |
| AndrewRecinos/shopping-sessions | 2.14 s |

## What this is based on

- Metadata, README, file list and commit log for all 107 repos via the GitHub API, plus a shallow clone of each for text search.
- I did not build or benchmark anyone's code. "Candidates" 1 to 4 rest on how optimized the commit history looks, not on a measured time.
- Recorded scores are whatever each repo last wrote down; teams may have scored better since without logging it.
- Six repos were empty and about 25 had no README.
- The top teams' repos may simply not be in the search results, or may be private until judging.
