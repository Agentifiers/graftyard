# Trail review 6: rows 103 to 121 (Cloudflare segment)

Scope: `decisions.tsv` lines 103 to 121, which is the end of the file. Sources: the files the rows cite, the four scripts, the lead's session transcript (`~/.omp/agent/sessions/-Code-agentifiers/2026-10-24T03-22-18-922Z_01a16bee-….jsonl`, records 1404 to 1719), and the subagent transcripts in the same session directory. Times below are UTC from that transcript.

Disclosure: I run as panel seat B. The seat label `pstack-panel-b` also wrote candidate 2 (`CfManaged`) and the BYOC walk (`CfByocWalk`) [inferred: the label matches my role]. Flag 4 applies to this review too. My own process breaks two rules: I used `head` once and `grep -E` once inside bash pipelines. I also copied the design directory to `/tmp/trib6` to regenerate `research/costs.md` without touching the repo, and I deleted the copy afterwards.

## Flags

1. **High. Row 120 says "resolved" and "the lead read the Summary, Deployment modes, Install operations, Cost", but the lead saw about a third of the revised file before logging it [measured].**
   - Between CfResolve's return (18:51:44) and the row (18:52:35), the lead made two reads (`cloudflare/DESIGN.md:665-720` and `:3-12,599-643`) and three greps. The greps showed lines cut at 512 bytes.
   - Counting every line number those outputs showed, the lead saw 327 of 899 lines. Some ranges it never saw:
     - Install operations 653 to 655 and 658 to 662, which are the revocation and rotation runbooks, the vault, and the lifeboat (CF-ACT-2, 10, 16).
     - The SLO and RPO/RTO tables, 493 to 520 (CF-ACT-5, 10).
     - Most of Failure modes, 532 to 598.
     - The Milestone 0 gates, 729 to 743 (CF-ACT-1, 5, 9, 12).
     - Tradeoffs, 789 to 808.
     - Open questions, 826 to 871, which hold the operator question (CF-ACT-15).
   - The lead read 492 to 520 and 760 to 782 only after logging the row (18:52:39).
   - This repeats `review/trail-review-5.md` flags 1 and 2. The row cites ranges such as `183-261` that the lead saw only as grep hits (183 to 187, 197 to 212, and 252 to 261).
   - Correction row: "row 120 overstated the read. Before logging, the lead saw lines 3-12, 104-124, 599-657 in part, 664-723 and grep hits. 'Resolved' rests on CfResolve's report and `check_cloudflare.py --final`." Then read the unseen ranges, or have a seat that wrote no candidate check the resolutions, before the final reply calls them resolved.

2. **Medium. Nobody walked the revised design again, so the redefined self-hosted mode has never been walked as defined [measured].**
   - All three reviews walked the 742-line version (`review/cloudflare-byoc-walk.md:3`; `review/cloudflare-selfhost-walk.md:166` cites the $168 table).
   - CF-ACT-1 to 17 then rewrote release control (`trib-front`, `trib-ops`, `SiteMode`), the managed topology, the vault, and the meaning of self-hosted (`cloudflare/DESIGN.md:614-624`). The file grew to 899 lines.
   - The self-host walk's persona had "no Cloudflare account". The mode it now describes ("its own Cloudflare account, with no vendor token", line 624) only half-overlaps the BYOC walk's revoke scenario.
   - The AWS rounds ran re-walks after resolution (`review/byoc-rewalk.md`, `review/selfhost-rewalk.md`). No row explains why this round did not.
   - Fix: run a BYOC and self-hosted re-walk of the 899-line file, or log a row that skips it with a reason.

3. **Medium. The act-on list drops or softens six reviewer fixes, while `review/cloudflare-interrogate.md:137-139` says "Dismissed: None… each one checked out" [measured].**
   - Automatic drain on SLO burn, which two seats asked for (`review/cloudflare-foundation-attack.md:21`, `review/cloudflare-byoc-walk.md:69`), became a manual signed `SiteMode` (`review/cloudflare-interrogate.md:23`; `cloudflare/DESIGN.md:399`, "An operator or `trib-ops` may also drain").
   - "During its first day, the rolling site deliberately leads a 1% sample" (`review/cloudflare-foundation-attack.md:51`) became "for its first hour" (`review/cloudflare-interrogate.md:44`; `cloudflare/DESIGN.md:413, 788`). Seat A's Tuesday case (a rare command that does not appear early) is the risk the day-long sample covered.
   - Self-host F8 (major, `review/cloudflare-selfhost-walk.md:107-121`) is never cited. Its BYOC half was not applied: "Rebuild sites from the lifeboat export" (`cloudflare/DESIGN.md:518`) still has no procedure, and the Milestone 7 gate drills only a vault restore (`cloudflare/DESIGN.md:758`).
   - "Require disks that honor cache flushes" (`review/cloudflare-selfhost-walk.md:60`) is missing from CF-ACT-16 (`review/cloudflare-interrogate.md:115`) and from the design (line 657).
   - The semaphore Durable Object for `durable_object`-policy classes (`review/cloudflare-byoc-walk.md:118`), the 1% deploy step (`:67`), and image-storage pricing (`:194`) are not carried.
   - Fix: list each of these under Consider or Dismissed with a reason, or apply it. Add a correction row that counts them.

