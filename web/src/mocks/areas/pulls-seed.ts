import { file } from "./core-db";
import { edge, raft, users, web, type Repository, type SimpleUser } from "../world";

// The plans pulls-db.ts builds commit graphs from. Each branch forks from its repository's
// default branch after `fork` commits, so ahead and behind counts fall out of the graph.
// A branch whose last commit is not given a sha ends on world.ts's branchSha, which is the
// head sha the shared PR records and other areas already use.

type DiffEntry = ReturnType<typeof file>;

export type CommitPlan = {
  message: string;
  author: SimpleUser;
  date: string;
  // Filenames of this commit's changes, taken from the branch's files.
  files: string[];
};

export type BranchPlan = {
  fork: number;
  commits: CommitPlan[];
  // Files only this plan knows. PR heads take theirs from the shared PR files instead.
  files?: DiffEntry[];
  conflicts?: boolean;
};

export type RepoPlan = {
  repo: Repository;
  main: (CommitPlan & { patch: [path: string, patch: string] })[];
  branches: Record<string, BranchPlan>;
};

const day = (d: string, t = "12:00") => `2026-${d}T${t}:00Z`;

export const plans: RepoPlan[] = [
  {
    repo: edge,
    main: [
      {
        message: "Serve GET /repos/{owner}/{repo} from the repo fold",
        author: users.mira,
        date: day("10-20", "09:12"),
        files: [],
        patch: [
          "crates/api/src/rest/repos.rs",
          `@@ -1,3 +1,6 @@
 use crate::fold::RepoFold;
+
+pub fn get(fold: &RepoFold) -> Response {
+    Response::json(&fold.repository())
+}`,
        ],
      },
      {
        message: "Add Link header pagination",
        author: users.tomas,
        date: day("10-24", "15:40"),
        files: [],
        patch: [
          "crates/wire/src/link.rs",
          `@@ -0,0 +1,4 @@
+pub fn link(base: &str, page: u32, last: u32) -> String {
+    format!("<{base}?page={}>; rel=\\"next\\", <{base}?page={last}>; rel=\\"last\\"", page + 1)
+}
+`,
        ],
      },
      {
        message: "Parse per_page and page like GHES",
        author: users.devi,
        date: day("10-27", "11:05"),
        files: [],
        patch: [
          "crates/wire/src/pagination.rs",
          `@@ -4,4 +4,6 @@ pub struct Page {
 pub fn parse(q: &Query) -> Page {
-    Page { per_page: q.get("per_page").unwrap_or(30), page: q.get("page").unwrap_or(1) }
+    let per_page = q.get("per_page").unwrap_or(30).clamp(1, 100);
+    let page = q.get("page").unwrap_or(1).max(1);
+    Page { per_page, page }
 }`,
        ],
      },
      {
        message: "Cut release/0.4",
        author: users.mira,
        date: day("10-30", "17:00"),
        files: [],
        patch: [
          "CHANGELOG.md",
          `@@ -1,3 +1,7 @@
 # Changelog
 
+## 0.4.0
+
+- Link header pagination and GHES page parsing.
+`,
        ],
      },
      {
        message: "Send rate limit headers on every error response (#5)",
        author: users.devi,
        date: day("11-05", "18:30"),
        files: [],
        patch: [
          "crates/wire/src/error.rs",
          `@@ -40,9 +40,11 @@ impl ErrorResponse {
     pub fn render(&self, limits: &RateLimit) -> Response {
         let mut res = Response::json(&self.body).with_status(self.status);
-        if self.status < 400 {
-            limits.apply(res.headers_mut());
-        }
+        // GHES sends the rate limit headers on errors too, and
+        // Octokit's throttling plugin reads them from 403 and 429.
+        limits.apply(res.headers_mut());
         res
     }
 }`,
        ],
      },
      {
        message: "Record GHES 3.22 fixtures for the issues list",
        author: users.lin,
        date: day("11-05", "20:10"),
        files: [],
        patch: [
          "tests/fixtures/issues-list.json",
          `@@ -0,0 +1,3 @@
+[
+  { "number": 2, "state": "open", "title": "GET /repos/{owner}/{repo}/issues omits Link header on the last page" }
+]`,
        ],
      },
      {
        message: "Bump wrangler to 4.40",
        author: users.tomas,
        date: day("11-06", "08:30"),
        files: [],
        patch: [
          "wrangler.toml",
          `@@ -1,3 +1,3 @@
 name = "graftyard-edge"
-compatibility_date = "2026-09-01"
+compatibility_date = "2026-11-01"
 main = "./build/worker/shim.mjs"`,
        ],
      },
      {
        message: "Tidy the error module's imports",
        author: users.devi,
        date: day("11-07", "17:40"),
        files: [],
        patch: [
          "crates/wire/src/error.rs",
          `@@ -1,4 +1,3 @@
-use std::fmt;
 use crate::headers::RateLimit;
 use crate::response::Response;
 `,
        ],
      },
    ],
    branches: {
      "mira/idempotent-retry": {
        fork: 5,
        commits: [
          {
            message:
              "Retry only GETs and HEADs, at most twice\n\nA retried POST can create the same issue twice. Reads are the only requests that are safe to repeat.",
            author: users.mira,
            date: day("11-05", "09:00"),
            files: ["crates/wire/src/retry.rs", "crates/wire/src/method.rs"],
          },
          {
            message: "Pass the request method to should_retry",
            author: users.mira,
            date: day("11-08", "10:12"),
            files: ["crates/wire/src/lib.rs"],
          },
        ],
      },
      "devi/ratelimit-on-errors": {
        fork: 4,
        commits: [
          {
            message: "Send rate limit headers on every error response",
            author: users.devi,
            date: day("11-04", "10:00"),
            files: ["crates/wire/src/error.rs"],
          },
        ],
      },
      "lin/learner-reads": {
        fork: 8,
        commits: [
          {
            message: "Read hot issue lists from a caught-up learner",
            author: users.lin,
            date: day("11-07", "21:10"),
            files: ["crates/raft/src/read.rs"],
          },
        ],
      },
      "tomas/label-objects": {
        fork: 7,
        commits: [
          {
            message: "Accept label objects on issue create",
            author: users.tomas,
            date: day("11-06", "08:00"),
            files: ["crates/api/src/rest/issues.rs"],
          },
        ],
      },
      "release/0.4": {
        fork: 4,
        conflicts: true,
        commits: [
          {
            message: "Pin wrangler for the 0.4 line",
            author: users.mira,
            date: day("10-31", "10:00"),
            files: ["wrangler.toml"],
          },
          {
            message: "Backport: answer 304 for a matching ETag",
            author: users.tomas,
            date: day("11-02", "14:20"),
            files: ["crates/api/src/rest/etag.rs"],
          },
        ],
        files: [
          file(
            edge,
            "release/0.4",
            "wrangler.toml",
            "modified",
            `@@ -1,3 +1,3 @@
 name = "graftyard-edge"
-compatibility_date = "2026-09-01"
+compatibility_date = "2026-09-15"
 main = "./build/worker/shim.mjs"`,
          ),
          file(
            edge,
            "release/0.4",
            "crates/api/src/rest/etag.rs",
            "added",
            `@@ -0,0 +1,11 @@
+use crate::response::Response;
+
+/// Answers 304 when the client's ETag matches the representation's.
+pub fn conditional(req: &Request, etag: &str, res: Response) -> Response {
+    match req.header("if-none-match") {
+        Some(tag) if tag == etag => Response::not_modified().with_header("etag", etag),
+        _ => res.with_header("etag", etag),
+    }
+}
+
+// The weak validator form (W/"...") is compared the same way.`,
          ),
        ],
      },
    },
  },
  {
    repo: raft,
    main: [
      {
        message: "Log, lease, and leader transfer",
        author: users.lin,
        date: day("10-12", "09:00"),
        files: [],
        patch: [
          "crates/raft/src/lib.rs",
          `@@ -0,0 +1,4 @@
+pub mod lease;
+pub mod log;
+pub mod read;
+pub mod transfer;`,
        ],
      },
      {
        message: "Pre-vote before a term bump",
        author: users.tomas,
        date: day("10-21", "13:00"),
        files: [],
        patch: [
          "crates/raft/src/election.rs",
          `@@ -10,3 +10,6 @@ impl Node {
     pub fn on_timeout(&mut self) {
-        self.start_election();
+        if self.pre_vote_granted() {
+            self.start_election();
+        }
     }`,
        ],
      },
      {
        message: "Learners replicate but never vote",
        author: users.lin,
        date: day("10-29", "16:30"),
        files: [],
        patch: [
          "crates/raft/src/membership.rs",
          `@@ -1,3 +1,8 @@
 pub struct Membership {
     pub voters: BTreeSet<NodeId>,
+    pub learners: BTreeSet<NodeId>,
+}
+
+impl Membership {
+    pub fn can_vote(&self, id: NodeId) -> bool { self.voters.contains(&id) }
 }`,
        ],
      },
      {
        message: "Chaos run: restart two members together",
        author: users.devi,
        date: day("11-05", "08:03"),
        files: [],
        patch: [
          "tests/chaos/restart.rs",
          `@@ -0,0 +1,3 @@
+#[test]
+fn two_members_restart_together() {
+}`,
        ],
      },
    ],
    branches: {
      "lin/joint-consensus": {
        fork: 3,
        commits: [
          {
            message: "Model joint configurations in Membership",
            author: users.lin,
            date: day("11-07", "09:00"),
            files: ["crates/raft/src/membership.rs", "crates/raft/src/lib.rs"],
          },
          {
            message: "Generate the joint-consensus transition table tests",
            author: users.lin,
            date: day("11-07", "18:20"),
            files: ["crates/raft/tests/joint_consensus.rs"],
          },
          {
            message: "Require both majorities while a joint config is active",
            author: users.lin,
            date: day("11-08", "08:00"),
            files: [],
          },
        ],
      },
    },
  },
  {
    repo: web,
    main: [
      {
        message: "First slice of the web app on a mocked GHES 3.22 API",
        author: users.mira,
        date: day("11-02", "10:00"),
        files: [],
        patch: [
          "README.md",
          `@@ -0,0 +1,2 @@
+# Graftyard web app
+`,
        ],
      },
      {
        message: "Square corners everywhere",
        author: users.devi,
        date: day("11-05", "11:00"),
        files: [],
        patch: [
          "src/styles.css",
          `@@ -82,3 +82,3 @@
 :root {
-  --radius: 0.625rem;
+  --radius: 0.125rem;
 }`,
        ],
      },
      {
        message: "Split the mock and the queries by area",
        author: users.mira,
        date: day("11-07", "17:40"),
        files: [],
        patch: [
          "src/mocks/handlers.ts",
          `@@ -1,3 +1,4 @@
 import type { RequestHandler } from "msw";
+// Every module in ./areas that exports handlers is served.`,
        ],
      },
    ],
    branches: {
      "lin/dark-diff": {
        fork: 3,
        commits: [
          {
            message: "Raise deletion contrast in dark mode",
            author: users.lin,
            date: day("11-07", "19:00"),
            files: ["src/styles.css"],
          },
          {
            message: "Use the stronger tint for deleted lines in the diff",
            author: users.lin,
            date: day("11-08", "09:15"),
            files: ["src/components/diff.tsx"],
          },
        ],
        files: [
          file(
            web,
            "lin/dark-diff",
            "src/styles.css",
            "modified",
            `@@ -120,6 +120,9 @@
 .dark {
   --background: oklch(0.145 0 0);
   --foreground: oklch(0.985 0 0);
-  --destructive: oklch(0.577 0.245 27.325);
+  /* 4.6:1 against the card background, so red text and tints pass AA. */
+  --destructive: oklch(0.704 0.191 22.216);
+  --diff-del: oklch(0.704 0.191 22.216 / 18%);
 }`,
          ),
          file(
            web,
            "lin/dark-diff",
            "src/components/diff.tsx",
            "modified",
            `@@ -31,5 +31,5 @@ const ROW = {
   hunk: "bg-info/10 text-muted-foreground",
   context: "",
   add: "bg-success/12",
-  del: "bg-destructive/12",
+  del: "bg-destructive/12 dark:bg-(--diff-del)",
 } as const;`,
          ),
        ],
      },
    },
  },
];

