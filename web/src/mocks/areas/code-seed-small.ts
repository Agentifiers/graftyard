// The graftyard/raft and graftyard/web working trees on main, before the scripted commits.

export const raftFiles: Record<string, string> = {
  "README.md": `# raft

Log, lease, transfer, and learners for each Graftyard group.

- **Log.** One entry in flight per group. A follower verifies entry k+1 only after it
  applies k.
- **Lease.** A leader answers reads without a round trip while its lease holds.
- **Transfer.** Leadership moves to the member with the freshest log.
- **Learners.** Non-voting members that serve hot reads.

\`\`\`sh
cargo test
cargo test --features chaos -- --ignored
\`\`\`
`,
  "Cargo.toml": `[package]
name = "raft"
version = "0.1.0"
edition = "2021"
license = "Apache-2.0"

[dependencies]
serde = { version = "1", features = ["derive"] }

[features]
chaos = []
`,
  "src/lib.rs": `pub mod lease;
pub mod log;
pub mod member;
pub mod vote;

pub use log::{Entry, Log};
pub use member::{Member, Role};
`,
  "src/log.rs": `/// One committed command and the term it was proposed in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub index: u64,
    pub term: u64,
    pub command: Vec<u8>,
}

#[derive(Default)]
pub struct Log {
    entries: Vec<Entry>,
    applied: u64,
}

impl Log {
    pub fn last(&self) -> Option<&Entry> {
        self.entries.last()
    }

    /// A follower appends k+1 only after it applied k.
    pub fn append(&mut self, entry: Entry) -> Result<(), Gap> {
        if entry.index != self.applied + 1 {
            return Err(Gap { expected: self.applied + 1, got: entry.index });
        }
        self.entries.push(entry);
        Ok(())
    }

    pub fn apply(&mut self, index: u64) {
        self.applied = self.applied.max(index);
    }
}

#[derive(Debug)]
pub struct Gap {
    pub expected: u64,
    pub got: u64,
}
`,
  "src/lease.rs": `use std::time::{Duration, Instant};

pub const LEASE: Duration = Duration::from_secs(9);

pub struct Lease {
    started: Instant,
}

impl Lease {
    pub fn start(now: Instant) -> Self {
        Self { started: now }
    }

    pub fn holds(&self, now: Instant) -> bool {
        now.duration_since(self.started) < LEASE
    }
}
`,
  "src/member.rs": `#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Leader,
    Follower,
    Candidate,
    Learner,
}

#[derive(Clone, Debug)]
pub struct Member {
    pub id: u64,
    pub role: Role,
    pub voting: bool,
}
`,
  "src/vote.rs": `use crate::log::Entry;

/// Pre-vote: a candidate asks before it bumps its term, so a partitioned member
/// cannot disrupt a healthy group when it rejoins.
pub fn grant_pre_vote(candidate_last: Option<&Entry>, ours: Option<&Entry>) -> bool {
    match (candidate_last, ours) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(c), Some(o)) => (c.term, c.index) >= (o.term, o.index),
    }
}
`,
  "tests/chaos.rs": `#[test]
#[ignore]
fn two_members_restart_together() {
    let mut cluster = Cluster::new(5);
    cluster.restart(&[1, 2]);
    assert!(cluster.elects_within(std::time::Duration::from_secs(3)));
}
`,
};

export const webFiles: Record<string, string> = {
  "README.md": `# web

The Graftyard web app: React 19 on TanStack Start in SPA mode.

\`\`\`sh
npm ci
npm run dev
\`\`\`

Every page region is its own error boundary with its own query, so one failing
backend empties one box and nothing else.
`,
  "package.json": `{
  "name": "graftyard-web",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite dev --port 3000",
    "build": "vite build",
    "check": "npm run typecheck && npm run build"
  }
}
`,
  "tsconfig.json": `{
  "compilerOptions": {
    "target": "ES2022",
    "jsx": "react-jsx",
    "module": "ESNext",
    "strict": true,
    "moduleResolution": "bundler"
  }
}
`,
  "src/router.tsx": `import { createRouter } from "@tanstack/react-router";
import { routeTree } from "./routeTree.gen";

export function getRouter() {
  return createRouter({ routeTree, defaultPreload: "intent" });
}
`,
  "src/components/diff.tsx": `type Line = { kind: "add" | "del" | "context"; text: string };

const ROW = {
  add: "bg-success/12",
  del: "bg-destructive/12",
  context: "",
} as const;

export function DiffLine({ line }: { line: Line }) {
  return <tr className={ROW[line.kind]}><td>{line.text}</td></tr>;
}
`,
  "src/components/region.tsx": `import { Suspense, type ReactNode } from "react";
import { ErrorBoundary } from "react-error-boundary";

export function Region({ name, children }: { name: string; children: ReactNode }) {
  return (
    <ErrorBoundary fallbackRender={({ error }) => <p>Could not load {name}: {String(error)}</p>}>
      <Suspense fallback={<p>Loading</p>}>{children}</Suspense>
    </ErrorBoundary>
  );
}
`,
  "src/styles.css": `@import "tailwindcss";

:root {
  --radius: 0.125rem;
}
`,
};
