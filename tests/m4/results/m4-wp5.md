# M4-WP5: the web app on a hosted edge

BUILD-PLAN M4-WP5 asks for the web app reading public GraphQL and REST with persisted queries plus the private `/_t/ui/*` API, an HTML shell with the first route's data inlined, immutable static assets, one error boundary and query per region, and BF-38 and BF-61 under Playwright on a hosted edge. **BF-38 passes on graft-m4a, its running-job live log included (edge day 12, 15:09Z on 2026-11-14). BF-61 passes on graft-m4a: `bf61.spec.mjs` printed one clean result on wp3-live job 1114123 at 16:25Z on 2026-11-14 (edge day 13, no new site day), with lines within 2 s, the fallback 2.2 s after the relay's kill, and a complete final log. M4-WP5 is done.**

Branch `m4-web`, cut from `origin/main` at `20147db`, merged with main at `3b18ded` (M5-WP1's `release.rs`, which graft-m4a edges need).

## What was built

- **The design first.** `design/SHARED.md`, "Frontend and native app", "How the edge serves it (M4-WP5)": the shell and `ASSETS`, the `__graft_seed` inline, persisted documents, the session cookie, the `/_t/ui` routes, and client retries. `decisions/DECISIONS.md`, "The edge serves the web app… (2026-11-13)". F55, F61, and F62 updated.
- **The git cache's `/ui/*` routes** (`containers/git/server/ui.go`): `resolve`, `tree`, `blame`, `diff` (from the merge base), `compare`, `readme`, and `commits`, read-only, under the network's shared lock. The shard forwards them (`UI_ROUTES`). Deployed on graft-m4a by site days 2 to 4 (image `sha256:099a022c…`).
- **The edge** (`workers/edge/src/ui.rs`):
  - `/_t/ui/*` only: no GHES 3.22 path changes. The shard's 404 maps to 404, 400 and 422 to 422, and anything else to 503 with `Retry-After`.
  - The web session: `POST /_t/ui/login/session` (PBKDF2-HMAC-SHA256, `crates/identity/src/password.rs`) sets `__Host-gy_session` (`Secure`, `HttpOnly`, `SameSite=Strict`). It counts only with `x-graftyard-web: 1`, which a GHES client never sends. `PUT /_t/ui/settings/password`.
  - The shell: `_shell.html` from `ASSETS` for a browser's `GET` of a web path, with the seed inlined for `/{o}/{r}` and `/{o}/{r}/pull/{n}` (each tab). `/assets/*` answers `public, max-age=31536000, immutable`; a missing asset is 404.
  - Persisted GraphQL: `web/src/api/persisted.json`, embedded with `include_str!`; an unknown hash answers `PERSISTED_QUERY_NOT_FOUND`.
- **The web app** (`web/`): every client sends `x-graftyard-web: 1`. Reads retry on 5xx and network errors within a 10-token budget (refilling 0.1 per success, at most 2 retries, waiting at least `Retry-After`, capped at 10 s); writes never retry. The seed is loaded before the router starts. Files, commits, and the README read `/_t/ui`. The merge box polls the persisted `PullMergeState` query every 3 s while `mergeable` is `UNKNOWN`. Each page region has its own boundary. The mock (`VITE_MOCK_API`, the default for `npm run dev` and `npm run build`) still answers every route, including the new `/_t/ui` and GraphQL ones.

## Found by running BF-38 on the hosted edge, and fixed

1. **A private repository's shell had no seed.** A browser's document `GET` cannot carry `x-graftyard-web`, so the cookie never counted there. The shell's own `GET` now accepts the cookie alone, because the seed is read-only, lands in HTML no cross-origin page can read, and `SameSite=Strict` keeps the cookie off a cross-site navigation. SHARED records the exception. (`3a88585`, edge day 5.)
2. **A review comment without a review did not show on the timeline.** The edge's REST `POST /pulls/{n}/comments` stored the comment with `pull_request_review_id: null`, and the timeline drew threads only inside their review. A thread whose review is not listed now shows on its own (`91464e1`), and `m4-web-2` gives such a comment its single-comment review, as GHES does.
3. **The agent card failed.** The edge's `application/vnd.graftyard.agent+json` reply carries only `graftyard_trust` so far; the card read `graftyard_overlaps.with` unconditionally. The other agent fields are optional in the type now. (`91464e1`.)
4. **`seed.mjs`** survives a repository create that outlasts the client's 180 s timeout (161 s measured).

## BF-38

`tests/m4/bf38/`: `seed.mjs` builds the data through product routes only, and `bf38.spec.mjs` runs it.

```
source tests/am1/lf13/env-m4a.sh
BF38_PASSWORD=… node tests/m4/bf38/seed.mjs web38-a > facts.json
cd tests/m4/bf38 && BF38_EDGE=$GRAFT_EDGE BF38_FACTS="$(cat facts.json)" BF38_PASSWORD=… npx playwright test
```

Data: `m3org/web38-a` (private), `main` with a README, `src/`, and `docs/`; PR #1 from `web38/retry-after` (2 commits, 3 files), two issue comments, a review comment on `src/retry.ts` line 5, three check runs by App m3-ci (success, failure, in progress), and a `ci/preview` status.

Run at 13:24Z on 2026-11-13 against `graft-m4a-edge` `3911b234` (product build): **6 passed, 1 pending.**

| Step | Result |
| --- | --- |
| Sign in at `/login` with the web password | `__Host-gy_session`, `HttpOnly`, `Secure`, `SameSite=Strict` |
| Repository overview | 4 queries seeded; directory and README rendered; **0 seeded reads** before render or first paint; **0 `/api` or `/_t` requests before first contentful paint** (FCP 5,736 ms, the first read at 5,806 ms) |
| PR timeline | 12 queries seeded; comments and the review thread rendered; 0 seeded reads; 0 requests before FCP (3,368 ms). `mergeable` was `MERGEABLE` on the first persisted poll |
| PR diff (`?tab=files`) | 12 queries seeded; the file tree and patches rendered; 0 seeded reads; 0 requests before FCP (1,360 ms) |
| PR checks (`?tab=checks`) | 10 queries seeded; "1 of 3 checks passed, 1 failed, 1 running" and `ci/preview`; 0 seeded reads; 0 requests before FCP (1,092 ms) |
| Checks query forced to fail | check-runs removed from the seed and answered `503` with `Retry-After: 1`: 13 failed reads (the region's retries plus the tab bar's and merge box's reads). **Only `checks` showed an error**; the header, tabs, and sidebar rendered. On the timeline with the same fault, no region showed an error |
| Running job's live log | **passed** on edge `3bd23b1d` (below): the page joined the job's relay, showed Live, and drew new lines as the runner sent them |

The reads after first paint are the chrome's own widgets (notifications, watch, star, branch count, check-suites for re-run buttons, reactions), which the design leaves unseeded. Several of those answer 404 on this edge because their REST routes are not served yet (F61), and their widgets render nothing on a 404.

`mergeable`: the seed's `GET /pulls/1` asked for the test merge, so it was computed before the run; the polling path (UNKNOWN, then a value) was checked on the mock, where the first poll answers `UNKNOWN`.

### Re-pass on edge `3d41cbe7` (edge day 7)

About 16:02Z on 2026-11-13: **6 passed, 1 pending** (live log) in 41.9 s. Overview FCP 3,908 ms, timeline 1,324 ms, diff 2,252 ms, checks 1,128 ms, each with 0 seeded reads and 0 reads before first paint; `mergeable` `MERGEABLE`; the checks fault answered 10 check-run reads 503 and only the checks region showed an error.

### Hosted `mergeable` on a fresh pull request

`m3org/web38-a` PR #3, opened from a new branch by REST at 06:50Z on 2026-11-14 (PR #2 earlier showed the same). Polling the persisted `PullMergeState` query: `UNKNOWN` at 442 ms after the create, `MERGEABLE` at 1,237 ms. REST at page open: `mergeable: null`, `mergeable_state: "unknown"`. The page, opened 2 ms after the create, settled at 4.6 s with `MERGEABLE` / `CLEAN` and no region errors. It never showed the "Checking…" row, because the merge was computed before its first poll; that row was seen only on the mock.

### Edge day 8 (`bcb4ed2a`), branch `m4-web-2`

Built from main `5c0f06fb` merged with `m4-web-2` `97841ed` (which ships `8014ebb` too):

- **Pull request counts.** The shard's `/ui/compare` returns `additions`, `deletions`, and `changed_files` from the merge base, and the edge reads it for every pair a REST pull, GraphQL query, or shell seed renders, then renders again (at most 50 pairs a request). Until the new git shard image ships in a site day, the old shard answers only `ahead_by`: on `bcb4ed2a`, PR #1 shows `commits: 2` (was 0) and PR #3 `commits: 1`, while `additions`, `deletions`, and `changed_files` stay 0.
- **Single-comment reviews.** REST `POST /pulls/{n}/comments` outside a review gives the comment a COMMENTED review, as GHES does, decided by the site's leader, so it waits on the same site day. The web timeline keeps showing a thread whose review is not listed (`91464e1`): the edge gives no such review to a commenter whose verdict approves or requests changes, and older comments, like PR #1's, keep none.

Checked: `/api/v3/user`, `/repos/m3org/j4`, `pulls/1`, `pulls/3`, `/hooks`, `/api/v3/meta`, and git `info/refs` answer 200; the shell answers 200 to a document request; an asset answers 200 with `public, max-age=31536000, immutable`. BF-38 at 08:41Z on 2026-11-14: **6 passed, 1 pending** (live log) in 46.8 s. The diff tab's "Files changed 0" and "+0 −0" are the counts above; the tree and patches show 3 files.

### After site day 26: counts and single-comment reviews, hosted

Site day 26 (M4 Actions, 09:10Z on 2026-11-14) put git shard image `sha256:969fd147…` and main `d41da860`'s site code, which includes `m4-web-2`, on all three sites. `m3org/web38-a`'s shards (repository 65542) had run since 06:47Z on the old image, and a container keeps its image until it restarts, so M4 Actions tore them down at 09:23Z; the next read started them on the new one. The edge stayed `bcb4ed2a`.

| Read, 09:25Z | PR #1 | PR #3 |
| --- | --- | --- |
| REST `commits` / `additions` / `deletions` / `changed_files` | 2 / 8 / 2 / 3 (were 2 / 0 / 0 / 0) | 1 / 1 / 0 / 1 (were 1 / 0 / 0 / 0) |
| GraphQL `additions` / `deletions` / `commits.totalCount` | 8 / 2 / 2 | 1 / 0 / 1 |
| `/_t/ui` files, summed | +8 −2, 3 files | +1 −0, 1 file |
| Web tab bar | "Files changed 3", "+8 −2" | "Files changed 1" |

A fresh review comment on PR #3 at 09:22:00Z (`POST /pulls/3/comments`, `docs/fresh.md` line 1) answered `id` 491524 with `pull_request_review_id` 491525, which `GET /pulls/3/reviews` lists as COMMENTED with an empty body on the head commit. Its reply (`in_reply_to` 491524) answered 458756 with review 491526. The timeline shows both comments once, inside m3-admin's "reviewed" entry. BF-38 at 09:26Z: **6 passed, 1 pending** (live log) in 46.4 s.

### Format 32.22 on graft-m4a: an approver's review comment

Site day 27 (M4 Actions, 10:18Z on 2026-11-14) put members that read 32.22 on all three sites (`f0cef289` / `98fa6950` / `dce37093`), and edge day 9 (`4a4dcf9a`, main `a2d918f1`) added 32.22 to `FORMATS_ENABLED`. The same probe ran on m3org/web38-a PR #3 before and after (`m3-member`, write access, approves through `addPullRequestReview`, then posts a review comment and a reply):

| | Before 32.22 (edge `bcb4ed2a`, 10:08Z) | After (edge `4a4dcf9a`, 10:22Z) |
| --- | --- | --- |
| Comment's `pull_request_review_id` | `null` (491531) | 491534, COMMENTED, empty body, by m3-member |
| Reply's `pull_request_review_id` | `null` (458758) | 491535 |
| Merge box's reviews row (`/_t/ui/…/merge-requirements`) | "Changes approved" | "Changes approved" |
| Reviewers sidebar | m3-member Approved | m3-member Approved |

The timeline shows the 10:23 comment and its reply once, inside m3-member's "reviewed" entry; the 10:08 comment, which has no review, shows on its own through the unlisted-thread view. BF-38 on `4a4dcf9a` at 10:25Z: **6 passed, 1 pending** (live log) in 43.1 s.

### The running job's live log, hosted (edge days 10 to 13)

M4-WP3's run service went to graft-m4a on site day 28 (`LogRelay`, `RunnerBroker`) and edge day 10 (`workers/edge/src/actions.rs`). M4 Actions ran an unmodified runner 2.338.2 on `m3org/wp3-live` (private) with `tests/m4/wp3/hosted/live.yml`, which prints one line a second for ten minutes. The viewer opens `/_t/logrelay/repos/{owner}/{repo}/jobs/{job}` (`m4-web-5` `c4fe2898`).

- **Job 1114114, edge 11 (`4ba3663e`), 14:50Z: 6 passed, 1 failed.** The job page rendered from the new GHES reads (`actions/runs/{id}`, `runs/{id}/jobs`, `jobs/{id}`), but its socket was refused. A browser `WebSocket` cannot send `x-graftyard-web: 1`, so `ui::session` read the viewer as anonymous and the private repository's relay answered 404. The fallback then read `/logs`, which was 404 while the job ran, and the region showed "Could not load the log". Fixed on the edge: an upgrade whose `Origin` is the edge's own `https://` origin uses the session cookie (`464e1d3d`, edge day 12; SHARED, "Sign-in and the session cookie"). M4 Actions made a running job's `/logs` serve its finished steps (`m4-actions-2` `8afafac5`, edge day 13).
- **Job 1114117, edge 12 (`3bd23b1d`), 15:09Z: 7 passed** in 50.6 s, with `BF38_JOB={"repo":"m3org/wp3-live","run":1114116,"job":1114117}`. The page showed Live and the line count grew, with `hello` and `line` frames on the socket. The other six steps matched earlier runs: 4, 12, 12, and 10 queries seeded, 0 seeded reads before render or first paint, `mergeable` `MERGEABLE`, and 11 check-run reads answered 503 with only the checks region in error. A second capture at 15:10:49Z, scrolled to the live cursor, shows line 283 stamped 15:10:48.25 under "streaming", and the count grew 294 → 299 in 5 s.
- **Socket probe** alongside, with m3-admin's token: for each line, the delay from the runner's own timestamp in its text to arrival.

| Job | Lines | Gaps | Lag p50 | p99 | max | End |
| --- | --- | --- | --- | --- | --- | --- |
| 1114114 (joined 14:52:54Z) | 276 | 0 | 350 ms | 608 ms | 744 ms | `end` succeeded, close 1000 |
| 1114117 (joined 15:09:10Z) | 418 | 0 | 408 ms | 571 ms | 707 ms | `end` succeeded, close 1000 |

Afterwards `/logs` answered 200 with 623 lines (37,385 B) for each job, and the job read `completed` / `success` with five successful steps. The relay's own `line.at` is the site's clock at arrival, to the second, and ran about 4 s ahead of the runner and the edge [measured, cause not checked], so the lags above use the runner's timestamp.

Checked on edge 12 with a real session cookie: the edge's `Origin` opens the socket; a foreign `Origin`, no `Origin`, or the edge's `Origin` without the cookie is refused.

### BF-61: the running job's live log, then a killed `LogRelay` (edge day 13)

Site day 30 (M4 Actions, 15:40Z) gave `LogRelay` an `/abort` that only a binding reaches. M4 Actions ran live.yml as wp3-live run 1114119, job 1114120 (runner from 15:42:01Z; relay `logrelay:1114120` on site a), and at 15:47:00.025Z by its orb's clock (15:47:05.05Z by Cloudflare's) aborted that relay through a scratch Worker, answered 200 `{"aborted":true}`. `tests/m4/bf38/bf61.spec.mjs` ran the web page from 15:42:45Z with `BF61_JOB={"repo":"m3org/wp3-live","run":1114119,"job":1114120}`, and the socket probe ran beside it.

| Check | Result |
| --- | --- |
| Lines within 2 s | probe: 251 lines (runner lines 47 to 297), no seq gaps, lag p50 302 ms, p99 422 ms, max 529 ms from the runner's timestamp; the page showed Live from 15:42:58Z |
| The kill | the probe's socket errored after line 297 (runner 15:46:59.50) with no `end`; the page's socket closed at the same moment and it showed "Live stream unavailable, showing saved logs (refreshing)" by 15:47:06Z, with no error region |
| Fallback | the page polled `/logs` every 3 s; while step 3 ran it held steps 1 and 2 only, because the runner uploads a step's log at the step's end |
| Final log | the job ended `completed` / `success` (15:52:15Z, Cloudflare); `/logs` 200 with 623 lines, the counter's lines 1 to 600 all present (lines 298 to 330, across the abort and the runner's reconnect, included) and `last step`; a fresh load of the page reads 624 lines from the saved log, "Success in 10m 13s", every step completed, no error region |

