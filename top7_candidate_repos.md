# Shopping Sessions: likely top-7 repos

As of 2026-11-20, about 11:45 PM ET. Leaderboard at that time: Mahalas 0.65 s, Tark305 0.68 s, Unlimited PTO 0.69 s, Wix 0.71 s, ND 0.75 s, Euphoria 0.80 s, Tyke 0.85 s.

**No repo names itself as any of these seven teams.** I searched all 107 hackathon repos (READMEs, `TEAM.md`, commit messages, file contents, owner GitHub profiles) for the seven names and got zero hits. The candidate list below is inferred from how optimized each repo is and what scores it records for itself. None of it is confirmed, and no repo can be tied to a specific team name.

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

## Eagle Eye (added on request)

| Repo | Team | Language | Commits | Recorded board scores |
|---|---|---|---|---|
| [shanmukha-here/eagle-eye-shopping-sessions](https://github.com/shanmukha-here/eagle-eye-shopping-sessions) | Eagle Eye | Rust | 7 | 2.06 s, 1.60 s, 1.50 s, 1.01 s |

- Its best score is 1.01 s (confirmed against the leaderboard), so it is not in the top 7. "Eagle Eye" is also not one of the seven names.
- One more commit landed after the 1.01 s release (02:08 UTC, cold-read and candidate compaction work), and nothing since.

### How Eagle Eye's repo has its exact score

The 1.01 s is not a prediction. It is a real leaderboard result the team typed into their own code after the scorer returned it.

In `localboard.py`, the `predict` function has four hard-coded rows of the form `[local wall time, local CPU seconds, board seconds]`. The third column is the official score of each of their four submissions. The commit that added the last row is titled "Refit the local board predictor with four scored native releases".

They use those rows to estimate the board time of builds they have not submitted yet:

1. **Measure locally.** `localboard.sh` runs the binary on a 20M-event dataset with the input evicted from page cache, and records wall time and CPU seconds.
2. **Fit a model.** `board_model.py` tries seven simple models (constant, wall ratio, wall affine, CPU offset, CPU affine, CPU plus wait, CPU and wait scales) that map local wall and CPU time to board seconds, trained on the four scored releases.
3. **Pick by leave-one-out error.** It keeps the model with the lowest mean error when each scored release is held out in turn.
4. **Predict the next submission.** It prints a `predicted_board_s`, so they can judge whether a change is worth one of their limited submissions.

The repo records no predicted-versus-actual comparison, so how accurate the predictions are is unknown. Four points is a thin basis for a fit.

## All 107 repos

Every hackathon repo from the search results, alphabetical. This is the full list, not a ranking. "Declared team" is what the repo states in `TEAM.md`, its README or its description; blank means it states none. "Recorded board score" is the last official score the repo itself wrote down; blank means none found. Commit counts and push times are as of about 04:45 UTC on 2026-11-21.

| Repo | Language | Commits | Declared team | Recorded board score | Last push (UTC) |
|---|---|---|---|---|---|
| [AbhirajTiwari12/shopping-sessions-pipeline](https://github.com/AbhirajTiwari12/shopping-sessions-pipeline) | Rust | 49 |  |  | 11-21 03:23 |
| [Adelb150/Baja_Latency_Shopping_Sessions](https://github.com/Adelb150/Baja_Latency_Shopping_Sessions) | C++ | 1 |  |  | 11-21 04:29 |
| [adman40/shopping-sessions-buildathon](https://github.com/adman40/shopping-sessions-buildathon) | Rust | 1 |  |  | 11-21 04:41 |
| [ajiteshbankulaa/DatabricksHackthonThing](https://github.com/ajiteshbankulaa/DatabricksHackthonThing) | Rust | 10 |  |  | 11-21 02:36 |
| [ajp5113/shopping-sessions](https://github.com/ajp5113/shopping-sessions) | none | 0 |  |  | 11-21 00:10 |
| [alexwilldev/shopping-sessions](https://github.com/alexwilldev/shopping-sessions) | Rust | 10 |  | 37.48 s | 11-21 03:16 |
| [AndrewRecinos/shopping-sessions](https://github.com/AndrewRecinos/shopping-sessions) | Rust | 14 |  | 2.14 s | 11-21 03:40 |
| [Annick-Irakoze/shopping-sessions](https://github.com/Annick-Irakoze/shopping-sessions) | Python | 2 | Anika! |  | 11-21 04:38 |
| [Anthony-Voo/Shopping-Sessions](https://github.com/Anthony-Voo/Shopping-Sessions) | Rust | 4 |  |  | 11-21 04:37 |
| [anthonyteti/buildathon-shopping-sessions](https://github.com/anthonyteti/buildathon-shopping-sessions) | Rust | 3 |  |  | 11-21 04:18 |
| [arnabxero/xero-shopping-sessions](https://github.com/arnabxero/xero-shopping-sessions) | Rust | 27 |  |  | 11-21 04:40 |
| [AryanKumar1401/shopping-sessions](https://github.com/AryanKumar1401/shopping-sessions) | Python | 2 |  |  | 11-21 04:33 |
| [atypicality/ShoppingSessions](https://github.com/atypicality/ShoppingSessions) | Python | 10 | Mahjongers |  | 11-21 04:13 |
| [axhiao/Shopping-Sessions](https://github.com/axhiao/Shopping-Sessions) | Rust | 25 |  |  | 11-21 04:01 |
| [Britogus7/Shopping-Sessions](https://github.com/Britogus7/Shopping-Sessions) | Rust | 3 |  |  | 11-21 03:02 |
| [chelseanguyenn/NoReturns](https://github.com/chelseanguyenn/NoReturns) | Python | 14 |  |  | 11-21 02:41 |
| [cherylaurelia/builders-hackathon](https://github.com/cherylaurelia/builders-hackathon) | Rust | 61 |  |  | 11-21 04:34 |
| [devakchow/team-kanyaraasi-databricks-shopping-sessions](https://github.com/devakchow/team-kanyaraasi-databricks-shopping-sessions) | C++ | 5 | Kanyaraasi |  | 11-21 04:41 |
| [Dremix10/shopping-sessions-fast](https://github.com/Dremix10/shopping-sessions-fast) | C++ | 13 |  |  | 11-21 04:26 |
| [dstarnes05/shopping-sessions](https://github.com/dstarnes05/shopping-sessions) | Rust | 10 |  |  | 11-21 02:13 |
| [Dubbus/shopping-sessions](https://github.com/Dubbus/shopping-sessions) | Python | 12 |  |  | 11-21 04:39 |
| [enkyuan/polaris](https://github.com/enkyuan/polaris) | C++ | 6 |  | 1.44 s | 11-21 02:19 |
| [ericle85/shopping-sessions](https://github.com/ericle85/shopping-sessions) | C++ | 1 |  |  | 11-21 02:18 |
| [ErikUmble/buildathon-shopping-sessions](https://github.com/ErikUmble/buildathon-shopping-sessions) | none | 0 |  |  | 11-21 03:57 |
| [explorer-skp/buildathon-shopping-sessions](https://github.com/explorer-skp/buildathon-shopping-sessions) | Rust | 48 |  |  | 11-21 04:37 |
| [FDS-Dark/buildathon-shopping-sessions](https://github.com/FDS-Dark/buildathon-shopping-sessions) | Rust | 5 |  |  | 11-21 03:58 |
| [forgeandfile/shopping-sessions](https://github.com/forgeandfile/shopping-sessions) | none | 0 | Forge & File (from description) |  | 11-21 00:30 |
| [h-ardy/buildathon-shopping-sessions](https://github.com/h-ardy/buildathon-shopping-sessions) | Rust | 5 |  |  | 11-21 04:23 |
| [Hermes-25/builders-shopping-sessions](https://github.com/Hermes-25/builders-shopping-sessions) | Rust | 5 |  |  | 11-21 02:34 |
| [himanshu-thalor/shopping-sessions](https://github.com/himanshu-thalor/shopping-sessions) | C++ | 3 |  |  | 11-21 01:01 |
| [j3278wan/shopping-sessions-buildathon](https://github.com/j3278wan/shopping-sessions-buildathon) | C++ | 4 | SJJJ | 2.60 s | 11-21 01:04 |
| [jacobcorbett/buildathon-shopping-sessions](https://github.com/jacobcorbett/buildathon-shopping-sessions) | none | 1 |  |  | 11-21 03:16 |
| [Jerryh127/fraun-shopping-sessions](https://github.com/Jerryh127/fraun-shopping-sessions) | C++ | 1 |  |  | 11-21 02:23 |
| [jklein07/shopping-sessions](https://github.com/jklein07/shopping-sessions) | Rust | 23 | fortnite pirate token | 1.42 s | 11-21 02:57 |
| [joch44/shopping-sessions](https://github.com/joch44/shopping-sessions) | Rust | 42 |  | 1.97 s | 11-21 03:23 |
| [joch44/shopping-sessions-reference](https://github.com/joch44/shopping-sessions-reference) | Python | 1 |  |  | 11-21 00:15 |
| [Joelsp19/shopping-sessions-duckdb](https://github.com/Joelsp19/shopping-sessions-duckdb) | Python | 3 | Ardere |  | 11-21 04:14 |
| [Johngg2642/shopping-sessions](https://github.com/Johngg2642/shopping-sessions) | Rust | 6 |  |  | 11-21 03:35 |
| [joshuamuellenbach25/Databricks---Shopping-Sessions](https://github.com/joshuamuellenbach25/Databricks---Shopping-Sessions) | Rust | 27 | BackwardsDeployeds | 1.15 s | 11-21 04:41 |
| [jpss2004-bot/shopping-sessions](https://github.com/jpss2004-bot/shopping-sessions) | Rust | 10 | Drunken Coders |  | 11-21 02:06 |
| [kaedeng/shopping-sessions](https://github.com/kaedeng/shopping-sessions) | Rust | 3 |  |  | 11-21 00:40 |
| [KennethLei1492/Databricks---Shopping-Sessions](https://github.com/KennethLei1492/Databricks---Shopping-Sessions) | Rust | 3 |  |  | 11-21 04:43 |
| [kkilborn25/DataBricks-ShoppingSessions](https://github.com/kkilborn25/DataBricks-ShoppingSessions) | none | 0 |  |  | 11-21 03:56 |
| [kundansai-138/b311e47eb0d89b3de98f6f458a0ad652](https://github.com/kundansai-138/b311e47eb0d89b3de98f6f458a0ad652) | Rust | 14 |  | 2.23 s | 11-21 04:38 |
| [kyleuphus/shopping-sessions-submission](https://github.com/kyleuphus/shopping-sessions-submission) | Rust | 1 |  |  | 11-21 03:14 |
| [Louise-Ruza/shopping-sessions](https://github.com/Louise-Ruza/shopping-sessions) | Rust | 1 |  |  | 11-21 04:35 |
| [LuBolin/shopping-sessions](https://github.com/LuBolin/shopping-sessions) | none | 1 |  |  | 11-21 02:39 |
| [MahmoudAlhihi/cedar-ledger](https://github.com/MahmoudAlhihi/cedar-ledger) | Python | 12 |  |  | 11-20 23:53 |
| [matthewf725/shopping-sessions](https://github.com/matthewf725/shopping-sessions) | C++ | 3 |  |  | 11-21 04:26 |
| [MeeksonJr/databricks-shopping-sessions](https://github.com/MeeksonJr/databricks-shopping-sessions) | C++ | 3 |  |  | 11-21 03:05 |
| [micah-andelman/shopping-sessions](https://github.com/micah-andelman/shopping-sessions) | Rust | 32 | 5-hour limit reached | 1.37 s | 11-21 04:37 |
| [mike200004/shopping-sessions-rs](https://github.com/mike200004/shopping-sessions-rs) | Rust | 4 |  |  | 11-21 04:06 |
| [mike200004/shopping-sessions-v1](https://github.com/mike200004/shopping-sessions-v1) | Rust | 1 |  |  | 11-21 01:38 |
| [mliao0128/fraun-shopping-sessions](https://github.com/mliao0128/fraun-shopping-sessions) | C++ | 2 |  |  | 11-21 04:25 |
| [Mridul-Sharma17/buildathon_databricks_shopping_sessions](https://github.com/Mridul-Sharma17/buildathon_databricks_shopping_sessions) | C | 3 |  |  | 11-21 02:30 |
| [muha-0/shopping-sessions](https://github.com/muha-0/shopping-sessions) | Rust | 6 |  |  | 11-21 04:41 |
| [mwkhettab/shopping-sessions-cpp](https://github.com/mwkhettab/shopping-sessions-cpp) | C++ | 8 |  |  | 11-21 03:44 |
| [Myusuf2134/shopping-sessions](https://github.com/Myusuf2134/shopping-sessions) | Python | 1 |  |  | 11-21 00:38 |
| [Nathanatpurdue/Databricks---Shopping-Sessions](https://github.com/Nathanatpurdue/Databricks---Shopping-Sessions) | Rust | 8 |  |  | 11-21 03:17 |
| [nikkhatwani/shopping-sessions](https://github.com/nikkhatwani/shopping-sessions) | Rust | 5 |  |  | 11-21 04:36 |
| [Niranjan6752/shopping-sessions](https://github.com/Niranjan6752/shopping-sessions) | Rust | 4 |  |  | 11-21 04:34 |
| [niranjankumarnk/shopping-sessions](https://github.com/niranjankumarnk/shopping-sessions) | Rust | 21 |  |  | 11-21 03:57 |
| [orbitautomation/buildathon-shopping-sessions](https://github.com/orbitautomation/buildathon-shopping-sessions) | Rust | 1 |  |  | 11-21 01:09 |
| [Osca-K/builders-buildit-shopping-sessions](https://github.com/Osca-K/builders-buildit-shopping-sessions) | Rust | 12 |  |  | 11-21 03:54 |
| [ourux/Databricks-Shopping-Sessions](https://github.com/ourux/Databricks-Shopping-Sessions) | Rust | 1 | ourux |  | 11-21 00:58 |
| [PanditBear/shopping-sessions](https://github.com/PanditBear/shopping-sessions) | Python | 33 | Carter Core |  | 11-21 04:34 |
| [prishalala/shopping-sessions](https://github.com/prishalala/shopping-sessions) | C++ | 12 |  |  | 11-21 03:58 |
| [Prudhv1Raju/shopping-sessions](https://github.com/Prudhv1Raju/shopping-sessions) | Rust | 2 |  |  | 11-20 23:38 |
| [radioheadfan123/shopping-sessions](https://github.com/radioheadfan123/shopping-sessions) | Rust | 7 |  |  | 11-21 04:30 |
| [RagoSpams/shopping-sessions](https://github.com/RagoSpams/shopping-sessions) | Rust | 20 | EzDev | 2.28 s | 11-21 02:30 |
| [Rebecca922/shopping-sessions](https://github.com/Rebecca922/shopping-sessions) | Rust | 3 |  |  | 11-21 04:15 |
| [ricky-he2006/Databricks---Shopping-Sessions](https://github.com/ricky-he2006/Databricks---Shopping-Sessions) | Rust | 1 |  |  | 11-20 23:34 |
| [ruma2061/db-shopping-sessions](https://github.com/ruma2061/db-shopping-sessions) | none | 1 |  |  | 11-21 00:08 |
| [Russell-Day/builders-shopping-sessions](https://github.com/Russell-Day/builders-shopping-sessions) | Rust | 4 |  |  | 11-21 04:41 |
| [saaranshgarg1/shopping-sessions](https://github.com/saaranshgarg1/shopping-sessions) | C++ | 4 |  |  | 11-21 02:13 |
| [saikarthikbattula/shopping-sessions-duckdb](https://github.com/saikarthikbattula/shopping-sessions-duckdb) | Python | 42 | Shopping Sessions |  | 11-21 04:37 |
| [sarbzcode/shopping-sessions](https://github.com/sarbzcode/shopping-sessions) | Rust | 8 |  |  | 11-21 04:40 |
| [sarvesh2003/Hackathon_Submission_Shopping_Sessions](https://github.com/sarvesh2003/Hackathon_Submission_Shopping_Sessions) | none | 0 |  |  | 11-20 23:59 |
| [sarveshjoshi29/buildathonShoppingSessions](https://github.com/sarveshjoshi29/buildathonShoppingSessions) | C++ | 1 |  |  | 11-21 03:57 |
| [shanmukha-here/eagle-eye-shopping-sessions](https://github.com/shanmukha-here/eagle-eye-shopping-sessions) | Rust | 7 | Eagle Eye | 1.01 s | 11-21 02:16 |
| [shifat-sketch/solosynapse-databricks-buildathon-2026](https://github.com/shifat-sketch/solosynapse-databricks-buildathon-2026) | Python | 5 | SoloSynapse |  | 11-21 03:07 |
| [shubham4564/shopping-sessions](https://github.com/shubham4564/shopping-sessions) | C++ | 2 |  |  | 11-21 04:41 |
| [sntin/shopping-sessions](https://github.com/sntin/shopping-sessions) | Rust | 7 |  |  | 11-21 04:42 |
| [sonalrajapaksha/shopping-sessions](https://github.com/sonalrajapaksha/shopping-sessions) | Rust | 24 |  |  | 11-21 04:10 |
| [SophieZ596/shopping-sessions](https://github.com/SophieZ596/shopping-sessions) | C++ | 1 |  |  | 11-21 02:36 |
| [sshasteen2/shopping-sessions](https://github.com/sshasteen2/shopping-sessions) | Rust | 7 | shopping-sessions |  | 11-21 03:25 |
| [SsjRaz/databricks-buildathon-shopping-sessions](https://github.com/SsjRaz/databricks-buildathon-shopping-sessions) | C++ | 16 |  |  | 11-21 04:35 |
| [strobor/shopping-sessions](https://github.com/strobor/shopping-sessions) | Python | 9 |  |  | 11-21 04:19 |
| [SuperHuyGaming/shopping-sessions](https://github.com/SuperHuyGaming/shopping-sessions) | Rust | 9 | GMUNVIS |  | 11-21 02:35 |
| [Surya-Prasad/shoppers-top](https://github.com/Surya-Prasad/shoppers-top) | Rust | 1 |  |  | 11-21 04:04 |
| [suvathikannan17/shopping-sessions](https://github.com/suvathikannan17/shopping-sessions) | Rust | 2 | Suvi Achu |  | 11-21 00:34 |
| [tankat6/shopping-sessions](https://github.com/tankat6/shopping-sessions) | Rust | 1 |  |  | 11-21 03:43 |
| [TejasviniChawla/shopping-sessions](https://github.com/TejasviniChawla/shopping-sessions) | Rust | 55 |  | 1.69 s | 11-21 04:41 |
| [Th3r0n/shopping-sessions](https://github.com/Th3r0n/shopping-sessions) | Rust | 27 |  |  | 11-21 04:21 |
| [toM0dots/shopping-sessions-baseline](https://github.com/toM0dots/shopping-sessions-baseline) | C++ | 29 |  |  | 11-21 04:21 |
| [tyuobd45/shopping-sessions](https://github.com/tyuobd45/shopping-sessions) | Rust | 16 |  |  | 11-21 01:49 |
| [vamsi-kandikonda/hackathon_shopping_sessions](https://github.com/vamsi-kandikonda/hackathon_shopping_sessions) | Rust | 6 | Vamsi Kandikonda |  | 11-21 03:49 |
| [VedantMoghe16/shopping-sessions](https://github.com/VedantMoghe16/shopping-sessions) | C++ | 10 |  |  | 11-21 03:57 |
| [Vibhor7-7/shopping-sessions](https://github.com/Vibhor7-7/shopping-sessions) | Rust | 33 |  | 0.96 s | 11-21 04:39 |
| [Vijayragav-V/shopping-sessions](https://github.com/Vijayragav-V/shopping-sessions) | Python | 2 |  |  | 11-21 01:45 |
| [viktor-lorentz/shopping-sessions](https://github.com/viktor-lorentz/shopping-sessions) | Rust | 14 |  | 2.61 s | 11-21 02:07 |
| [vishusinghsohi/shopping-sessions](https://github.com/vishusinghsohi/shopping-sessions) | Python | 5 |  |  | 11-21 04:25 |
| [vishvap-bhupathi/databricks-shopping-sessions-buildathon](https://github.com/vishvap-bhupathi/databricks-shopping-sessions-buildathon) | Rust | 1 |  |  | 11-21 03:26 |
| [Warrior-hound/shopping-sessions](https://github.com/Warrior-hound/shopping-sessions) | Rust | 9 |  |  | 11-21 04:27 |
| [whyMiso/shopping-sessions-dev](https://github.com/whyMiso/shopping-sessions-dev) | Rust | 18 |  | 1.69 s | 11-21 02:55 |
| [wzardomri/buildathon-shopping-sessions](https://github.com/wzardomri/buildathon-shopping-sessions) | Rust | 10 |  |  | 11-21 04:33 |
| [ZheXie/shopping-sessions](https://github.com/ZheXie/shopping-sessions) | Rust | 13 |  |  | 11-21 03:54 |

## What this is based on

- Metadata, README, file list and commit log for all 107 repos via the GitHub API, plus a shallow clone of each for text search.
- I did not build or benchmark anyone's code. Candidates 1 to 4 rest on how optimized the commit history looks, not on a measured time.
- Recorded scores are whatever each repo last wrote down; teams may have scored better since without logging it.
- Six repos were empty and about 25 had no README.
- The top teams' repos may simply not be in the search results, or may be private until judging.
