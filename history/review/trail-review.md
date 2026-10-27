Twelve flags, most serious first. Each is labelled measured (seen directly in the transcript or files) or inferred.

1. **04:24:59 interrogate row ("design is contested at its core bet").** The reviewer brief put the core bet, one DSQL database holding everything, inside an "Intent (do not question it; challenge the execution)" block (spawn context at 04:24:55). `review/interrogate.md:60` then counts "No reviewer attacked the core bet" as agreement with the judge. The adversarial pass was told not to test the one choice it was run to test. *measured*
2. **04:38:03 row ("36 findings … zero dismissals").** Each reviewer was capped at 12 findings, and A, B and C each returned exactly 12 (`### ` headings in `review/reviewer-{a,b,c}.md`). The cap was binding, so "zero dismissals" may be partly an effect of the cap. *measured / inferred*
3. **04:20:06 pick row (candidate 3 as base).** The stated reason is the rubric's maintainability tie-break, but the operator ranked uptime first. The lead's own reasoning at 03:44:28 favoured the S3 log design because git could survive a metadata-database outage. The judge's margin was 26 to 25, and candidate 3 scored lowest on availability (`interrogate.md:60`). Milestone 0 tests DSQL load, not DSQL failure (`interrogate.md:47`). No row accepts "git availability equals DSQL availability"; that tradeoff appears only as a "Consider" item in `interrogate.md`. *measured*
4. **Same row, "cross-judge independently picked the same base".** No panel seats are configured, so every seat runs the lead's model (`modelRoles` output at 03:26:07; the 03:35:50 row admits this). The judge also ran on panel-b, the seat that wrote candidate 2. The agreement between lead and judge is a weak signal. *measured*
5. **04:14:34 row ("read all three candidates end to end").** The lead never read candidate-1 lines 501-575 or candidate-3 lines 561-581, the wire-compatibility tables. The thinking at 04:13:20 says "move quickly through the rest". Candidate 1 was read in 12 seconds (04:13:08 to 04:13:20), after the lead had already judged it "likely weaker" (04:13:17). *measured*
6. **04:54:41 revise row ("verified the ten ACT resolutions landed").**
   - The `--final` check (`scripts/check_design.py:110-121`) only looks for each ACT-n id under "## Review resolutions", a section the delegate wrote itself.
   - The lead read only `DESIGN.md:404-437` (the merge sketch, ACT-1) and `840-854`. The outbox and tenant-fence spot-checks it planned at 04:54:31 never happened.
   - The bodies of ACT-2 to ACT-10 were never read. *measured*
7. **Same row: ResolveActItems' own additions were accepted without review** (its result at 04:54:20):
   - an 800-deletion cap per atomic push, a new deviation that clients can see;
   - a migration freeze bound raised from 30 s to 60 s;
   - numbers it chose itself (5 route changes, 10 failures, 7-day pruning);
   - new [unverified] claims (Route 53 fail-open, multi-Region KMS keys, the NLB quota).

   None of these has its own row. *measured*
8. **04:24:37 graft row: lead fixes based on recall, not sources.** The thinking at 04:19:58 relies on memory for three things: "GitHub enforces global SSH key uniqueness… well documented", "GitHub updates `refs/pull/N/head` asynchronously", and "GitHub caps pushes around 5,000 refs".
   - The SSH fix became ACT-6, rated critical.
   - The derived, lagging `refs/pull/N/head` is the root of ACT-1 (`interrogate.md:19`), yet the 04:38:03 row blames only the SSH graft on the synthesis.
   - The graft row was never superseded. *measured / inferred*
9. **ACT-6 fix adds a per-tenant NLB for every managed tenant** (`interrogate.md:29`). This is a large cost and quota change, logged only inside "10 ACT items", and `DESIGN.md` marks the NLB quota and price as [unverified]. *inferred*
10. **03:53:52 arena row (all three bets have "production lineage … Azure DevOps").** Refs-as-rows, the design that won, has only a secondary source. Microsoft documentation was not found (`research/git-storage.md:46,114`), and its cost is rated "Very high" (`git-storage.md:133`). *measured*
11. **03:32:34 frame row ("reuse the BYOC machinery") was never closed**; it still reads "open, research slice byoc-grounding to confirm". ByocGrounding found no handling for stateful fleets or migrations, and health checks that cannot see stateful resources (03:38:46). That partly undercuts reuse for a stateful git host, and it was only folded into the "BYOC gaps" fact on the 03:53:52 row. Separately, the 03:46:50 row says "added to arena TASK constraints" seven minutes before the `TASK.md` edit at 03:53:22. *measured*
12. **Decisions and pivots the trail does not record:**
    - **Truncated request.** The prompt ends mid-sentence ("design. use"). The lead noticed at 03:25:39 and carried on. The 03:32:34 "no human checkpoint" row cites only `FRAME.md`, although figure-it-out says to present the framing first and that a multi-hour run earns one checkpoint. This run took about 90 minutes. *measured*
    - **GitHub token prefixes.** Reusing GitHub's prefixes (`DESIGN.md:323,647`) means third-party secret scanners will flag these tokens as GitHub's. The lead flagged this at 03:54:09 as something for the interrogate step, but it never got a row or a place in the reviewer brief. *measured*
    - **Arena directions.** The third arena direction moved from single-tenant appliance, then hybrid (03:27:23), then two directions only (03:44:28), before landing on refs-as-rows. Only the final set is logged. *measured*