import { splitLines } from "@/areas/code/text-diff";
import { sha, users, type Repository, type SimpleUser } from "../world";
import {
  addStatus,
  blobText,
  makeCommit,
  person,
  putBlob,
  putText,
  type CodeRepo,
  type Commit,
} from "./code-db";

// Builds a repository's history from its final working tree. Each file arrives in a few
// growing chunks spread over the repository's life, so blame, stats, and the commit graph
// look lived-in, and main's last generated tree is exactly the seed. Scripted commits
// then add the branches, merges, and tags that the other areas' records name.

type Files = Record<string, string>;

const rand = (seed: string) => parseInt(sha(seed).slice(0, 8), 16) / 0xffffffff;

type Owners = { prefix: string; scope: string; authors: SimpleUser[] }[];

type Event = { path: string; upto: number; total: number; text: string };

function scopeOf(owners: Owners, path: string) {
  return owners.find((o) => path.startsWith(o.prefix)) ?? owners.at(-1)!;
}

function messageFor(scope: string, events: Event[]) {
  const what = (e: Event) =>
    e.path
      .split("/")
      .at(-1)!
      .replace(/\.[a-z]+$/, "");
  const first = events[0];
  const ends = chunkEnds(first.total);
  const verb =
    first.upto === ends[0]
      ? "add"
      : first.text
            .split("\n")
            .slice(0, first.upto)
            .slice(-12)
            .some((l) => l.includes("#[test]"))
        ? "test"
        : first.upto === first.total
          ? "finish"
          : "extend";
  const tail = events.length > 1 ? ` and ${what(events[1])}` : "";
  return `${scope}: ${verb} ${what(first)}${tail}`;
}

// Where each chunk of a file ends: about five lines per commit, at most ten commits.
function chunkEnds(total: number): number[] {
  const n = Math.max(1, Math.min(10, Math.round(total / 5)));
  return Array.from({ length: n }, (_, i) => Math.ceil((total * (i + 1)) / n));
}

// A working day between `from` and `to`, at office hours, for commit `i` of `n`.
function dateAt(seed: string, from: number, to: number, i: number, n: number) {
  const t = from + ((to - from) * (i + rand(`${seed}:jit`) * 0.8)) / n;
  const d = new Date(t);
  if (rand(`${seed}:wk`) > 0.12) {
    while (d.getUTCDay() === 0 || d.getUTCDay() === 6) d.setUTCDate(d.getUTCDate() + 1);
  }
  d.setUTCHours(8 + Math.floor(rand(`${seed}:h`) * 11), Math.floor(rand(`${seed}:m`) * 60), 0, 0);
  return d.toISOString().replace(/\.\d{3}Z$/, "Z");
}

export function generateMain(
  code: CodeRepo,
  full: string,
  files: Files,
  binary: Files,
  owners: Owners,
  span: { from: string; to: string },
): Commit[] {
  const events: Event[] = [];
  const order = Object.keys(files);
  // Every file starts in the first two thirds, then grows in later chunks.
  const queue = order.map((path, k) => {
    const text = files[path];
    const total = splitLines(text).length;
    return { path, text, total, ends: chunkEnds(total), start: k };
  });
  let round = 0;
  while (queue.some((q) => q.ends.length)) {
    for (const q of queue) {
      if (!q.ends.length || q.start > round * 3 + 4) continue;
      if (round > 0 && rand(`${full}:${q.path}:${round}`) < 0.35) continue;
      events.push({ path: q.path, upto: q.ends.shift()!, total: q.total, text: q.text });
    }
    round++;
  }
  // Group neighbouring events of one scope into a commit.
  const groups: Event[][] = [];
  for (const e of events) {
    const last = groups.at(-1);
    const same =
      last &&
      last.length < 2 &&
      scopeOf(owners, last[0].path).scope === scopeOf(owners, e.path).scope &&
      !last.some((x) => x.path === e.path) &&
      rand(`${full}:g:${e.path}:${e.upto}`) < 0.5;
    if (same) last.push(e);
    else groups.push([e]);
  }
  const from = Date.parse(span.from);
  const to = Date.parse(span.to);
  const tree = new Map<string, string>();
  const made: Commit[] = [];
  let parent: string | undefined;
  groups.forEach((group, i) => {
    for (const e of group) {
      const lines = e.text.split("\n");
      const content = e.upto === e.total ? e.text : `${lines.slice(0, e.upto).join("\n")}\n`;
      tree.set(e.path, putText(content));
    }
    if (i === 0) for (const [path, b64] of Object.entries(binary)) tree.set(path, putBlob(b64));
    const owner = scopeOf(owners, group[0].path);
    const who = owner.authors[Math.floor(rand(`${full}:a:${i}`) * owner.authors.length)];
    const c = makeCommit(code, {
      message: i === 0 ? "Initial commit" : messageFor(owner.scope, group),
      author: person(who),
      committer: person(who),
      date: dateAt(`${full}:${i}`, from, to, i, groups.length),
      parents: parent ? [parent] : [],
      files: new Map(tree),
    });
    made.push(c);
    parent = c.sha;
  });
  return made;
}

