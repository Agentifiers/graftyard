import type { Schemas } from "@/api/client";
import {
  checkRollup,
  evaluate,
  linkedIssues,
  MERGEABLE,
  verdicts,
  type Requirements,
} from "@/areas/pulls/merge-rules";
import { hunkUpTo } from "@/areas/pulls/patch";
import type {
  DraftComment,
  MergeMethod,
  MergeRequirements,
  PullEvent,
  PullSummary,
  ThreadState,
} from "@/areas/pulls/ui-api";
import {
  checkRun,
  db,
  issueForPull,
  pullRequest,
  type CheckRun,
  type DiffEntry,
  type PullRequest,
  type RepoState,
} from "./core-db";
import { plans, raftPull, type CommitPlan } from "./pulls-seed";
import {
  API,
  branchSha,
  edge,
  HOST,
  id,
  raft,
  sha,
  teams,
  users,
  type Repository,
  type SimpleUser,
} from "../world";

// The pull request area's own state: each repository's commit graph, reviews, review
// comments, thread resolution, viewed files, PR timeline events, branch protection, and
// the timers that move check runs along. Issues, pulls, files, and check runs themselves
// stay in core-db.ts, so the issue list and the issue page see merges and new PRs.

type Commit = Schemas["commit"];
type Review = Schemas["pull-request-review"];
type ReviewComment = Schemas["pull-request-review-comment"];
type Team = Schemas["team"];

export const viewer = users.mira;

// ---- Commit graph ----

type Node = { sha: string; commit: Commit; files: DiffEntry[]; author: SimpleUser };
type Branch = { name: string; history: Node[]; conflicts: boolean };

const graphs = new Map<string, Map<string, Branch>>();

function node(
  full: string,
  shaValue: string,
  parent: string | undefined,
  plan: { message: string; author: SimpleUser; date: string },
  files: DiffEntry[],
  extraParent?: string,
): Node {
  const base = `${API}/repos/${full}`;
  const who = {
    name: plan.author.name ?? plan.author.login,
    email: `${plan.author.login}@graftyard.test`,
    date: plan.date,
  };
  const parents = [parent, extraParent].filter((p): p is string => Boolean(p));
  const additions = files.reduce((n, f) => n + f.additions, 0);
  const deletions = files.reduce((n, f) => n + f.deletions, 0);
  return {
    sha: shaValue,
    files,
    author: plan.author,
    commit: {
      url: `${base}/commits/${shaValue}`,
      sha: shaValue,
      node_id: `C_${shaValue.slice(0, 12)}`,
      html_url: `${HOST}/${full}/commit/${shaValue}`,
      comments_url: `${base}/commits/${shaValue}/comments`,
      commit: {
        url: `${base}/git/commits/${shaValue}`,
        author: who,
        committer: who,
        message: plan.message,
        comment_count: 0,
        tree: { sha: sha(`tree:${shaValue}`), url: `${base}/git/trees/${sha(`tree:${shaValue}`)}` },
        verification: {
          verified: plan.author.login === "mira",
          reason: plan.author.login === "mira" ? "valid" : "unsigned",
          payload: null,
          signature: null,
          verified_at: plan.author.login === "mira" ? plan.date : null,
        },
      },
      author: plan.author,
      committer: plan.author,
      parents: parents.map((p) => ({
        sha: p,
        url: `${base}/commits/${p}`,
        html_url: `${HOST}/${full}/commit/${p}`,
      })),
      stats: { additions, deletions, total: additions + deletions },
    },
  };
}

let nodeSeq = 0;
const freshSha = (full: string, branch: string) => sha(`${full}:${branch}:new:${nodeSeq++}`);

