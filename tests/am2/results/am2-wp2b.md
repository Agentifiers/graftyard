# AM2-WP2b: `coordination.overlaps`, `provenance.why`, and their edge reads, code only

Branch `am2-wp2b`, from main `e942cd53`. Code only, nothing hosted. This is the second half of AM2-WP2
(`build/BUILD-PLAN.md`, AM2-WP2, "Split"), built as the lead approved the design on 2026-11-23.

## What was built

- **`GET /api/graftyard/v1/repos/{owner}/{repo}/overlaps?ref=&pr=&paths=`** (`crates/api/src/overlaps.rs`).
  - **Subject:** the union of the `PathSet` of `ref`, the `PathSet` of pull request `pr`'s head ref, and the `paths`
    globs (at most 32). With none of the three the answer is 422.
  - **Overlaps:** every other ref's `PathSet` row whose pushed or planned paths overlap the subject by `glob_overlap`.
    A `truncated` row's directories (`pkg/`) cover everything under them. The answer lists at most 100, then sets
    `truncated: true`.
  - **Each overlap** gives `ref`, `pr` (the row's own, else `pull_by_head`'s), `draft`, `app`, `session` and
    `requester` (from the pull request's `AgentMeta`, else the Graftyard-run task whose `refs/heads/graftyard/<uuid>/`
    prefix holds the ref), `last_push` (the head SHA, A46), `shared_paths`, `truncated`, and `verdict: null`.
  - **Leases:** the leases whose paths or refs overlap the subject and that `LeaseView::holds` at the request's time.
  - **Gates:** 503 until 32.14 is enabled; 404 without `contents: read`.
- **`GET /api/graftyard/v1/repos/{owner}/{repo}/provenance?ref=&path=&line=`**.
  - **Steps:** the ref resolves through the rows `rest::resolve` reads (one narrow read). Then the shard's `/ui/blame`
    gives the range that holds `line`, and with it commit C. Blame's 422 (binary, over 1 MiB or 20,000 lines, a
    submodule) passes through. A second narrow read covers C.
  - **Sources:** C's `provenance` rows, each with its ref. A session is `linked: true`. A `Claimed` trailer is
    `linked: false`, cut to 200 characters, and trust-viewed.
  - **Pull request:** the one that merged as C (`Merged { merge_commit }`); else the one whose head is or was C; else
    the newest one opened on a ref C's rows name, open or closed, read from the metadata `pull` rows. A merged pull
    request's `pull_by_head` row is gone, so that lookup could not find it.
  - **Sessions:** the rows' sessions and the pull request's `AgentMeta` session, each with its task's App, requester,
    issue, and snapshot index.
  - **Gates:** 503 until 32.8 is enabled; 404 without `contents: read`.
- **Narrow member reads** (`crates/api/src/narrow.rs`): `Route::Overlaps(OverlapQuery)` and
  `Route::Provenance { commit }`. The member runs the same renderer over its whole fold through the `Recorder`. The
  selections over `path_set`, `lease`, and the pull request lookup are `Directory::pick`s, so the member records only
  the rows the reply renders. `UNSERVED` and `TierRows::dump` are unchanged.