4. **Medium. Row 116 calls the reviewers "three fresh seats", but each one is the seat that wrote a candidate [measured].**
   - The candidates were spawned on `pstack-panel-a` (C1), `pstack-panel-b` (C2), and `pstack-panel-c` (C3, the base), per the 17:34:37 task call.
   - The reviews ran on the same three seats (18:18:08 task call). C3's author walked self-hosted. C1's author, which is also the flip target, ran the foundation attack.
   - `review/cloudflare-interrogate.md:9` discloses that the seats share one model, but it does not disclose that they wrote the candidates.
   - CfJudge, CfIntegrate, and CfResolve all ran on `pstack-judge`, so the seat that scored the base also wrote and revised it. Row 114 calls that seat "fresh".
   - This repeats `review/trail-review-5.md` flag 5.
   - Correction row naming each reviewer's prior authorship. Add the same note to `review/cloudflare-interrogate.md:11-13`.

5. **Medium. The lifeboat relies on the same workerd features the design rejects for production, and the Stack status hides that from the checker [measured text; risk inferred].**
   - `cloudflare/DESIGN.md:624-627` and `:801` refuse production on customer hardware because `localDisk` is experimental and `localDocker` is "only used for local development and testing".
   - Yet the lifeboat, "the one component outside Cloudflare in production" (line 7), is "pinned workerd + git container + local disk" (line 49) and keeps `uniqueKey` state (line 658). That is the same storage and container engine.
   - A BYOC lifeboat holds every `Tokens` and `Identity` shard (line 661). `research/cloudflare.md:34` describes workerd as "for local dev and tests", "GA [inference]", and "not a hardened sandbox".
   - The design's Stack row (line 99) prints "GA [runtime GA; …]", which drops the research's [inference] label. It passes only because `norm()` strips bracketed text (`scripts/check_cloudflare.py:56-57`). Rows 109 and 120 state that choice openly.
   - Fix: in Deployment modes and Tradeoffs, state that the lifeboat runs on experimental and development-only workerd features and holds token hashes on a runtime that is not a hardened sandbox. Restore "GA [inference]" in the status cell.

6. **Medium. Operator decisions are taken by default, and the trail records only one of them [measured].**
   - The design adopts its own default answer "yes, yes, no" to the operator question (`cloudflare/DESIGN.md:831`). Its Cloudflare-wide outage story (lines 9, 484, 518) rests on the "yes" for the lifeboat.
   - It also sets "no service during a Cloudflare-wide outage for managed users" as the default (line 793). Availability is the operator's first attribute (`FRAME.md:47`).
   - Row 117 records only "self-hosted … pending an operator answer". No row records the managed default.
   - `FRAME.md` has no pending-decision note, although seat C asked for the answer to be recorded there (`review/cloudflare-selfhost-walk.md:157`). The lead accepted workerd as "Cloudflare technology" in thinking at 17:55:52 with no row.
   - Fix: add a row and a `FRAME.md` note that list both open operator decisions, and put both questions in the final reply.

7. **Medium. The Cloudflare doc frames the 50-developer comparison against AWS's dearer setup [measured].**
   - `cloudflare/DESIGN.md:780` names $388 (`scrappy-paired`) as AWS's "cheapest reliable" setup.
   - `DESIGN.md:1092` gives `scrappy` ($226) as the cheapest layout that keeps an availability SLO, and `research/costs.md:118` treats it as reliable. Row 121 itself says $264 is more than $226, but the design table omits that.
   - `research/costs.md:138-142` sets the Cloudflare default ($264) beside the AWS design as written ($1,287), not beside the scrappy rows.
   - The $264 assumes Pro with a 100 MB push cap and a $0 Spectrum add-on that is [unverified] (`cloudflare/DESIGN.md:704, 710`). Neither appears in the comparison row.
   - Fix: add the $226 row to line 780, with its RPO difference (cold `dr` against RPO zero). Add `scrappy` and `scrappy-paired` columns to the Cloudflare table in `scripts/costs_report.py`.

8. **Medium. Row 107 overstates the lead's check of a subagent's edit [measured].**
   - DocsCostProfile (`pstack-judge`) made the `DESIGN.md` edits. The row reads as the lead's own work, and its why says "I grepped and read each changed passage".
   - The lead ran one grep (17:08:45) for small-profile terms. Its lines were cut at 512 bytes, and 1088 and 1092 appear only as mid-line fragments.
   - It never showed the by-size cost table and Cost paragraph (`DESIGN.md:994-999`), which the row lists first. The figures there do match the scripts today (flag 11 aside).
   - Correction row naming DocsCostProfile and the passages the lead actually saw.

9. **Medium. Row 112 says the lead read "all three candidates end to end except a few truncated journey lines" [measured].**
   - The lead's reads skipped:
     - candidate 1, lines 421 to 473 (most of Failure modes);
     - candidate 2, lines 331 to 375 (Failure modes rows);
     - candidate 3, lines 201 to 203 and 421 to 469 (Failure modes rows).
   - `FRAME.md:82` requires an end-to-end read. The pick itself rests on the judge's finding, which the lead did read (candidate 1 lines 64 to 250 include 183 to 185).
   - Correction row listing the unread ranges.