type Step = {
  message: string;
  by: SimpleUser;
  at: string;
  change: Record<string, string | null | ((src: string) => string)>;
  sha?: string;
  parents?: string[];
};

// One scripted commit on top of `base`. A change is new content, a function of the old
// content, or null to delete the path.
export function step(code: CodeRepo, base: Commit, s: Step): Commit {
  const files = new Map(base.files);
  for (const [path, change] of Object.entries(s.change)) {
    if (change === null) {
      files.delete(path);
      continue;
    }
    const old = files.get(path);
    const text = typeof change === "function" ? change(blobText(old) ?? "") : change;
    files.set(path, putText(text));
  }
  return makeCommit(code, {
    sha: s.sha,
    message: s.message,
    author: person(s.by),
    committer: person(s.by),
    date: s.at,
    parents: s.parents ?? [base.sha],
    files,
  });
}

export function chain(code: CodeRepo, base: Commit, steps: Step[]): Commit {
  return steps.reduce((c, s) => step(code, c, s), base);
}

// A commit `n` steps back from `c` on its first-parent line.
export function back(code: CodeRepo, c: Commit, n: number): Commit {
  let cur = c;
  for (let i = 0; i < n && cur.parents[0]; i++) cur = code.commits.get(cur.parents[0])!;
  return cur;
}

// The newest first-parent ancestor committed at or before `at`.
export function atDate(code: CodeRepo, c: Commit, at: string): Commit {
  let cur = c;
  while (cur.date > at && cur.parents[0]) cur = code.commits.get(cur.parents[0])!;
  return cur;
}

export function annotatedTag(
  code: CodeRepo,
  repo: Repository,
  name: string,
  target: Commit,
  message: string,
) {
  const key = sha(`tagobj:${repo.full_name}:${name}`);
  code.tagObjects.set(key, {
    node_id: `TAG_${key.slice(0, 12)}`,
    tag: name,
    sha: key,
    url: `${repo.url}/git/tags/${key}`,
    message,
    tagger: { date: target.date, email: "mira@graftyard.test", name: "Mira Okafor" },
    object: { sha: target.sha, type: "commit", url: `${repo.url}/git/commits/${target.sha}` },
    verification: {
      verified: true,
      reason: "valid",
      signature: null,
      payload: null,
      verified_at: target.date,
    },
  });
  code.tags.set(name, key);
}

export function seedStatuses(code: CodeRepo, repo: Repository, c: Commit, state: string) {
  const ci = users.org;
  const at = new Date(Date.parse(c.date) + 6 * 60_000).toISOString().replace(/\.\d{3}Z$/, "Z");
  addStatus(
    code,
    repo,
    c.sha,
    {
      state,
      context: "ci/build",
      description:
        state === "success"
          ? "Build passed in 3m 12s"
          : state === "pending"
            ? "Build is running"
            : "Build failed: 2 tests",
      target_url: `${repo.html_url}/actions`,
    },
    ci,
    at,
  );
  addStatus(
    code,
    repo,
    c.sha,
    {
      state: state === "failure" ? "success" : state,
      context: "deploy/preview",
      description: state === "pending" ? "Waiting for the build" : "Preview is live",
      target_url: `https://preview.graftyard.test/${c.sha.slice(0, 7)}`,
    },
    ci,
    at,
  );
}
