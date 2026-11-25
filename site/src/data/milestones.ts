// Every milestone in build/BUILD-PLAN.md, in the plan's order, with its state and the results that close it. The
// Status page renders all of it; the home page's status section renders the strip and the milestones under way.
// Sources: the plan's milestone status tables and work-package notes, tests/RESULTS.md, tests/am1/results/merge-p99.md,
// narrow-reads-3.md (gates 17 to 19), tests/m4/results/m4-wp1-hosted.md, m4-wp2.md, m4-wp3.md, m4-wp5.md,
// tests/m5/results/m5-wp1.md to m5-wp6.md, tests/am1/results/b033-closing.md, b034-mt19.md, b029-m3fmf.md,
// tests/am2/results/am2-wp1.md, am2-wp3.md, am2-wp4.md and tests/m6/results/m6-wp2.md. The plan wins wherever this file is shorter or older.
export const asOf = '25 November 2026';
const row = (id: string) => ` <span class="rid">${id}</span>`;

export type State = 'done' | 'now' | 'next';
export interface Milestone {
  id: string;
  code: string;
  name: string;
  state: State;
  when: string;
  lead: string;
  groups?: { title: string; items: string[] }[];
  after?: string;
}
export const label: Record<State, string> = { done: 'Closed', now: 'Under way', next: 'Planned' };