10. **Low. `scripts/audit_evidence.py` now reports 16 missing paths, up from 7, with 9 new in this segment and no row about them [measured].**
    - The new ones are rows 107, 110, 114 (two), 115, 117, 120 (two), and 121.
    - All are bare names that do resolve under full paths: `check_cloudflare.py`, `cost.py`, `costs_report.py`, `check_design.py`, and `index.md` taken from a URL description in row 117.
    - Fix: future rows cite `scripts/…` and `cloudflare/cost.py`. Add a correction row giving the full paths for the 9 rows.

11. **Low. One configuration has two totals: $1,287 and $1,288 [measured].**
    - `research/costs.md:23` and `DESIGN.md:995` say $1,287. `research/costs.md:54, 62` and `DESIGN.md:999, 1378` say $1,288. The two figures come from `scripts/cost_model.py` and `scripts/cost_small.py`.
    - At 17:05:52 the lead's thinking called it "a small inconsistency … rounding artifact" and logged nothing. Row 115 cites both numbers.
    - Fix: one source, or a note in the generator.

12. **Low. Subagent shell breaches are not recorded [measured].**
    - Every subagent in the segment used shell text tools for reading or fetching. Examples:
      - CfIntegrate: `awk … | grep … | cut`, `curl | grep | head`;
      - CfResolve: `grep … | head`, `cat`;
      - CfFoundationAttack: `sed | grep | head`;
      - CfUptime and CfGit: dozens of `sed`, `cat`, and `curl`;
      - DocsCostProfile: `wc -l`.
    - Row 119 covers only the lead's breaches. `review/trail-review-5.md` flag 6 counted subagent breaches.
    - Correction row.

13. **Low. Ordering slips [measured].**
    - Rows 111 and 112 were logged in the same tool batch as the `arena/cloudflare/SYNTHESIS.md` write they cite (18:08:28), so they were logged before that write returned.
    - `review/cloudflare-interrogate.md` was written at 18:34:41, before the doc spot-check at 18:34:48 that row 117 gives as its basis. The check passed, so nothing changed.

## What checked out

- `python3 scripts/check_cloudflare.py --final cloudflare/DESIGN.md` prints `cloudflare/DESIGN.md: OK` with exit 0. `python3 scripts/check_design.py --final DESIGN.md` prints `DESIGN.md: OK` with exit 0. `python3 scripts/audit_evidence.py` reports 228 paths checked, 16 missing, exit 1 (see flag 10) [measured].
- `python3 cloudflare/cost.py` prints $264, $1,296, and $9,380 for the default and $165, $928, and $5,962 for `Solo`. These match `cloudflare/DESIGN.md:673-685`, `research/costs.md:138-176`, and row 121. `--detail large design` matches the line items at 693 to 704, and the derived figures at 687, 689, and 706 add up ($11, $64, and $103; $3,418 at 57%; $99) [measured].
- Running `scripts/costs_report.py` on a copy produces a `research/costs.md` byte-identical to the committed file [measured].
- Row 121's figures hold: container memory is 67% at 50 developers and 42% at 2,000, the vault costs $157, the working set is 30%, 20%, and 10%, and the AWS rows are $226 and $388. Row 110's candidate costs hold ($150/$212, $438, $180). Row 111's scores hold (`review/cloudflare-judge.md:13`), and so do row 112's lead scores (`arena/cloudflare/SYNTHESIS.md:11-13`) [measured].
- Row 108: `research/cloudflare.md` has 36 products and 63 `cf.*` prices. Basin is at line 21 and Artifacts at line 35. The lead's searches returned Basin and Artifacts pages on developers.cloudflare.com. The lead ran three searches, not two [measured].
- Row 109's checker was exercised on `arena/round4/candidate-3/DESIGN.md` at 17:14:43, before the row [measured].
- Rows 113, 118, and 119 match the lead's breaches in the transcript (`cp agent://`, `awk`, `cut`, `curl`, `tr`, `wc`, `head`, `ls`, `grep -n`, `python3 -c`) [measured].
- The reviewers were told "You may attack anything" (18:18:08 task context). Both installs were walked. Rows 116 and 117 count correctly: 35 findings, 3 blockers, 17 act-on items, 2 consider, 0 dismissed [measured].
- Text exists for all 17 CF-ACT items, for example lines 123, 177-178, 413, 614, 655-663, 831, and 884-899. I did not judge whether each resolution is correct [measured].
- The Stack table (`cloudflare/DESIGN.md:77-99`) lists only Cloudflare products from the research catalog. The design states where it leaves Cloudflare: the Summary at line 7 names the lifeboat, and line 831 names Docker. It also states its provider-wide exposure: the Downtime row at 484 is `exposed`, and line 500 says such an event breaks every monthly target [measured].
- `FRAME.md:39` records the operator's words verbatim, and the run's reading carries an [inference] label (row 103) [measured].
