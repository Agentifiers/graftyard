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

## Not done here

- **Hosted.** The member half ships only through a site day. Until then a member answers `error` and the routes
  answer 503. The hosted day needs the lead's clearance after the merge, within AM2's remaining $9.40.
- **`graftyard_provenance` on `GET /commits/{sha}`:** unowned (`decisions/OPEN-QUESTIONS.md`, A47).
- **A push time for `last_push`:** needs a `PathSet` field and a format (A46).
