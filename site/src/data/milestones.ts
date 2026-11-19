// Every milestone in build/BUILD-PLAN.md, in the plan's order, with its state and the results that close it. The
// Status page renders all of it; the home page's status section renders the strip and the milestones under way.
// Sources: the plan's milestone status tables and work-package notes, tests/RESULTS.md, tests/am1/results/merge-p99.md,
// narrow-reads-3.md (gates 17 to 19), tests/m4/results/m4-wp1-hosted.md, m4-wp2.md, m4-wp3.md, m4-wp5.md,
// tests/m5/results/m5-wp1.md and m5-wp2.md. The plan wins wherever this file is shorter or older.
export const asOf = '19 November 2026';
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
    id: 'am1', code: 'AM1', name: 'Agent-scale safety and load', state: 'now', when: 'Features built, gate open',
    lead: 'What the host needs to carry hundreds of AI coding agents safely on one repository. The features are built and their functional tests pass. What is left is scale.',
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
      { title: 'Passing at 500 agents on one repository', items: [
        'Merges: p99 1,208 to 1,571 ms in three ten-minute runs, against a 2 s line' + row('B0-29'),
        'API reads: p99 1,339 to 1,982 ms with 100.00% of reads served and no leader change, three runs in a row, against a 2 s line' + row('B0-29'),
        'Leases, refusals for six kinds of actor, the activity feed, the trust view on eight resources, and a byte-identical GHES default' + row('L0-11, L0-13, L0-16, LF-4, LF-12, LF-13'),
        'The activity feed with the leader killed every 60 s: feed p99 2,954 ms between kills against 3 s, and no overlapping exclusive leases past the clock allowance, met at 90%' + row('L0-12'),
        '1 MiB agent pushes on a repository led from another continent, then with every bucket failing: commit p99 1,834 ms against that case\'s 2 s bound, and 1,262 of 1,262 oversized pushes refused at once' + row('B0-31'),
        'Storage on a hot repository: its metadata moved to a child group under load, with a peak of 7.74 GB against a 9.45 GB limit and no storage-full errors, on an accelerated run from the split on, met at 90%' + row('B0-30'),
      ] },
      { title: 'Still to pass', items: [
        'Commit p99 under 800 ms with 500 agents pushing' + row('B0-29'),
        'A burst of 500 pushes in one second, and 5,000 agents with human commits still under 1 s' + row('B0-32, B0-33'),
        'Git CPU for one 5 GB repository under load, with clones starting within 1 s' + row('B0-34'),
        'Test merges under 500 open pull requests, with Actions starting each run within 10 s of the push' + row('B0-35'),
      ] },
    ],
    after: 'Each piece passes on its own. The gate asks for all of it at once on one repository: 500 agents, human commits fast, no leader churn, memory under the isolate limit, and 30 days of storage.',
  },
  {
    id: 'm4', code: 'M4', name: 'Webhooks, notifications, search, Actions and the web app', state: 'now', when: 'Every test row passes, still in build',
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
      ] },
    ],
  },
  {
    id: 'm5', code: 'M5', name: 'Release control and site days', state: 'now', when: '2 of 7 work packages done',
    lead: 'How a new version reaches an install: releases the customer signs, applied one site a day with the site drained first, and a pre-signed way back for every step.',
    groups: [
      { title: 'Passed', items: [
        'The release group, its genesis and its keys: a genesis signed by another key is refused by the members themselves (13 November)' + row('BF-41'),
        'The front door and pin-only pointers: 200,416 of 200,416 requests stayed pinned through a 601 s blackhole (14 November)' + row('BF-8'),
      ] },
      { title: 'Still to come', items: [
        'The <code>graft-ops</code> executor, site days with automatic drain, the change feed and export endpoints, and the revocation and rotation runbooks',
        'The gate: a bad release commits nothing, and a vendor token revoked mid-roll returns every site to the last good version' + row('BM-5'),
      ] },
    ],
  },
  {
    id: 'am2', code: 'AM2', name: 'graft-mcp, the remote MCP server', state: 'next', when: 'After Milestone 5',
    lead: 'An MCP server at <code>mcp.&lt;host&gt;</code> with its own sign-in and a consent page per client. Its tools cover coordination and provenance, and every text field comes through the trust view.',
    groups: [{ title: 'Gate', items: [
      'The MCP row passes, and a bad <code>graft-mcp</code> at 1% swaps back with the Cloudflare API blocked' + row('LM-2'),
    ] }],
  },
  {
    id: 'am3', code: 'AM3', name: 'The controller and Graftyard-run agents', state: 'next', when: 'After Milestone 5',
    lead: 'Graftyard\'s own agents, off until an org turns them on. A controller in a separate account turns an assigned issue into a task, runs it in a Sandbox workspace whose network is closed by default, and keeps a session log for every run.',
    groups: [{ title: 'Gate', items: [
      'An issue to a merged pull request end to end, in a fork and in the repository; a Sandbox killed mid-task resumes; a bad controller at 1% swaps back' + row('LM-3'),
    ] }],
  },
  {
    id: 'm6', code: 'M6', name: 'Region loss, SSH and the lifeboat', state: 'next', when: 'Planned',
    lead: 'Replacing a lost region\'s members with every account API token revoked, opt-in SSH through Cloudflare Tunnel, and the lifeboat: a read-only copy on the customer\'s own hosts that serves clones and reads when Cloudflare is down.',
    groups: [{ title: 'Gate', items: [
      'One site fails for a day and its members are replaced with no new script version, every acknowledged write present, and <code>gh pr list</code> working against the lifeboat' + row('BM-6'),
    ] }],
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