Both lag figures use timestamps from different clocks: the runner's on M4 Actions' orb, which reads about 5 s behind Cloudflare's, and the probe's on this orb, about 4.5 s behind Cloudflare's (three `Date` readings, 15:49Z). The two orbs agree to about half a second, so the lags hold to that much; the relay's `line.at`, from the site's clock, is why it read about 4 s ahead of the runner.

After the abort the runner noticed on its next send, about 27 s later, and reconnected its feed within a second to a fresh relay with no tail [M4 Actions' runner log]. A viewer joining at 15:47:39Z got `hello` with an empty tail and, before the reconnect, no lines. The page did not rejoin: it stays on the saved log until the job ends.

The spec's run did not print its result: after it saved `final.log` at 15:52:14Z it stalled on a page action with no timeout, most likely the scroll to `line 600`, which a completed job's page keeps in a collapsed step [inference], and it was stopped at 15:56Z, before its 20-minute test timeout. Every figure above comes from the probe, the screenshots, `final.log`, and the fresh page load. The spec now drops that action and sets a 30 s action timeout.

### BF-61, printed (wp3-live job 1114123)

M4 Actions started live.yml as run 1114122, job 1114123 (relay `logrelay:1114123` on site a), at 16:15Z and aborted the relay at 16:20:00.113Z by Cloudflare's clock through `graft-m4a-relaykill`, answered 200 `{"aborted":true}`. The runner noticed about 33 s later and reconnected to a fresh relay; the job ended `success`. `bf61.spec.mjs` attached at 16:16:05Z and printed **1 passed (9.6 min)**:

```
BF61_RESULTS {"live":{"n":220,"first":51,"last":270},"closedAt":"2026-11-14T16:19:55.204Z","endBeforeClose":false,"fallbackMs":2237,"linesShownAfterFallback":14,"conclusion":"success","final":{"lines":623,"counted":600,"missingCount":[],"lastStep":true,"liveLinesMissing":[],"pageLines":624,"pageSource":"saved"},"lag":{"n":220,"p50":392,"p99":500,"max":665,"over2s":0}}
```

| BF-61 column | Result |
| --- | --- |
| Lines appear within 2 s | 220 lines on the page's socket (runner lines 51 to 270), lag p50 392 ms, p99 500 ms, max 665 ms, none over 2 s |
| After the kill, the viewer falls back to step log blobs | the socket closed at 16:19:55.2Z on this orb's clock (about 16:20:00 on Cloudflare's) with no `end`; "showing saved logs (refreshing)" 2.2 s later, with no error region |
| The final log is complete | `/logs` 623 lines with the counter's 1 to 600 and `last step`; every line the page saw live is in it; the page shows 624 saved lines, "Success in 10m 13s" |