- **The edge** (`workers/edge/src/agents.rs`, `coordination`; `workers/edge/src/api.rs`, `read_repo`):
  - The routes are read-class: `c.read_class` true and `c.write` false before `authenticate`, with the token read and
    the name resolve together, as `rest_repo` does (B0-34's ask).
  - A member that answers these routes `whole` or `error` gets 503 "the repository's members do not serve this read
    yet (site release)", never the whole-tier fallback, which would answer an empty list. A render that asks a pick
    the member did not make gets the same 503.
  - The lease and policy routes are unchanged.
- **`graft-mcp`** (`workers/mcp/src/tools.rs`):
  - `coordination.overlaps` (Coordination, Read) and `provenance.why` (Provenance, Read). Each is one GET after the
    grant check, with `ref`, `path`, and `paths` in the query, so a branch with a slash works.
  - The overlaps tool's description says a null verdict is "not judged yet". `provenance` is not in the default
    toolsets, so a client asks for it by scope.

## Checks

- `crates/api/tests/coordination.rs`, 5 tests over real repository members (AM1's World). Each reply is rendered over
  the whole fold and again from the member's narrow answer, and the two must be equal JSON:
  - overlaps through pushed, planned-only, and truncated-directory paths, with the subject excluded, a ref with no
    overlap left out, the pull request found through `pull_by_head`, one overlapping lease listed and one not, the
    lease gone after its TTL, and the narrow rows holding no row of the non-overlapping ref;
  - the 100 cap with `truncated`;
  - the query's 422s;
  - provenance for a squash commit (through `Merged`) and for both branch commits after the merge removed
    `pull_by_head` (through the `pull` rows), the session from `AgentMeta`, a forged `Claude-Session` trailer unlinked,
    cut to 200 characters, and with its HTML comment hidden, and a human commit with nothing;
  - blame's inclusive ranges, a line past the end 422, and the provenance query's 422s.
- `workers/mcp/tests/wp2a.rs`, `overlaps_and_provenance_are_one_get_each_with_the_ref_in_the_query`: one GET each,
  the exact query strings, 404 with no edge call outside the grant, and `provenance.why` absent from the default
  toolsets.
- `tests/m0carried/plant96.py`: **20 of 20 caught**. The plants cover the subject excluded, planned paths, truncated
  directories, the cap, lease overlap and liveness, the `pull_by_head` lookup, the null verdict, the member recording
  for each route, the squash and closed-branch mappings, an unlinked, capped, and viewed trailer, blame's range end,
  and both tools' query, path, grant check, and toolset.
- The full checks (`tests/am1/runs/checks.sh`) and both edgeboot builds: see the commit message.

## Hosted, day m3fa4 (2026-11-23): both tools on a real repository, and the 503 before the site day

Cleared by the lead after the merge (main `7c9a67f3`), within AM2's remaining $9.40. It ran on graft-m3f, taken
from B0-32 at 12:28Z, from main `7c9a67f3` (`tests/am1/results/install-m3f.md`). The driver is
`tests/am2/hosted/wp2b.mjs`; its rows are `tests/am2/raw/wp2b-m3fa4b.jsonl` (`old`) and
`tests/am2/raw/wp2b-m3fa4.jsonl` (`seed`, `check`, `check3`).

**Order.** First the start sweep (no shard running), RERETIRE OK, and edgeboot on both builds (PASS, 0 traps). The
day ran in two halves so the 503 case was real: first the edge (`ee326616`, from `7c9a67f3`) over B0-32's members
from `ad8fd52f`, which do not know the two narrow specs. Then a fresh `m3org/am2-m3fa4` (repo:65619, 201 in 34 s),
graft-mcp deployed (`day.sh m3fa4` init, claims, register, deploy, smoke), and `wp2b old`. Then the sites. Site a's
first deploy hit wrangler's "Container application settings were not fully applied"; the `--skip-build` rerun
exited 0 with sites a `7474e910`, b `0ee3b84d`, c `ab9a4792`. Images were reused (`containers/` unchanged since
`8d390b0f`), with the same flags and FORMATS as m3fa3.

| Case | Result |
|---|---|
| Before the site day: both routes direct | **pass**: 503 "the repository's members do not serve this read yet (site release)", never an empty list |
| Before the site day: both tools | **pass**: `isError`, `status` 503, `retry_after_s` 5 |
| Overlaps of agent ref x | **pass**: y alone (shares `pkg/auth/session.go`), not z or x itself; `verdict` null, `last_push` y's head, `pr` 1, `session` the task's; one lease (`pkg/auth/**`), not the `infra/**` one |
| The tool's body equals the edge's | **pass**, byte for byte |
| Overlaps by `pr` (x's draft #2) and by `paths` (`web/**`) | **pass**: y and z |
| Provenance of an agent line on x | **pass**: x's commit, lines 1 to 3, draft #2; a `session` source linked, the forged `Claude-Session` `claimed` and unlinked, its `visible` 163 characters with the HTML comment dropped |
| Provenance of a human line (`README.md` on main) | **pass**: no sources, `pr` null |
| A line past the end | **pass**: 422 through the tool |
| Squash-merge y (#1), then provenance of its line on main | **pass**: the squash commit `8419ca91`, #1 merged, the task's session (through `agent_meta`); no sources, as no push wrote that commit |
| After the merge, overlaps of x | **pass**: y's row is gone |
| Anonymous direct read; a repository outside the grant | **pass**: 401; 404 from the tool with no edge call |

**15 of 15 pass.** Two driver faults were fixed during the day, and neither is a product finding:
- The first `old` run (its four rows open `wp2b-m3fa4.jsonl`: both direct reads 503, both tools HTTP 400) sent
  `tools/call` without the `Mcp-Name` header that the 2026-07-28 lane requires. `mcpCall` now sends it, and `old`
  was rerun as tag m3fa4b, still before the site day.
- The squash merge needs two human writers' approvals (`graftyard/agent-gate`, task with no requester). `check`
  merged without them (403), and the orb then lost DNS for graft-mcp for one call, which ended the run. A second
  run (`check2`) tried REST `POST pulls/{n}/reviews`, which the edge does not route (404; only `GET` is served, F61's
  scope), so its two merge-dependent cases are void. `check3` approved through GraphQL `addPullRequestReview` as
  admin and member, merged (200), and passed the last four cases. Cases 1 to 7 come from `check`.

The claimed `ghs_` for the task was refused (422, "not a live graftyard_run task"), as it should be for an
external task. The pushes used the installation token, and the trailer still linked, because the linking rule
takes the App's own sessions on the repository.

**Teardown.** At 13:04:31Z graft-mcp, graft-mcp-aux, and the four `graft-m3f-` store secrets were deleted, and
m3-ci was unflagged (204). net:65619's 3 shards were torn down (200 each). repo:65619 was retired (4 names, all
not joined, retired rows) and purged (25 objects, second pass 0; `tests/am2/raw/retire-m3fa4.log`). End sweep at
13:10:48Z: no shard running. Cost $0.21 for the `graft-m3f` prefix from 12:28:53Z (`acctcost.py`; the account
total of $0.83 includes graft-m2b's running sites).

## Not done here

- **`graftyard_provenance` on `GET /commits/{sha}`:** unowned (`decisions/OPEN-QUESTIONS.md`, A47).
- **A push time for `last_push`:** needs a `PathSet` field and a format (A46).