// ---- A pull request with a long diff, so the Files tab virtualizes ----

function transitionTable(cases: number) {
  const states = ["Stable", "Joint", "Leaving"];
  const lines = [
    "@@ -0,0 +1,0 @@",
    "+//! Generated: every (from, to, event) transition of a joint configuration.",
    "+//! Regenerate with `cargo xtask joint-table`.",
    "+",
    "+use graftyard_raft::membership::{Config, Event, Membership};",
    "+",
  ];
  for (let i = 0; i < cases; i++) {
    const from = states[i % 3];
    const to = states[(i + 1) % 3];
    const voters = 3 + (i % 3) * 2;
    lines.push(
      "+#[test]",
      `+fn case_${String(i).padStart(3, "0")}_${from.toLowerCase()}_to_${to.toLowerCase()}() {`,
      `+    let mut m = Membership::with_voters(${voters});`,
      `+    m.apply(Event::Propose(Config::${to}));`,
      `+    assert_eq!(m.phase(), Config::${to}, "case ${i}: ${from} -> ${to}");`,
      `+    assert!(m.quorum_of(&[${Array.from({ length: Math.ceil(voters / 2) + 1 }, (_, k) => k + 1).join(", ")}]));`,
      "+}",
      "+",
    );
  }
  lines[0] = `@@ -0,0 +1,${lines.length - 1} @@`;
  return lines.join("\n");
}

