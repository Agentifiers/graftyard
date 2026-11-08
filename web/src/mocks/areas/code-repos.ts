import { branchSha, edge, raft, sha, users, web, type Repository } from "../world";
import type { CodeRepo, Commit } from "./code-db";
import { db } from "./core-db";
import { edgeAfter, edgeFiles, edgeImages } from "./code-seed-edge";
import { raftFiles, webFiles } from "./code-seed-small";
import { annotatedTag, atDate, chain, generateMain, seedStatuses, step } from "./code-history";

// The three seeded repositories. Every branch tip has the sha world.ts names, so the core
// area's pull request heads, the check runs keyed by them, and these refs agree.

const { mira, tomas, lin, devi } = users;

function empty(repo: Repository): CodeRepo {
  return {
    repoId: repo.id,
    commits: new Map(),
    branches: new Map(),
    tags: new Map(),
    tagObjects: new Map(),
    protectedBranches: new Set(),
    statuses: new Map(),
  };
}

function seedEdge(): CodeRepo {
  const code = empty(edge);
  const full = edge.full_name;
  const main = generateMain(
    code,
    full,
    edgeFiles,
    edgeImages,
    [
      { prefix: "crates/wire", scope: "wire", authors: [mira, tomas, mira] },
      { prefix: "crates/api", scope: "api", authors: [tomas, devi] },
      { prefix: "crates/raft", scope: "raft", authors: [lin, devi] },
      { prefix: "crates/edge", scope: "edge", authors: [mira, devi] },
      { prefix: ".github", scope: "ci", authors: [devi] },
      { prefix: "docs", scope: "docs", authors: [lin, mira] },
      { prefix: "", scope: "chore", authors: [mira, devi, lin] },
    ],
    { from: "2025-03-03T09:00:00Z", to: "2026-10-27T17:00:00Z" },
  );
  const genTip = main.at(-1)!;
  annotatedTag(
    code,
    edge,
    "v0.1.0",
    main[Math.floor(main.length * 0.3)],
    "The first wire-compatible REST slice.",
  );
  annotatedTag(
    code,
    edge,
    "v0.2.0",
    main[Math.floor(main.length * 0.6)],
    "ETags and GHES error bodies.",
  );
  annotatedTag(code, edge, "v0.3.0", genTip, "Learner groups and the byte-diff gate.");

  const release = step(code, genTip, {
    sha: branchSha(full, "release/0.4"),
    message: "release: 0.4.0-rc.1",
    by: mira,
    at: "2026-10-30T15:20:00Z",
    change: { "Cargo.toml": edgeAfter.version },
  });
  annotatedTag(code, edge, "v0.4.0-rc.1", release, "Release candidate for 0.4.");

  const ratelimit = chain(code, genTip, [
    {
      message: "wire: send rate limit headers on error responses",
      by: devi,
      at: "2026-11-04T10:00:00Z",
      change: { "crates/wire/src/error.rs": edgeAfter.error },
    },
    {
      sha: branchSha(full, "devi/ratelimit-on-errors"),
      message: "docs: note rate limit headers in the changelog",
      by: devi,
      at: "2026-11-04T16:45:00Z",
      change: { "CHANGELOG.md": edgeAfter.changelog },
    },
  ]);

  // PR #5 squash-merged into main, as core-db's merge_commit_sha says.
  const merged = step(code, genTip, {
    sha: sha(`${full}:merge:5`),
    message:
      "Send rate limit headers on every error response (#5)\n\nFixes #11. Adds `x-ratelimit-limit`, `-remaining`, `-used`, and `-reset` to 4xx and 5xx responses.",
    by: devi,
    at: "2026-11-05T18:30:00Z",
    change: {
      "crates/wire/src/error.rs": edgeAfter.error,
      "CHANGELOG.md": edgeAfter.changelog,
    },
  });

  const mainTip = step(code, merged, {
    sha: branchSha(full, "main"),
    message: "ci: cache the cargo registry between jobs",
    by: devi,
    at: "2026-11-07T16:20:00Z",
    change: {
      ".github/workflows/ci.yml": (src) =>
        src.replace(
          "      - uses: dtolnay/rust-toolchain@stable\n",
          "      - uses: dtolnay/rust-toolchain@stable\n      - uses: Swatinem/rust-cache@v2\n",
        ),
    },
  });

  const retry = chain(code, atDate(code, genTip, "2026-11-05T09:00:00Z"), [
    {
      message: "wire: retry only idempotent reads, with full jitter",
      by: mira,
      at: "2026-11-05T09:00:00Z",
      change: {
        "crates/wire/src/method.rs": edgeAfter.method,
        "crates/wire/src/retry.rs": edgeAfter.retry,
        "crates/wire/src/lib.rs": edgeAfter.libWithMethod,
      },
    },
    {
      sha: branchSha(full, "mira/idempotent-retry"),
      message: "wire: pass the request method into the retry policy",
      by: mira,
      at: "2026-11-08T10:12:00Z",
      change: { "crates/wire/src/lib.rs": edgeAfter.libRetryClosure },
    },
  ]);

  const learner = chain(code, merged, [
    {
      message: "raft: add a learner read path",
      by: lin,
      at: "2026-11-06T14:00:00Z",
      change: { "crates/raft/src/read.rs": edgeAfter.read },
    },
    {
      sha: branchSha(full, "lin/learner-reads"),
      message: "raft: pick a learner when the leader is cold",
      by: lin,
      at: "2026-11-07T21:10:00Z",
      change: {
        "crates/raft/src/read.rs": (src) =>
          src.replace(
            "impl ReadPath {",
            "impl ReadPath {\n    pub fn is_learner(&self) -> bool {\n        matches!(self, ReadPath::Learner { .. })\n    }\n",
          ),
      },
    },
  ]);

  const labels = step(code, merged, {
    sha: branchSha(full, "tomas/label-objects"),
    message: "api: accept label objects on issue create",
    by: tomas,
    at: "2026-11-06T08:00:00Z",
    change: { "crates/api/src/rest/issues.rs": edgeAfter.issues },
  });

  const tips: [string, Commit][] = [
    ["main", mainTip],
    ["mira/idempotent-retry", retry],
    ["devi/ratelimit-on-errors", ratelimit],
    ["lin/learner-reads", learner],
    ["tomas/label-objects", labels],
    ["release/0.4", release],
  ];
  for (const [name, c] of tips) code.branches.set(name, c.sha);
  code.protectedBranches.add("main").add("release/0.4");

  seedStatuses(code, edge, mainTip, "success");
  seedStatuses(code, edge, merged, "success");
  seedStatuses(code, edge, retry, "failure");
  seedStatuses(code, edge, learner, "pending");
  seedStatuses(code, edge, ratelimit, "success");
  seedStatuses(code, edge, release, "success");
  return code;
}