export const milestones: Milestone[] = [
  {
    id: 'm0', code: 'M0', name: 'Measure the platform and the protocol', state: 'done', when: 'Closed 31 October',
    lead: 'Measured Cloudflare and the replication protocol on prototype code before any product code. Every number the design depends on was tested here, and no test hit its fail line.',
    groups: [{ title: 'Rerun since on the production Rust host', items: [
      'Commit at 100 commands/s: 6 of 6 runs under 600 ms, p99 244 to 377 ms' + row('B0-01'),
      'Group commit at 300 and 330 commands/s: 16 of 16 runs under 600 ms' + row('B0-02'),
      'A gray site during another site\'s deploy: 235 of 235 groups moved within 20 s, 0 failed writes' + row('B0-07'),
    ] }],
  },
  {
    id: 'm1', code: 'M1', name: 'Groups and git', state: 'done', when: 'Closed 4 November',
    lead: 'Replicated groups across three sites, and git push, clone and fetch over HTTPS with stock git. Packs are stored in R2 with a clone cache in front, plus basic accounts and tokens.',
    groups: [{ title: 'Gate', items: [
      '348 of 348 client checks on git 2.39.5, 2.51.0 and 2.56.0, with a site failing in half the runs' + row('BM-1'),
      'A 24-hour chaos run lost 0 of 904,294 acknowledged writes' + row('BM-1'),
    ] }],
  },
  {
    id: 'm2', code: 'M2', name: 'Issues, pull requests and merge', state: 'done', when: 'Closed 7 November',
    lead: 'Issues, pull requests, reviews, check runs, merge and the merge queue, LFS objects and release assets, with REST and GraphQL that match GHES 3.22.',
    groups: [{ title: 'Gate', items: [
      'gh 2.102.1, git and git-lfs 3.8.0 pass unmodified against the hosted edge',
      'A merge on a newly elected leader: p99 1.36 s in its region and 1.83 s from another continent' + row('BM-2'),
    ] }],
  },
  {
    id: 'm3', code: 'M3', name: 'Identity and authorization', state: 'done', when: 'Closed 9 November',
    lead: 'Fine-grained tokens, OAuth and device flows, GitHub Apps with installation tokens, single sign-on with OIDC, collaborators and invitations, repository transfers, and read replicas (learners) for busy repositories.',
    groups: [{ title: 'Gate', items: [
      '200 of 200 revoked tokens refused on the next push, over HTTPS' + row('BM-3'),
      'Terraform\'s own acceptance test for collaborators, 6 of 6 runs' + row('BF-34'),
    ] }],
    after: 'After Milestone 3, <code>git</code>, <code>gh</code>, Terraform and Octokit work against Graftyard unchanged, at human scale.',
  },
  {
    id: 'am1', code: 'AM1', name: 'Agent-scale safety and load', state: 'done', when: 'Closed 24 November',
    lead: 'What the host needs to carry hundreds of AI coding agents safely on one repository. Every feature is built, and the gate passed at 500 and at 5,000 agents on one repository, with people still merging beside them.',
    groups: [
      { title: 'New features', items: [
        '<b>Agent identity.</b> The host knows which commits and requests come from an agent, which vendor and kind of agent it is, and records provenance on each push.',
        '<b>Agent policy.</b> Protected paths an agent can\'t touch, a merge gate for agent changes, and per-org switches to turn agents on or off.',
        '<b>Tasks.</b> An API under <code>/api/graftyard/v1</code> to create and track agent tasks.',
        '<b>Leases.</b> An agent claims files or refs before it edits them, so two agents don\'t collide. Pushes outside the lease are refused.',
        '<b>Activity feed.</b> Agents long-poll for repository events instead of hammering the API.',
        '<b>Trust view.</b> An opt-in field that flags hidden characters and agent authors in issue and pull request text. Default GHES responses stay byte-identical.',
      ] },
      { title: 'Load handling for hundreds of agents on one repository', items: [
        '<b>Admission lanes.</b> Human and merge-queue commands always go first. Agents get a share, and past it they are refused cleanly with "retry after" instead of slowing people down.',
        '<b>Inline small packs and log archiving,</b> so one hot repository\'s log doesn\'t grow without bound.',
        '<b>Fan-out to more git cache shards</b> under heavy push load, and budgeted test merges.',
      ] },
      { title: 'The gate, on hosted test installs', items: [
        '500 agents pushing to one repository: commit p99 756 ms against 800 ms, merges p99 1,663 ms against 2 s, every API read served, and no leader change, on the median of three runs' + row('B0-29'),
        'A burst of 500 agent pushes in one second: 495 of 500 acknowledged within 5 s or refused cleanly, met at 90%' + row('B0-32'),
        '5,000 agents on one repository: 115 agent commands/s admitted against 110, human commits at p99 1,372 ms against 3.9 s, and no leader change, on the median of three runs' + row('B0-33'),
        'One 5 GB repository under that load with a warm site killed, then a hostile owner: every clone started within 5 s, human pushes p99 2.6 s against 3 s, the owner held to its share, and 0 Cloudflare API calls' + row('B0-34'),
        '500 open pull requests with the base moving 10 times a minute: Actions starts each run at p99 7.2 s against 10 s, with test merges inside their budget' + row('B0-35'),
        '1 MiB agent pushes on a repository led from another continent, then with every bucket failing: commit p99 1,834 ms against that case\'s 2 s bound, and 1,262 of 1,262 oversized pushes refused at once' + row('B0-31'),
        'The activity feed with the leader killed every 60 s: feed p99 2,954 ms between kills against 3 s, and no overlapping exclusive leases, met at 90%' + row('L0-12'),
        'Storage on a hot repository: its metadata moved to a child group under load, peaking at 7.74 GB against a 9.45 GB limit with no storage-full errors, met at 90%' + row('B0-30'),
        'Leases, refusals for six kinds of actor, the trust view on eight resources, and a byte-identical GHES default' + row('L0-11, L0-13, L0-16, LF-4, LF-12, LF-13'),
      ] },
    ],
    after: 'Three rows, B0-32, B0-33 and B0-34, are met on the lead\'s rulings, which the operator accepted by closing AM1. Clauses that need Graftyard\'s own agents move to AM3, and fleets this size on one repository stay off until metadata child groups pass (M7-WP4).',
  },
  {
    id: 'm4', code: 'M4', name: 'Webhooks, notifications, search, Actions and the web app', state: 'now', when: 'Every test row passes; held open for cache, artifacts and OIDC',
    lead: 'Everything that reads the committed log: webhooks, notifications and search, the Actions engine and its runners, and the web app.',
    groups: [
      { title: 'Passing on a hosted test install', items: [
        'Webhooks: 12,325 of 12,325 deliveries arrived after a site was killed, none under a new GUID, and healthy hooks kept p99 1.07 s beside one timing out' + row('BF-57, BF-58'),
        'Notifications and search: an inbox rebuilt with read state kept, 10,000 stars listed, and EU documents kept in EU shards' + row('BF-5, BF-67, BF-68'),
        'Actions with an unmodified <code>actions/runner</code>: a killed runner\'s job fails at its lease end, required checks conclude after the commit path is killed, and secrets survive two key rotations while a fork pull request sees none' + row('BF-4, BF-56, BF-59'),
        'Merge queue workflows: a <code>merge_group</code> run on each queue ref built, and each ref deleted 10 s after its pull request landed or was ejected' + row('BF-37'),
        'A site killed during a 256-job matrix: runners polled the new leader\'s site 11.5 s later, and all 256 jobs ran exactly once' + row('BF-55'),
        'The web app under Playwright: every page renders with its first data in the HTML, and the live job log streams within 2 s and falls back when its relay dies' + row('BF-38, BF-61'),
        '2,500 runners polling one broker past its cap: 512 of 512 jobs succeeded, starting at p99 16.6 s against a 35 s line' + row('BF-54'),
        'Hosted runners on Cloudflare Sandboxes: 50 of 50 jobs from a cold start, never more than 20 at once under the per-owner cap, and no Sandbox left behind' + row('BF-60'),
        'The gate: an unmodified <code>actions/runner</code> ran a 2 by 4 matrix with a site taken down mid-run, and every leg finished exactly once' + row('BM-4'),
      ] },
      { title: 'Still in build', items: [
        'The launch service, so a step can call another action with <code>uses:</code>, and each job\'s own <code>GITHUB_TOKEN</code>. Both are built and ship with the next site day',
        'What holds M4 open: <code>actions/cache</code>, <code>upload-artifact</code> and <code>download-artifact</code>, and OIDC tokens for cloud sign-in. Each gets its own test row, and the cache is being built now',
      ] },
    ],
  },
  {
    id: 'm5', code: 'M5', name: 'Release control and site days', state: 'done', when: 'Closed 25 November',
    lead: 'How a new version reaches an install: releases the customer signs, applied one site a day with the site drained first, and a pre-signed way back for every step.',
    groups: [
      { title: 'Passed on hosted test installs', items: [
        'The release group, its genesis and its keys: a genesis signed by another key is refused by the members themselves' + row('BF-41'),
        'The front door and pin-only pointers: 200,416 of 200,416 requests stayed pinned through a 601 s blackhole' + row('BF-8'),
        'The <code>graft-ops</code> executor runs every step: a rollback that can\'t read an enabled format is refused and pages, and with the Cloudflare API blocked a bad edge swaps back in 5.6 s' + row('BF-7, BF-43'),
        '<code>graft-ops</code> killed 134 times mid-rollout: every step committed once and in order, with one finish per intent' + row('BF-39'),
        'A whole release roll in order, with the vault upgraded under its own token and never an install token; the vault pulls every export endpoint, and a revoked export token is refused within 10 s' + row('BF-47, BF-49'),
        'The deploy, read and mint keys rotated under live traffic with no new script version, and 235 of 235 mints answered' + row('BF-46'),
        'Gate: a deliberately unreadable format sent to one site commits nothing, and its groups move to another site' + row('BM-5'),
        'Gate: with the vendor\'s token revoked mid-roll, the customer halts, removes the vendor key and aborts. Every site returns to the version it ran before, and a vendor-signed intent is refused' + row('BM-5'),
        'Gate: a bad <code>graft-ops</code> at 1% swaps back by its pre-signed pointer with the Cloudflare API blocked, after about 700 token mints, and returns to the last good version once unblocked' + row('BM-5'),
        'The SSH host keys, the MCP and Insights App keys, the MCP client secret and the secrets wrap key rotated under live traffic with no new script version: 0 failed SSH sessions in 47 rounds, 0 bad hook signatures, and 16 of 16 MCP logins through the switch' + row('BF-46'),
      ] },
    ],
  },
  {
    id: 'am2', code: 'AM2', name: 'graft-mcp, the remote MCP server', state: 'done', when: 'Closed 24 November',
    lead: 'An MCP server at <code>mcp.&lt;host&gt;</code> with its own sign-in and a consent page per client. Its tools cover coordination and provenance, and every text field comes through the trust view.',
    groups: [
      { title: 'The gate, on hosted test installs', items: [
        'Four MCP SDK clients, 30 of 30 calls each, with tool overhead p50 14 to 45 ms against 50 ms; 100 of 100 wrong-audience tokens refused, and a refreshed token keeps its scope' + row('L0-08, LF-6'),
        'A bad <code>graft-mcp</code> at 1% with the deploy token invalid: <code>graft-ops</code> pinned the last good version, and 119,218 of 119,218 sampled answers came from it' + row('LM-2'),
        'An approval through MCP, or by the agent\'s own requester, never satisfies the agent merge gate: 42 of 42 such merges refused' + row('L0-15'),
      ] },
    ],
    after: 'One known limit stays open: the swap-back is judged on too few requests at 1% (R47). Its fix, a held and judged 100% step, is built and gets its hosted read on the next MCP release.',
  },
  {
    id: 'am3', code: 'AM3', name: 'The controller and Graftyard-run agents', state: 'next', when: 'First pieces built',
    lead: 'Graftyard\'s own agents, off until an org turns them on. A controller in a separate account turns an assigned issue into a task, runs it in a Sandbox workspace whose network is closed by default, and keeps a session log for every run.',
    groups: [
      { title: 'Built so far, in memory', items: [
        'The agents release view and its refusals, and the controller\'s task and workspace rules as a pure state machine. Test installs use the runner account as the agents account until go-live',
      ] },
      { title: 'Gate', items: [
      'An issue to a merged pull request end to end, in a fork and in the repository; a Sandbox killed mid-task resumes; a bad controller at 1% swaps back' + row('LM-3'),
    ] },
    ],
  },
  {
    id: 'm6', code: 'M6', name: 'Region loss, SSH and the lifeboat', state: 'next', when: 'Parts built early',
    lead: 'Replacing a lost region\'s members with every account API token revoked, opt-in native SSH on port 22 through Cloudflare Spectrum, and the lifeboat: a read-only copy on the customer\'s own hosts that serves clones and reads when Cloudflare is down.',
    groups: [
      { title: 'Started early', items: [
        'Stock <code>git clone</code> and <code>git push</code> over SSH through Spectrum on a Pro zone, a removed key refused, and a session held 10 minutes' + row('B0-24d'),
        'SSH host keys rotated with clients learning the new set, and 20 of 20 SSH pushes from a suspended user refused' + row('BF-51, BF-52'),
        'Region-loss replacement and the lifeboat\'s learners, built and tested in code',
      ] },
      { title: 'Gate', items: [
      'One site fails for a day and its members are replaced with no new script version, every acknowledged write present, and <code>gh pr list</code> working against the lifeboat' + row('BM-6'),
    ] },
    ],
  },
  {
    id: 'am4', code: 'AM4', name: 'Insights, the predictor and Code Mode', state: 'next', when: 'After Milestone 6',
    lead: 'Conflict prediction between agents\' branches before a pull request exists, insights behind an agents page, and Code Mode behind an org switch.',
    groups: [{ title: 'Gate', items: [
      'Every textual conflict on current tips found, with at most 5% false conflicts' + row('LM-4'),
    ] }],
  },
  {
    id: 'm7', code: 'M7', name: 'Breadth and modes', state: 'next', when: 'Planned, one package started early',
    lead: 'Self-hosted installs and installs in the customer\'s own Cloudflare account, managed cell accounts, metadata child groups for very large repositories, a single-site evaluation mode, import from GHES, and whole-install restore.',
    groups: [
      { title: 'Started early', items: ['Metadata child groups, built early for AM1\'s storage run' + row('M7-WP4')] },
      { title: 'Gate', items: ['A self-hosted install with no vendor key passes the gates of Milestones 1 to 6, and two restores into new accounts lose no entry up to their cutoff' + row('BM-7')] },
    ],
  },
];
export const count = (s: State) => milestones.filter((m) => m.state === s).length;