export const raftPull = {
  number: 2,
  title: "Joint consensus for membership changes",
  body: "Adds joint configurations, so a membership change never loses quorum. Both the old and the new majority must agree while a joint config is active.\n\nThe transition table tests are generated (about 1,450 lines), which is a good stress test for the diff view.\n\nCloses #1.",
  author: users.lin,
  labels: ["perf"],
  head: "lin/joint-consensus",
  created: day("11-07", "09:30"),
  updated: day("11-08", "08:00"),
  files: [
    file(
      raft,
      "lin/joint-consensus",
      "crates/raft/src/membership.rs",
      "modified",
      `@@ -1,8 +1,34 @@
 pub struct Membership {
     pub voters: BTreeSet<NodeId>,
     pub learners: BTreeSet<NodeId>,
+    /// The configuration being moved to, while a change is in flight.
+    pub joining: Option<BTreeSet<NodeId>>,
 }
 
 impl Membership {
-    pub fn can_vote(&self, id: NodeId) -> bool { self.voters.contains(&id) }
+    pub fn can_vote(&self, id: NodeId) -> bool {
+        self.voters.contains(&id) || self.joining.as_ref().is_some_and(|j| j.contains(&id))
+    }
+
+    /// A quorum needs a majority of the old voters and, while joint, of the new ones too.
+    pub fn quorum_of(&self, acks: &[NodeId]) -> bool {
+        let majority = |set: &BTreeSet<NodeId>| {
+            acks.iter().filter(|a| set.contains(a)).count() * 2 > set.len()
+        };
+        majority(&self.voters) && self.joining.as_ref().map_or(true, majority)
+    }
+
+    pub fn phase(&self) -> Config {
+        match &self.joining {
+            None => Config::Stable,
+            Some(next) if next.is_subset(&self.voters) => Config::Leaving,
+            Some(_) => Config::Joint,
+        }
+    }
+}
+
+#[derive(Clone, Copy, Debug, PartialEq, Eq)]
+pub enum Config {
+    Stable,
+    Joint,
+    Leaving,
 }`,
    ),
    file(
      raft,
      "lin/joint-consensus",
      "crates/raft/src/lib.rs",
      "modified",
      `@@ -1,4 +1,5 @@
 pub mod lease;
 pub mod log;
+pub mod membership;
 pub mod read;
 pub mod transfer;`,
    ),
    file(
      raft,
      "lin/joint-consensus",
      "crates/raft/tests/joint_consensus.rs",
      "added",
      transitionTable(180),
    ),
  ],
};