function seedRaft(): CodeRepo {
  const code = empty(raft);
  const full = raft.full_name;
  const main = generateMain(
    code,
    full,
    raftFiles,
    {},
    [
      { prefix: "src/vote", scope: "vote", authors: [lin, tomas] },
      { prefix: "src/lease", scope: "lease", authors: [lin] },
      { prefix: "tests", scope: "chaos", authors: [tomas] },
      { prefix: "", scope: "log", authors: [lin, devi] },
    ],
    { from: "2025-06-02T09:00:00Z", to: "2026-10-20T17:00:00Z" },
  );
  annotatedTag(code, raft, "v0.1.0", main[Math.floor(main.length * 0.7)], "First release.");
  const mainTip = step(code, main.at(-1)!, {
    sha: branchSha(full, "main"),
    message: "vote: document why pre-vote exists",
    by: lin,
    at: "2026-11-05T08:03:00Z",
    change: {
      "src/vote.rs": (src) => src.replace("/// Pre-vote:", "/// Pre-vote (Ongaro, section 9.6):"),
    },
  });
  const joint = chain(code, main.at(-1)!, [
    {
      message: "member: track joint configurations",
      by: lin,
      at: "2026-11-03T11:00:00Z",
      change: {
        "src/member.rs": (src) =>
          `${src}\n/// Old and new voter sets during a membership change.\n#[derive(Clone, Debug, Default)]\npub struct Joint {\n    pub old: Vec<u64>,\n    pub new: Vec<u64>,\n}\n`,
      },
    },
    {
      sha: branchSha(full, "lin/joint-consensus"),
      message: "log: require both majorities while joint",
      by: lin,
      at: "2026-11-04T15:30:00Z",
      change: {
        "src/joint.rs": `use crate::member::Joint;\n\n/// An entry commits only when both the old and the new voters have a majority.\npub fn committed(joint: &Joint, acked: &[u64]) -> bool {\n    majority(&joint.old, acked) && majority(&joint.new, acked)\n}\n\nfn majority(voters: &[u64], acked: &[u64]) -> bool {\n    voters.iter().filter(|v| acked.contains(v)).count() * 2 > voters.len()\n}\n`,
        "src/lib.rs": (src) => src.replace("pub mod lease;", "pub mod joint;\npub mod lease;"),
      },
    },
  ]);
  code.branches.set("main", mainTip.sha).set("lin/joint-consensus", joint.sha);
  code.protectedBranches.add("main");
  seedStatuses(code, raft, mainTip, "success");
  seedStatuses(code, raft, joint, "pending");
  return code;
}

function seedWeb(): CodeRepo {
  const code = empty(web);
  const full = web.full_name;
  const main = generateMain(
    code,
    full,
    webFiles,
    {},
    [
      { prefix: "src/components", scope: "ui", authors: [lin, devi] },
      { prefix: "src", scope: "app", authors: [devi, mira] },
      { prefix: "", scope: "chore", authors: [devi] },
    ],
    { from: "2026-02-02T09:00:00Z", to: "2026-11-01T17:00:00Z" },
  );
  const mainTip = step(code, main.at(-1)!, {
    sha: branchSha(full, "main"),
    message: "app: preload routes on intent",
    by: devi,
    at: "2026-11-07T17:40:00Z",
    change: { "README.md": (src) => `${src}\nRoutes preload on hover, so a click rarely waits.\n` },
  });
  const dark = step(code, mainTip, {
    sha: branchSha(full, "lin/dark-diff"),
    message: "ui: raise diff deletion contrast in dark mode",
    by: lin,
    at: "2026-11-07T19:05:00Z",
    change: {
      "src/components/diff.tsx": (src) =>
        src.replace(
          'del: "bg-destructive/12",',
          'del: "bg-destructive/12 dark:bg-destructive/20",',
        ),
    },
  });
  code.branches.set("main", mainTip.sha).set("lin/dark-diff", dark.sha);
  seedStatuses(code, web, mainTip, "success");
  seedStatuses(code, web, dark, "success");
  return code;
}

// Keyed by repository id, so a renamed or transferred repository keeps its git data.
const byId = new Map<number, CodeRepo>([
  [edge.id, seedEdge()],
  [raft.id, seedRaft()],
  [web.id, seedWeb()],
]);

// The repository and its git data. A repository another area created starts empty.
export function codeRepo(owner: string, name: string) {
  const state = db.state(owner, name);
  if (!state) return undefined;
  let code = byId.get(state.repo.id);
  if (!code) byId.set(state.repo.id, (code = empty(state.repo)));
  return { repo: state.repo, code, state };
}