function buildGraphs() {
  for (const plan of plans) {
    const full = plan.repo.full_name;
    const state = db.state(plan.repo.owner.login, plan.repo.name)!;
    const graph = new Map<string, Branch>();
    const main: Node[] = [];
    plan.main.forEach((c, i) => {
      const prNumber = /\(#(\d+)\)$/.exec(c.message)?.[1];
      const shaValue =
        i === plan.main.length - 1
          ? branchSha(full, "main")
          : prNumber
            ? sha(`${full}:merge:${prNumber}`)
            : sha(`${full}:main:${i}`);
      const [path, patch] = c.patch;
      const f: DiffEntry = {
        sha: sha(`${full}:main:${path}:${i}`),
        filename: path,
        status: patch.startsWith("@@ -0,0") ? "added" : "modified",
        additions: patch.split("\n").filter((l) => l.startsWith("+")).length,
        deletions: patch.split("\n").filter((l) => l.startsWith("-")).length,
        changes: 0,
        blob_url: `${HOST}/${full}/blob/main/${path}`,
        raw_url: `${HOST}/${full}/raw/main/${path}`,
        contents_url: `${API}/repos/${full}/contents/${path}?ref=main`,
        patch,
      };
      f.changes = f.additions + f.deletions;
      main.push(node(full, shaValue, main.at(-1)?.sha, c, [f]));
    });
    graph.set("main", { name: "main", history: main, conflicts: false });

    for (const [name, b] of Object.entries(plan.branches)) {
      const prFiles = state.pulls.find((p) => p.head.ref === name);
      const files = b.files ?? (prFiles && state.files.get(prFiles.number)) ?? [];
      const history = main.slice(0, b.fork);
      b.commits.forEach((c: CommitPlan, i) => {
        const shaValue =
          i === b.commits.length - 1 ? branchSha(full, name) : sha(`${full}:${name}:${i}`);
        const own = files.filter((f) => c.files.includes(f.filename));
        history.push(node(full, shaValue, history.at(-1)?.sha, c, own));
      });
      graph.set(name, { name, history, conflicts: b.conflicts ?? false });
    }
    graphs.set(full.toLowerCase(), graph);
  }
}

export function graph(full: string) {
  let g = graphs.get(full.toLowerCase());
  if (!g) {
    // A repository another area created: one empty main branch.
    g = new Map([["main", { name: "main", history: [], conflicts: false }]]);
    graphs.set(full.toLowerCase(), g);
  }
  return g;
}

const tip = (b: Branch) => b.history.at(-1);

// A ref is a branch name, `owner:branch`, or the sha of any commit in the graph.
export function resolveRef(full: string, ref: string): Branch | undefined {
  const g = graph(full);
  const name = ref.includes(":") ? ref.slice(ref.indexOf(":") + 1) : ref;
  const branch = g.get(name);
  if (branch) return branch;
  if (!/^[0-9a-f]{7,40}$/.test(name)) return undefined;
  for (const b of g.values()) {
    const i = b.history.findIndex((n) => n.sha.startsWith(name));
    if (i >= 0) return { name, history: b.history.slice(0, i + 1), conflicts: false };
  }
  return undefined;
}

export function compareBranches(base: Branch, head: Branch) {
  let i = 0;
  while (i < base.history.length && i < head.history.length && base.history[i] === head.history[i])
    i++;
  const ahead = head.history.slice(i);
  const behind = base.history.slice(i);
  const files = new Map<string, DiffEntry>();
  for (const n of ahead) for (const f of n.files) files.set(f.filename, f);
  return { mergeBase: base.history[i - 1], ahead, behind, files: [...files.values()] };
}

export function comparison(
  state: RepoState,
  base: Branch,
  head: Branch,
): Schemas["commit-comparison"] {
  const full = state.repo.full_name;
  const c = compareBranches(base, head);
  const status =
    c.ahead.length && c.behind.length
      ? "diverged"
      : c.ahead.length
        ? "ahead"
        : c.behind.length
          ? "behind"
          : "identical";
  const empty = node(
    full,
    sha(`${full}:empty`),
    undefined,
    { message: "", author: users.org, date: state.repo.created_at ?? "" },
    [],
  ).commit;
  const range = `${base.name}...${head.name}`;
  return {
    url: `${API}/repos/${full}/compare/${range}`,
    html_url: `${HOST}/${full}/compare/${range}`,
    permalink_url: `${HOST}/${full}/compare/${tip(base)?.sha ?? ""}...${tip(head)?.sha ?? ""}`,
    diff_url: `${HOST}/${full}/compare/${range}.diff`,
    patch_url: `${HOST}/${full}/compare/${range}.patch`,
    base_commit: tip(base)?.commit ?? empty,
    merge_base_commit: c.mergeBase?.commit ?? empty,
    status,
    ahead_by: c.ahead.length,
    behind_by: c.behind.length,
    total_commits: c.ahead.length,
    commits: c.ahead.map((n) => n.commit),
    files: c.files,
  };
}

// GHES nests this shape in check suites and combined statuses.
export const minimalRepo = (repo: Repository): Schemas["minimal-repository"] => ({
  id: repo.id,
  node_id: repo.node_id,
  name: repo.name,
  full_name: repo.full_name,
  owner: repo.owner,
  private: repo.private,
  html_url: repo.html_url,
  description: repo.description,
  fork: repo.fork,
  url: repo.url,
  archive_url: repo.archive_url,
  assignees_url: repo.assignees_url,
  blobs_url: repo.blobs_url,
  branches_url: repo.branches_url,
  collaborators_url: repo.collaborators_url,
  comments_url: repo.comments_url,
  commits_url: repo.commits_url,
  compare_url: repo.compare_url,
  contents_url: repo.contents_url,
  contributors_url: repo.contributors_url,
  deployments_url: repo.deployments_url,
  downloads_url: repo.downloads_url,
  events_url: repo.events_url,
  forks_url: repo.forks_url,
  git_commits_url: repo.git_commits_url,
  git_refs_url: repo.git_refs_url,
  git_tags_url: repo.git_tags_url,
  issue_comment_url: repo.issue_comment_url,
  issue_events_url: repo.issue_events_url,
  issues_url: repo.issues_url,
  keys_url: repo.keys_url,
  labels_url: repo.labels_url,
  languages_url: repo.languages_url,
  merges_url: repo.merges_url,
  milestones_url: repo.milestones_url,
  notifications_url: repo.notifications_url,
  pulls_url: repo.pulls_url,
  releases_url: repo.releases_url,
  stargazers_url: repo.stargazers_url,
  statuses_url: repo.statuses_url,
  subscribers_url: repo.subscribers_url,
  subscription_url: repo.subscription_url,
  tags_url: repo.tags_url,
  teams_url: repo.teams_url,
  trees_url: repo.trees_url,
  hooks_url: repo.hooks_url,
});

// ---- Branch protection (repo-admin edits it; the merge rules read it) ----

const protection = new Map<string, Requirements>([
  [
    "graftyard/edge:main",
    { required_checks: ["test (ubuntu-latest)", "rustfmt"], required_approvals: 1, strict: false },
  ],
  [
    "graftyard/raft:main",
    { required_checks: ["test (ubuntu-latest)"], required_approvals: 1, strict: true },
  ],
]);
const NONE: Requirements = { required_checks: [], required_approvals: 0, strict: false };

export const requirementsFor = (full: string, branch: string) =>
  protection.get(`${full.toLowerCase()}:${branch}`);

// ---- Reviews, review comments, threads, viewed files, events ----

type PullExtra = {
  reviews: Review[];
  comments: ReviewComment[];
  threads: Map<number, ThreadState>;
  viewed: Map<string, Set<string>>;
  events: PullEvent[];
  requestedTeams: Team[];
  // The commits as they were when the PR merged or lost its head branch.
  snapshot?: Commit[];
};

const extras = new Map<string, PullExtra>();

export function extra(state: RepoState, number: number): PullExtra {
  const key = `${state.repo.full_name.toLowerCase()}#${number}`;
  let e = extras.get(key);
  if (!e) {
    e = {
      reviews: [],
      comments: [],
      threads: new Map(),
      viewed: new Map(),
      events: [],
      requestedTeams: [],
    };
    extras.set(key, e);
  }
  return e;
}

const association = (u: SimpleUser): "OWNER" | "MEMBER" =>
  u.login === "mira" ? "OWNER" : "MEMBER";

export function makeReview(
  pr: PullRequest,
  user: SimpleUser,
  state: string,
  body: string,
  at: string | undefined,
): Review {
  const n = id();
  const html = `${pr.html_url}#pullrequestreview-${n}`;
  return {
    id: n,
    node_id: `PRR_${sha(`${n}`).slice(0, 12)}`,
    user,
    body,
    state,
    html_url: html,
    pull_request_url: pr.url,
    _links: { html: { href: html }, pull_request: { href: pr.url } },
    ...(at ? { submitted_at: at } : {}),
    commit_id: pr.head.sha,
    author_association: association(user),
  };
}

export function makeComment(
  state: RepoState,
  pr: PullRequest,
  user: SimpleUser,
  reviewId: number | null,
  draft: DraftComment & { in_reply_to_id?: number },
  at: string,
): ReviewComment {
  const n = id();
  const patch = state.files.get(pr.number)?.find((f) => f.filename === draft.path)?.patch ?? "";
  const url = `${state.repo.url}/pulls/comments/${n}`;
  const html = `${pr.html_url}#discussion_r${n}`;
  return {
    url,
    pull_request_review_id: reviewId,
    id: n,
    node_id: `PRRC_${sha(`${n}`).slice(0, 12)}`,
    diff_hunk: hunkUpTo(patch, { side: draft.side, line: draft.line }) ?? "",
    path: draft.path,
    commit_id: pr.head.sha,
    original_commit_id: pr.head.sha,
    ...(draft.in_reply_to_id ? { in_reply_to_id: draft.in_reply_to_id } : {}),
    user,
    body: draft.body,
    created_at: at,
    updated_at: at,
    html_url: html,
    pull_request_url: pr.url,
    author_association: association(user),
    _links: { self: { href: url }, html: { href: html }, pull_request: { href: pr.url } },
    start_line: draft.start_line ?? null,
    original_start_line: draft.start_line ?? null,
    start_side: draft.start_line ? (draft.start_side ?? draft.side) : null,
    line: draft.line,
    original_line: draft.line,
    side: draft.side,
    subject_type: "line",
  };
}

export function addEvent(state: RepoState, number: number, e: PullEvent) {
  extra(state, number).events.push(e);
}

export const eventBase = (actor: SimpleUser = viewer) => ({
  id: id(),
  actor,
  created_at: new Date().toISOString(),
});

export function commitsOf(state: RepoState, pr: PullRequest): Commit[] {
  const e = extra(state, pr.number);
  if (e.snapshot) return e.snapshot;
  const g = graph(state.repo.full_name);
  const head = g.get(pr.head.ref);
  const base = g.get(pr.base.ref);
  return head && base ? compareBranches(base, head).ahead.map((n) => n.commit) : [];
}

// ---- Derived PR fields: head and base shas, size, reviewers, mergeability ----

export function sync(state: RepoState, pr: PullRequest) {
  const full = state.repo.full_name;
  const g = graph(full);
  const head = g.get(pr.head.ref);
  const base = g.get(pr.base.ref);
  const e = extra(state, pr.number);
  if (head && tip(head)) pr.head.sha = tip(head)!.sha;
  if (base && tip(base) && !pr.merged) pr.base.sha = tip(base)!.sha;
  pr.review_comments = e.comments.filter((c) => !isPending(e, c)).length;
  // Labels and assignees live on the issue, which the issues area edits.
  const issue = issueOf(state, pr.number);
  if (issue) {
    pr.labels = issue.labels.flatMap((l) =>
      typeof l === "object" && l.name && l.id
        ? [
            {
              id: l.id,
              node_id: l.node_id ?? "",
              url: l.url ?? "",
              name: l.name,
              description: l.description ?? "",
              color: l.color ?? "8b949e",
              default: l.default ?? false,
              archived_by: l.archived_by ?? null,
            },
          ]
        : [],
    );
    pr.assignees = issue.assignees ?? [];
    pr.assignee = issue.assignee;
    pr.title = issue.title;
    pr.body = issue.body ?? null;
  }
  pr.requested_teams = e.requestedTeams;
  if (pr.state !== "open") {
    pr.mergeable = null;
    pr.rebaseable = null;
    pr.mergeable_state = "unknown";
    return;
  }
  const cmp = head && base ? compareBranches(base, head) : undefined;
  if (cmp) pr.commits = Math.max(1, cmp.ahead.length);
  const conflicts = Boolean(head?.conflicts);
  const ev = evaluate({
    draft: pr.draft ?? false,
    runs: db.checkRuns(pr.head.sha),
    reviews: e.reviews,
    requirements: requirementsFor(full, pr.base.ref) ?? NONE,
    behind_by: cmp?.behind.length ?? 0,
    conflicts,
  });
  pr.mergeable = !conflicts;
  pr.rebaseable = !conflicts;
  pr.mergeable_state = head ? ev.mergeable_state : "unknown";
}

const isPending = (e: PullExtra, c: ReviewComment) =>
  e.reviews.some((r) => r.id === c.pull_request_review_id && r.state === "PENDING");

export function visibleComments(e: PullExtra) {
  return e.comments.filter((c) => !isPending(e, c));
}

export function summary(state: RepoState, pr: PullRequest): PullSummary {
  sync(state, pr);
  const e = extra(state, pr.number);
  const required = requirementsFor(state.repo.full_name, pr.base.ref)?.required_approvals ?? 0;
  const states = [...verdicts(e.reviews).values()];
  const approvals = states.filter((s) => s === "APPROVED").length;
  return {
    number: pr.number,
    review_decision: states.includes("CHANGES_REQUESTED")
      ? "CHANGES_REQUESTED"
      : approvals >= Math.max(1, required)
        ? "APPROVED"
        : required > 0 || pr.requested_reviewers?.length || e.requestedTeams.length
          ? "REVIEW_REQUIRED"
          : null,
    approvals,
    review_comments: pr.review_comments,
    checks: checkRollup(db.checkRuns(pr.head.sha)),
  };
}

export function requirementsView(state: RepoState, pr: PullRequest): MergeRequirements {
  sync(state, pr);
  const g = graph(state.repo.full_name);
  const head = g.get(pr.head.ref);
  const base = g.get(pr.base.ref);
  const cmp = head && base ? compareBranches(base, head) : undefined;
  const req = requirementsFor(state.repo.full_name, pr.base.ref) ?? NONE;
  const ev = evaluate({
    draft: pr.draft ?? false,
    runs: db.checkRuns(pr.head.sha),
    reviews: extra(state, pr.number).reviews,
    requirements: req,
    behind_by: cmp?.behind.length ?? 0,
    conflicts: Boolean(head?.conflicts),
  });
  return {
    ...ev,
    mergeable_state: pr.state === "open" ? ev.mergeable_state : "unknown",
    ...req,
    behind_by: cmp?.behind.length ?? 0,
    head_exists: Boolean(head),
  };
}

// ---- Check runs that move on their own ----

// Anything that can satisfy a merge requirement re-checks auto-merge.
function changed() {
  autoMerge();
}

export function simulate(
  run: CheckRun,
  outcome: CheckRun["conclusion"],
  summary: string,
  ms = [1500, 6000],
) {
  run.status = "queued";
  run.conclusion = null;
  run.completed_at = null;
  run.started_at = new Date().toISOString();
  run.output = {
    ...run.output,
    title: "Queued",
    summary: "Waiting for a runner",
    text: null,
    annotations_count: 0,
  };
  annotations.delete(run.id);
  setTimeout(() => {
    run.status = "in_progress";
    run.output = { ...run.output, title: "In progress", summary: "Running" };
    changed();
  }, ms[0]);
  setTimeout(() => {
    run.status = "completed";
    run.conclusion = outcome;
    run.completed_at = new Date().toISOString();
    run.output = { ...run.output, title: summary, summary };
    changed();
  }, ms[1]);
}

// New commits on a head start a fresh set of runs, as a push would.
export function startRuns(state: RepoState, headSha: string, names: string[]) {
  const suite = id();
  for (const name of names) {
    const now = new Date().toISOString();
    const run = checkRun(
      state.repo,
      headSha,
      name,
      "queued",
      null,
      now,
      null,
      "Waiting for a runner",
    );
    run.check_suite = { id: suite };
    db.addCheckRun(headSha, run);
    simulate(run, "success", name.startsWith("test") ? "318 tests passed" : "No problems found", [
      1000 + Math.random() * 1500,
      5000 + Math.random() * 5000,
    ]);
  }
}

export const annotations = new Map<number, Schemas["check-annotation"][]>();

// ---- Writes the handlers share ----

export function nextNumber(state: RepoState) {
  return Math.max(0, ...state.issues.map((i) => i.number), ...state.pulls.map((p) => p.number)) + 1;
}

export function createPull(
  state: RepoState,
  input: { title: string; body: string; head: string; base: string; draft: boolean },
): PullRequest {
  const full = state.repo.full_name;
  const g = graph(full);
  const cmp = compareBranches(g.get(input.base)!, g.get(input.head)!);
  const now = new Date().toISOString();
  const seed = {
    number: nextNumber(state),
    title: input.title,
    body: input.body,
    author: viewer,
    head: input.head,
    draft: input.draft,
    created: now,
    updated: now,
    files: cmp.files,
  };
  const pr = pullRequest(state.repo, seed);
  retarget(pr, input.base);
  pr.head.sha = tip(g.get(input.head)!)!.sha;
  state.pulls.push(pr);
  state.issues.push(issueForPull(state.repo, seed, pr));
  state.files.set(pr.number, cmp.files);
  state.repo.open_issues_count = state.repo.open_issues = state.issues.filter(
    (i) => i.state === "open",
  ).length;
  if (db.checkRuns(pr.head.sha).length === 0)
    startRuns(
      state,
      pr.head.sha,
      requirementsFor(full, input.base)?.required_checks.length
        ? requirementsFor(full, input.base)!.required_checks
        : ["test (ubuntu-latest)"],
    );
  sync(state, pr);
  return pr;
}

export function retarget(pr: PullRequest, base: string) {
  pr.base = { ...pr.base, ref: base, label: `${pr.base.user.login}:${base}` };
}

export function issueOf(state: RepoState, number: number) {
  return state.issues.find((i) => i.number === number);
}

export function touch(state: RepoState, pr: PullRequest, at = new Date().toISOString()) {
  pr.updated_at = at;
  const issue = issueOf(state, pr.number);
  if (issue) issue.updated_at = at;
}

export function setOpen(state: RepoState, pr: PullRequest, open: boolean) {
  const now = new Date().toISOString();
  pr.state = open ? "open" : "closed";
  pr.closed_at = open ? null : now;
  const issue = issueOf(state, pr.number);
  if (issue) {
    issue.state = pr.state;
    issue.closed_at = pr.closed_at;
    issue.state_reason = open ? "reopened" : null;
  }
  if (!open) pr.auto_merge = null;
  state.repo.open_issues_count = state.repo.open_issues = state.issues.filter(
    (i) => i.state === "open",
  ).length;
  touch(state, pr, now);
  addEvent(state, pr.number, { ...eventBase(), event: open ? "reopened" : "closed" });
}

export type MergeResult =
  | { ok: true; sha: string }
  | { ok: false; status: 405 | 409; message: string };

export function merge(
  state: RepoState,
  pr: PullRequest,
  input: { method: MergeMethod; title?: string; message?: string; sha?: string },
  actor: SimpleUser = viewer,
): MergeResult {
  sync(state, pr);
  if (pr.merged) return { ok: false, status: 405, message: "Pull Request is not mergeable" };
  if (input.sha && input.sha !== pr.head.sha)
    return {
      ok: false,
      status: 409,
      message: "Head branch was modified. Review and try the merge again.",
    };
  if (!MERGEABLE.has(pr.mergeable_state))
    return {
      ok: false,
      status: 405,
      message: `Pull Request is not mergeable (${pr.mergeable_state})`,
    };
  const allowed = {
    merge: state.repo.allow_merge_commit,
    squash: state.repo.allow_squash_merge,
    rebase: state.repo.allow_rebase_merge,
  };
  if (!allowed[input.method])
    return {
      ok: false,
      status: 405,
      message: `${input.method} merges are not allowed on this repository`,
    };

  const full = state.repo.full_name;
  const g = graph(full);
  const base = g.get(pr.base.ref)!;
  const head = g.get(pr.head.ref)!;
  const cmp = compareBranches(base, head);
  const now = new Date().toISOString();
  const plan = { author: actor, date: now };
  const title =
    input.title ||
    (input.method === "squash"
      ? `${pr.title} (#${pr.number})`
      : `Merge pull request #${pr.number} from ${pr.head.label.replace(":", "/")}`);
  const message =
    input.message ??
    (input.method === "squash"
      ? cmp.ahead.map((n) => `* ${n.commit.commit.message.split("\n")[0]}`).join("\n")
      : pr.title);
  extra(state, pr.number).snapshot = cmp.ahead.map((n) => n.commit);
  const parent = tip(base)?.sha;
  let result: Node;
  if (input.method === "squash") {
    result = node(
      full,
      freshSha(full, pr.base.ref),
      parent,
      { ...plan, message: `${title}\n\n${message}` },
      cmp.files,
    );
    base.history = [...base.history, result];
  } else if (input.method === "rebase") {
    let prev = parent;
    const copies = cmp.ahead.map((n) => {
      const c = node(
        full,
        freshSha(full, pr.base.ref),
        prev,
        { message: n.commit.commit.message, author: n.author, date: now },
        n.files,
      );
      prev = c.sha;
      return c;
    });
    base.history = [...base.history, ...copies];
    result = copies.at(-1)!;
  } else {
    result = node(
      full,
      freshSha(full, pr.base.ref),
      parent,
      { ...plan, message: `${title}\n\n${message}` },
      [],
      tip(head)?.sha,
    );
    base.history = [...base.history, ...cmp.ahead, result];
  }

  pr.merged = true;
  pr.merged_at = now;
  pr.merged_by = actor;
  pr.merge_commit_sha = result.sha;
  pr.auto_merge = null;
  pr.state = "closed";
  pr.closed_at = now;
  touch(state, pr, now);
  const issue = issueOf(state, pr.number);
  if (issue) {
    issue.state = "closed";
    issue.state_reason = "completed";
    issue.closed_at = now;
    if (issue.pull_request) issue.pull_request.merged_at = now;
  }
  for (const n of linkedIssues(pr.body)) {
    const linked = issueOf(state, n);
    if (linked && !linked.pull_request && linked.state === "open") {
      linked.state = "closed";
      linked.state_reason = "completed";
      linked.closed_at = now;
      linked.closed_by = actor;
      linked.updated_at = now;
    }
  }
  state.repo.open_issues_count = state.repo.open_issues = state.issues.filter(
    (i) => i.state === "open",
  ).length;
  addEvent(state, pr.number, {
    ...eventBase(actor),
    event: "merged",
    commit_id: result.sha,
    merge_method: input.method,
    base_ref: pr.base.ref,
  });
  if (state.repo.delete_branch_on_merge) deleteBranch(state, pr.head.ref, actor);
  sync(state, pr);
  changed();
  return { ok: true, sha: result.sha };
}

// Deletes a branch, and notes it on every pull request whose head it was.
export function deleteBranch(state: RepoState, name: string, actor: SimpleUser = viewer) {
  const g = graph(state.repo.full_name);
  if (name === state.repo.default_branch || !g.has(name)) return false;
  deleted.set(`${state.repo.full_name.toLowerCase()}:${name}`, g.get(name)!);
  for (const pr of state.pulls.filter((p) => p.head.ref === name)) {
    const e = extra(state, pr.number);
    e.snapshot ??= commitsOf(state, pr);
    addEvent(state, pr.number, { ...eventBase(actor), event: "head_ref_deleted", ref: name });
  }
  g.delete(name);
  return true;
}

const deleted = new Map<string, Branch>();

export function restoreBranch(state: RepoState, name: string, at: string) {
  const g = graph(state.repo.full_name);
  const key = `${state.repo.full_name.toLowerCase()}:${name}`;
  const old = deleted.get(key);
  const found = old && tip(old)?.sha === at ? old : undefined;
  const branch: Branch = found ?? {
    name,
    history: resolveRef(state.repo.full_name, at)?.history ?? [],
    conflicts: false,
  };
  g.set(name, { ...branch, name });
  deleted.delete(key);
  for (const pr of state.pulls.filter((p) => p.head.ref === name)) {
    if (!pr.merged) delete extra(state, pr.number).snapshot;
    addEvent(state, pr.number, { ...eventBase(), event: "head_ref_restored", ref: name });
  }
  return g.get(name)!;
}

export function updateBranch(state: RepoState, pr: PullRequest) {
  const full = state.repo.full_name;
  const g = graph(full);
  const base = g.get(pr.base.ref)!;
  const head = g.get(pr.head.ref)!;
  const cmp = compareBranches(base, head);
  const now = new Date().toISOString();
  const mergeNode = node(
    full,
    freshSha(full, pr.head.ref),
    tip(head)?.sha,
    { message: `Merge branch '${pr.base.ref}' into ${pr.head.ref}`, author: viewer, date: now },
    [],
    tip(base)?.sha,
  );
  head.history = [...base.history, ...cmp.ahead, mergeNode];
  head.conflicts = false;
  const before = pr.head.sha;
  pr.head.sha = mergeNode.sha;
  startRuns(
    state,
    mergeNode.sha,
    db.checkRuns(before).map((r) => r.name),
  );
  touch(state, pr, now);
  addEvent(state, pr.number, {
    ...eventBase(),
    event: "branch_updated",
    commit_id: mergeNode.sha,
    base_ref: pr.base.ref,
  });
  sync(state, pr);
  changed();
}

// Merges every open pull request whose auto-merge is on and whose requirements now pass.
function autoMerge() {
  for (const repo of db.repos()) {
    const state = db.state(repo.owner.login, repo.name);
    if (!state) continue;
    for (const pr of state.pulls) {
      if (pr.state !== "open" || !pr.auto_merge) continue;
      sync(state, pr);
      if (pr.mergeable_state !== "clean") continue;
      const { merge_method, commit_title, commit_message, enabled_by } = pr.auto_merge;
      merge(
        state,
        pr,
        { method: merge_method, title: commit_title, message: commit_message },
        enabled_by,
      );
    }
  }
}

export function afterReview() {
  changed();
}

// ---- Seed ----

function seed() {
  // Repository settings the merge box reads. repo-admin owns them; these defaults make
  // every merge method and auto-merge visible somewhere in the mock.
  edge.allow_auto_merge = true;
  raft.allow_auto_merge = true;
  raft.allow_merge_commit = true;
  raft.delete_branch_on_merge = false;

  const raftState = db.state("graftyard", "raft")!;
  const pr2 = pullRequest(raft, raftPull);
  pr2.requested_reviewers = [users.mira];
  raftState.pulls.push(pr2);
  raftState.issues.push(issueForPull(raft, raftPull, pr2));
  raftState.files.set(pr2.number, raftPull.files);
  raftState.repo.open_issues_count = raftState.repo.open_issues = raftState.issues.filter(
    (i) => i.state === "open",
  ).length;
  for (const [name, summary, at] of [
    ["test (ubuntu-latest)", "204 tests passed", "2026-11-08T08:09:00Z"],
    ["clippy", "No warnings", "2026-11-08T08:05:00Z"],
  ] as const) {
    db.addCheckRun(
      pr2.head.sha,
      checkRun(
        raft,
        pr2.head.sha,
        name,
        "completed",
        "success",
        "2026-11-08T08:01:00Z",
        at,
        summary,
      ),
    );
  }

  buildGraphs();

  const edgeState = db.state("graftyard", "edge")!;
  const pr1 = edgeState.pulls.find((p) => p.number === 1)!;
  const pr5 = edgeState.pulls.find((p) => p.number === 5)!;
  const pr9 = edgeState.pulls.find((p) => p.number === 9)!;

  // Every seeded run on a head shares one suite, as one workflow run would.
  for (const pr of [...edgeState.pulls, pr2]) {
    const suite = id();
    for (const run of db.checkRuns(pr.head.sha)) run.check_suite = { id: suite };
  }

  const runs1 = db.checkRuns(pr1.head.sha);
  const fmt = runs1.find((r) => r.name === "rustfmt")!;
  fmt.output = {
    ...fmt.output,
    title: "1 file needs formatting",
    summary: "`cargo fmt --check` found a difference in `crates/wire/src/lib.rs`.",
    text: "Diff in crates/wire/src/lib.rs at line 25:\n         .timeout(Duration::from_secs(10))\n-        .retry(|req, res, n| retry::should_retry(req.method(), res.status(), n))\n+        .retry(|req, res, n| {\n+            retry::should_retry(req.method(), res.status(), n)\n+        })\n         .build()",
    annotations_count: 1,
  };
  annotations.set(fmt.id, [
    {
      path: "crates/wire/src/lib.rs",
      start_line: 25,
      end_line: 25,
      start_column: null,
      end_column: null,
      annotation_level: "failure",
      title: "rustfmt",
      message: "This line exceeds max_width (100). Run `cargo fmt`.",
      raw_details: ".retry(|req, res, n| retry::should_retry(req.method(), res.status(), n))",
      blob_href: `${HOST}/graftyard/edge/blob/${pr1.head.sha}/crates/wire/src/lib.rs#L25`,
    },
  ]);
  const clippy = runs1.find((r) => r.name === "clippy")!;
  clippy.output = { ...clippy.output, annotations_count: 1, text: "Checked 14 crates in 38.2s." };
  annotations.set(clippy.id, [
    {
      path: "crates/wire/src/retry.rs",
      start_line: 16,
      end_line: 17,
      start_column: 5,
      end_column: 6,
      annotation_level: "notice",
      title: "clippy::cast_possible_truncation",
      message:
        "Casting `f64` to `u64` may truncate the value. The cap keeps it under 4,000, so this is allowed.",
      raw_details: null,
      blob_href: `${HOST}/graftyard/edge/blob/${pr1.head.sha}/crates/wire/src/retry.rs#L16`,
    },
  ]);
  // The byte-diff oracle is still running when the page loads, and finishes on its own.
  const byteDiff = runs1.find((r) => r.name.startsWith("byte-diff"))!;
  setTimeout(() => {
    byteDiff.status = "completed";
    byteDiff.conclusion = "success";
    byteDiff.completed_at = new Date().toISOString();
    byteDiff.output = {
      ...byteDiff.output,
      title: "1,204 of 1,204 responses match",
      summary: "1,204 of 1,204 recorded responses match byte for byte.",
    };
    changed();
  }, 30_000);

  // Reviews and threads on #1.
  const e1 = extra(edgeState, 1);
  const r1 = makeReview(
    pr1,
    users.tomas,
    "CHANGES_REQUESTED",
    "The jitter is right, but the cap needs a test, and `CAP_MS` should be public so the CLI can share it.",
    "2026-11-07T14:20:00Z",
  );
  const c1 = makeComment(
    edgeState,
    pr1,
    users.tomas,
    r1.id,
    {
      path: "crates/wire/src/retry.rs",
      side: "RIGHT",
      line: 5,
      body: "Make this `pub`, the CLI retries with the same cap.",
    },
    "2026-11-07T14:20:00Z",
  );
  const c1r = makeComment(
    edgeState,
    pr1,
    users.mira,
    null,
    {
      path: "crates/wire/src/retry.rs",
      side: "RIGHT",
      line: 5,
      body: "Done in the next push.",
      in_reply_to_id: c1.id,
    },
    "2026-11-07T16:02:00Z",
  );
  c1r.pull_request_review_id = makeReview(
    pr1,
    users.mira,
    "COMMENTED",
    "",
    "2026-11-07T16:02:00Z",
  ).id;
  const c2 = makeComment(
    edgeState,
    pr1,
    users.tomas,
    r1.id,
    {
      path: "crates/wire/src/retry.rs",
      side: "RIGHT",
      line: 15,
      start_line: 13,
      start_side: "RIGHT",
      body: "A test that `backoff(10, 1.0)` stays at `CAP_MS` would pin this range down.",
    },
    "2026-11-07T14:20:00Z",
  );
  const r2 = makeReview(
    pr1,
    users.devi,
    "APPROVED",
    "Looks good to me. HEAD is safe to repeat.",
    "2026-11-07T18:45:00Z",
  );
  const r3 = makeReview(pr1, users.lin, "COMMENTED", "", "2026-11-06T09:40:00Z");
  const c3 = makeComment(
    edgeState,
    pr1,
    users.lin,
    r3.id,
    {
      path: "crates/wire/src/method.rs",
      side: "RIGHT",
      line: 11,
      body: "Nit: `is_safe` is the RFC 9110 word for this.",
    },
    "2026-11-06T09:40:00Z",
  );
  e1.reviews.push(r3, r1, r2);
  e1.comments.push(c3, c1, c1r, c2);
  e1.threads.set(c3.id, { root_id: c3.id, resolved: true, resolved_by: users.mira });
  e1.viewed.set("mira", new Set(["crates/wire/src/method.rs"]));
  e1.events.push({
    id: id(),
    actor: users.mira,
    created_at: "2026-11-05T09:05:00Z",
    event: "review_requested",
    requested_reviewer: users.devi,
    requested_team: null,
  });
  e1.events.push({
    id: id(),
    actor: users.mira,
    created_at: "2026-11-05T09:05:00Z",
    event: "review_requested",
    requested_reviewer: null,
    requested_team: { slug: teams.edge.slug, name: teams.edge.name },
  });
  e1.requestedTeams = [teams.edge];
  pr1.assignees = [users.mira];
  pr1.assignee = users.mira;
  const issue1 = issueOf(edgeState, 1);
  if (issue1) {
    issue1.assignees = [users.mira];
    issue1.assignee = users.mira;
  }

  const e5 = extra(edgeState, 5);
  e5.reviews.push(makeReview(pr5, users.mira, "APPROVED", "Ship it.", "2026-11-05T18:10:00Z"));
  e5.events.push({
    id: id(),
    actor: users.mira,
    created_at: "2026-11-05T18:30:00Z",
    event: "merged",
    commit_id: pr5.merge_commit_sha!,
    merge_method: "squash",
    base_ref: "main",
  });

  pr9.requested_reviewers = [users.devi];
  extra(edgeState, 9).events.push({
    id: id(),
    actor: users.lin,
    created_at: "2026-11-06T14:01:00Z",
    event: "review_requested",
    requested_reviewer: users.devi,
    requested_team: null,
  });

  extra(edgeState, 14).events.push({
    id: id(),
    actor: users.tomas,
    created_at: "2026-11-07T09:00:00Z",
    event: "closed",
  });

  const e2 = extra(raftState, 2);
  e2.requestedTeams = [teams.storage];
  e2.events.push({
    id: id(),
    actor: users.lin,
    created_at: "2026-11-07T09:31:00Z",
    event: "review_requested",
    requested_reviewer: users.mira,
    requested_team: null,
  });
  e2.events.push({
    id: id(),
    actor: users.lin,
    created_at: "2026-11-07T09:31:00Z",
    event: "review_requested",
    requested_reviewer: null,
    requested_team: { slug: teams.storage.slug, name: teams.storage.name },
  });

  for (const state of [edgeState, raftState]) for (const pr of state.pulls) sync(state, pr);
}

seed();