The socket probe beside it agreed: 229 lines with no gaps, runner lag p50 386 ms, max 663 ms, dropped after line 270 with no `end`.

### Comments that still need the timeline's unlisted-thread view (`91464e1`)

The timeline still shows a thread whose review is not listed, because these comments have no review:
- every review comment created over REST before site day 26 put `m4-web-2`'s decide on the sites, 09:10Z on 2026-11-14: on graft-m4a, `m3org/web38-a` PR #1's "Cap this at 10 s…" (BF-38's seed) is one;
- a comment by a reviewer whose verdict on that pull request was APPROVED or CHANGES_REQUESTED, made before an edge enabled format 32.22: on graft-m4a, 10:22Z on 2026-11-14 (edge day 9), so PR #3's 491531 and its reply 458758 are two.

No new comment joins either group, and those comments stay as they are: no event rewrites a committed comment's `review`. The view can go when no install holds such a comment, which for graft-m4a means reseeding BF-38's repository.

## Edge days

| Edge day | `m4-web` | Version |
| --- | --- | --- |
| 3 | `863dd9d` (the web app) | `a57a7a87` |
| 4 | `a3315fa` (favicon, missing asset 404) | `f588de76` |
| 5 | `3a88585` (seed from the cookie) | `d9956ab8` |
| 6 | `91464e1` (timeline threads, agent card) | `3911b234` |
| 7 | main `334d968` + `m4-actions` `108c1b2` + `m4-wp2` `4e9da43`, for M4-WP2 | `3d41cbe7` |
| 8 | main `5c0f06fb` + `m4-web-2` `97841ed` (counts, single-comment reviews; `8014ebb`) | `bcb4ed2a` |
| 9 | main `a2d918f1` (`m4-web-3`: format 32.22 in `FORMATS_ENABLED`) | `4a4dcf9a` |
| 10 | m4a-deploy `f731c443` (M4-WP3's run service) + `m4-web-5` `c4fe2898` (the viewer path) + main `303fad5f`; `RUN_SERVICE_PUBLIC` | `474233ba` |
| 11 | main `5ba2c7d8` + `m4-actions` `7d0a7bfe` (GHES run and job reads) + `m4-web-5` `bafca7cc` | `4ba3663e` |
| 12 | edge day 11 + `m4-web-5` `464e1d3d` (the viewer socket's session by `Origin`) | `3bd23b1d` |
| 13 | main `e8e1f4da` + `m4-actions-2` `8afafac5` (a running job's `/logs`) + `m4-web-5` `464e1d3d` | `6a9c119b` (BF-61) |

Each was built from `m4-web` merged with `origin/m4-actions` `83afa6d`, deployed with `deploy-edge.mjs --web`, and logged in `tests/am1/results/install-m4a.md` on main.

## Setup the seed needed

- The m3-ci installation held only `contents: write, metadata: read`, so it could not create check runs. Its App record and installation were given `checks`, `issues`, and `pull_requests` write, as `tests/am1/hosted/app-perms.mjs` does, but through the tools edge's `/commit` with the operator credential, because the product edge has no `/_bed/identity`.

## Cost

`INSTALL=graft-m4a python3 tests/fanout/m2bcost.py 2026-11-13T03:00:00Z 2026-11-13T13:30:00Z`: **$4.52** for the whole install that day (all threads sharing it), under the $25 cap. Containers are $3.03 of it.

Later: `INSTALL=graft-m4a python3 tests/fanout/m2bcost.py 2026-11-13T13:30:00Z 2026-11-14T08:45:00Z` is **$24.47** (containers $14.51), mostly M4 Actions' load runs on the shared install. With the earlier window's $4.52, the install has spent about $29 since 03:00Z on 2026-11-13, past the $25 cap. The lead ruled that the cap counts only this thread's own share (its edge days and BF-38 runs), as for M4 Actions, so the shared install's container hours are not in it.

## Pending

- The chrome widgets whose REST routes the edge did not serve (F61) rendered nothing. Branch `f61-reads` (2026-11-22, code only) serves the last eight: branches, tags, labels, assignees, teams, check suites, and issue and comment reactions, each validated against the pinned description; notifications were already served behind 32.19 and 0x14.3. An edge day on graft-m4a and a BF-38 rerun confirm it hosted.

## Open items

These are known limits of the live log after a relay kill. BF-61's row does not test them, so they are not failures.

- **The fallback shows a running step's lines only once that step ends.** `/logs` for a running job serves the step logs the runner has uploaded, and the runner uploads a step's log at the step's end [inference, M4 Actions]. On job 1114120 the page held steps 1 and 2 from the kill until step 3 ended.
- **The page does not rejoin the relay.** After the kill the runner reconnected its feed to a fresh relay about 27 s later [M4 Actions' runner log], but the viewer stays on the saved log, polled every 3 s, until the job ends.
