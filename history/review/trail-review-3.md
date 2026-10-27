I found 12 flags in this segment, listed below. I didn't edit anything. Row numbers are line numbers in `decisions.tsv` (the segment covers lines 56–70), and "jsonl N" is a line in the main transcript.

1. **Row 57.** This row was written in the same parallel batch as the `FRAME.md` edit it reports (jsonl 616), before that edit returned. It also claims "exact words", but they aren't exact. The operator wrote "yes it needs to have org and repo name **uniquess** as we will support…". `FRAME.md:21` drops the "yes" and silently corrects the typo while labelling the quote "in their words". This is the same kind of error trail-review-2 flag 7 raised. [measured]
2. **Row 59.** The operator's request "ensure byoc self installs would work too" (jsonl 638) can be read two ways: BYOC self-installs, or BYOC plus self-hosted installs. The run assumed BYOC plus self-hosted, but no row records that choice. `FRAME.md` has operator-decision sections only for the first two instructions (lines 19–25). [measured]
3. **Row 60.** P8 checks only that each component has a row whose first cell matches its name (`check_design.py:97-111`). A row with empty mode cells passes, so the claim that "a component added later without a per-mode form fails" overstates the gate. The checker's docstring (line 6) still doesn't mention P8. [measured]
4. **Rows 61–62.** Row 62 says "the judge independently picked the same" but leaves out how close it was. The judge scored 22, 21 and 21, and gave candidate 1 a 2 out of 5 on simplicity, including BYOC and self-hosted (`round3-judge.md` Scores; jsonl 686). That came minutes after the operator asked for BYOC and self-hosted to work. The lead never fully scored the baseline ("about 22"). `SYNTHESIS.md` reports both sets of scores honestly; the trail doesn't. [measured]
5. **Row 62 graft.** One graft was "preflight rejects Paired Regions outside MRSC's list" (`SYNTHESIS.md` grafts). It put MRSC into two-Region BYOC installs, and GT-ACT-7 (row 66) reversed it 36 minutes later. [measured]
6. **Row 63, "verified the round-3 integration".**
   - The only check was the structure checker (jsonl 707–709), and that checker says it "Checks structure, not quality". The lead read none of the integrated text before writing the row (jsonl 710).
   - "11 components" comes from the writer's report (jsonl 706).
   - GlobalTierReview later found a blocker in that text (`global-tier-review.md:3`).
   - The row says the writer deferred cross-geography transfers, but the lead ordered that drop (`SYNTHESIS.md` Drops). The resulting launch-scope cut (a 422 on transfers for a public github-style service) has no row of its own.

   [measured]
7. **Unrecorded pivot, jsonl 728 and 737.**
   - The lead wrote "the third time the same underlying premise has failed the same gate… suspicious the premise itself is broken" (meaning fleet-relative freshness boards).
   - It weighed a writer-side redesign, then chose to patch instead, "given the scope exploding", deferring edge cases to milestone tests.
   - It also weighed a strict "no" (404 becomes 503 during outages) against coverage waits on creates, and picked the waits (GT-ACT-2).
   - No row records any of this, or that the lead authored GT-ACT-1 to GT-ACT-6. Row 65 names only IN-ACT-1.

   [measured]
8. **Row 68, "all 8 tier findings resolved".** Nobody re-reviewed the global-tier fixes adversarially. The re-walk brief (jsonl 768) asked the BYOC re-walker to add them "as extra rows". The claim rests on `byoc-rewalk.md:17-24`, written by an install walker, not the tier reviewer. [measured]
9. **Row 70, stopping the loop.** This repeats the gap row 68 gave as its reason to re-walk.
   - OP-ACT-1 to OP-ACT-9 fixed 2 new blockers and 8 majors in the new operator role that runs inside the customer's account.
   - The lead designed that role itself (IN-ACT-1, jsonl 737), moving away from `FRAME.md:27`'s instruction to reuse the vendor's existing BYOC machinery.
   - Those fixes were never walked. Row 69's verification is the checker, the lead reading lines 1131–1183, and a grep.
   - The operator's "ensure it works" is therefore closed on unreviewed fixes, though the run does say so.

   [measured/inferred]
10. **Rows 68 and 59, written before their actions returned.** Row 68 cites `interrogate.md`'s "Fifth pass" as evidence, but was written in the same batch as the edit that created that section (jsonl 787). Row 59 was written in the same batch as the messages steering the round-3 runners (jsonl 639). The row-68 tallies do check out against `selfhost-rewalk.md:1` and `byoc-rewalk.md:1`: 7+4 resolved and 5+8 partial. [measured]
11. **Unrecorded gate change.** The P7 pattern was widened from `(?:NS-)?ACT` to `(?:[A-Z]{2}-)?ACT` (jsonl 740–747; `check_design.py:131`). It makes the gate stricter, but no row records it. [measured]
12. **Tool-rule breaches, unrecorded.**
    - The lead created review files with bash `cp agent://… review/*.md` instead of the write tool (jsonl 687, 728, 781). At jsonl 737 it says it is avoiding shell writes because the trail reviewer flagged them.
    - It paged and searched with shell `awk 'NR>=50'`, `grep -E` and `awk` (jsonl 687, 728, 734, 762, 781), and used `cd … &&` instead of `cwd` throughout.
    - SelfHostRewalk paged `DESIGN.md` with `sed -n` nine times.
    - Judging from their tool-call lists, none of the read-only seats changed anything.

    [measured]

Running `audit_evidence.py` found 122 cited paths with 2 missing, both before this segment (rows 3 and 52). Running `check_design.py DESIGN.md --final` now exits 0 at 1,433 lines, which matches row 69. [measured]