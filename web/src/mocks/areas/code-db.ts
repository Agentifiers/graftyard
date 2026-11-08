import type { Schemas } from "@/api/client";
import { decodeText, encodeText, wrap60 } from "@/areas/code/base64";
import { matchLines, splitLines, unifiedDiff } from "@/areas/code/text-diff";
import { sha, users, type Repository, type SimpleUser } from "../world";

// The git object store behind the code area: content-addressed blobs, commits whose tree
// is a flat path-to-blob map, and per-repository refs and commit statuses. Directory
// trees, diffs, blame, and history are derived from those, so every read agrees with
// every write.

export type Person = { user: SimpleUser | null; name: string; email: string };

export type Commit = {
  sha: string;
  message: string;
  author: Person;
  committer: Person;
  date: string;
  parents: string[];
  files: ReadonlyMap<string, string>;
  signed: boolean;
};

type Blob = { sha: string; b64: string; size: number; text: string | null };

type Status = Schemas["status"];

export type CodeRepo = {
  repoId: number;
  commits: Map<string, Commit>;
  branches: Map<string, string>;
  tags: Map<string, string>;
  // Annotated tag objects by their own sha. refs/tags/<name> points at one of these.
  tagObjects: Map<string, Schemas["git-tag"]>;
  protectedBranches: Set<string>;
  statuses: Map<string, Status[]>;
};

export function person(user: SimpleUser): Person {
  return { user, name: user.name ?? user.login, email: `${user.login}@graftyard.test` };
}

// ---- Blobs ----

const blobs = new Map<string, Blob>();

export function putBlob(b64: string): string {
  const clean = b64.replace(/\s+/g, "");
  const key = sha(`blob:${clean}`);
  if (!blobs.has(key)) {
    const text = decodeText(clean);
    const size = Math.floor((clean.length * 3) / 4) - (clean.match(/=+$/)?.[0].length ?? 0);
    blobs.set(key, { sha: key, b64: clean, size, text });
  }
  return key;
}

export const putText = (text: string) => putBlob(encodeText(text));
export const blob = (key: string) => blobs.get(key);
export const blobText = (key: string | undefined) => (key ? (blobs.get(key)?.text ?? null) : null);

// ---- Trees ----

export type TreeEntry = { name: string; path: string; type: "blob" | "tree"; sha: string };

// Every tree object anyone has seen, so GET /git/trees/{sha} can find it again.
const trees = new Map<string, { files: ReadonlyMap<string, string>; dir: string }>();
const treeShaMemo = new WeakMap<ReadonlyMap<string, string>, Map<string, string>>();

const under = (dir: string, path: string) => (dir === "" ? true : path.startsWith(`${dir}/`));
const rel = (dir: string, path: string) => (dir === "" ? path : path.slice(dir.length + 1));

export function isDir(files: ReadonlyMap<string, string>, dir: string) {
  if (dir === "") return files.size > 0;
  for (const p of files.keys()) if (p.startsWith(`${dir}/`)) return true;
  return false;
}

export function treeSha(files: ReadonlyMap<string, string>, dir = ""): string {
  let memo = treeShaMemo.get(files);
  if (!memo) treeShaMemo.set(files, (memo = new Map()));
  const hit = memo.get(dir);
  if (hit) return hit;
  const parts = [...files].filter(([p]) => under(dir, p)).map(([p, b]) => `${rel(dir, p)}=${b}`);
  const value = sha(`tree:${parts.sort().join("\n")}`);
  memo.set(dir, value);
  trees.set(value, { files, dir });
  return value;
}

export const findTree = (key: string) => trees.get(key);

// The immediate children of a directory, directories first, as git and GHES sort them.
export function listDir(files: ReadonlyMap<string, string>, dir: string): TreeEntry[] {
  const seen = new Map<string, TreeEntry>();
  for (const [path, b] of files) {
    if (!under(dir, path)) continue;
    const [name, ...rest] = rel(dir, path).split("/");
    const full = dir ? `${dir}/${name}` : name;
    if (seen.has(name)) continue;
    seen.set(
      name,
      rest.length
        ? { name, path: full, type: "tree", sha: treeSha(files, full) }
        : { name, path: full, type: "blob", sha: b },
    );
  }
  return [...seen.values()].sort(
    (a, b) => (a.type === b.type ? 0 : a.type === "tree" ? -1 : 1) || a.name.localeCompare(b.name),
  );
}

// ---- Commits ----

let generated = 0;

export function makeCommit(
  code: CodeRepo,
  input: Omit<Commit, "sha" | "signed"> & { sha?: string; signed?: boolean },
): Commit {
  const commit: Commit = {
    ...input,
    sha: input.sha ?? sha(`commit:${code.repoId}:${generated++}:${input.message}:${input.date}`),
    signed: input.signed ?? input.author.user?.login === "mira",
  };
  code.commits.set(commit.sha, commit);
  return commit;
}

// Every ancestor of `tip`, newest first by commit date.
export function history(code: CodeRepo, tip: string): Commit[] {
  const seen = new Set<string>();
  const out: Commit[] = [];
  const stack = [tip];
  while (stack.length) {
    const key = stack.pop()!;
    if (seen.has(key)) continue;
    seen.add(key);
    const c = code.commits.get(key);
    if (!c) continue;
    out.push(c);
    stack.push(...c.parents);
  }
  return out.sort((a, b) => b.date.localeCompare(a.date));
}

function pathValue(files: ReadonlyMap<string, string>, path: string): string | undefined {
  if (path === "") return treeSha(files);
  return files.get(path) ?? (isDir(files, path) ? treeSha(files, path) : undefined);
}

// True when `commit` changed `path` relative to every parent, git's simplified history.
export function touches(code: CodeRepo, commit: Commit, path: string): boolean {
  const mine = pathValue(commit.files, path);
  if (commit.parents.length === 0) return mine !== undefined;
  return commit.parents.every((p) => {
    const parent = code.commits.get(p);
    return !parent || pathValue(parent.files, path) !== mine;
  });
}

export function lastTouching(code: CodeRepo, tip: string, path: string): Commit | undefined {
  return history(code, tip).find((c) => touches(code, c, path));
}

// ---- Diffs ----

type DiffEntry = Schemas["diff-entry"];

const diffMemo = new Map<string, DiffEntry[]>();

export function commitFiles(code: CodeRepo, repo: Repository, commit: Commit): DiffEntry[] {
  const hit = diffMemo.get(commit.sha);
  if (hit) return hit;
  const before: ReadonlyMap<string, string> =
    code.commits.get(commit.parents[0] ?? "")?.files ?? new Map();
  const after = commit.files;
  const added = [...after.keys()].filter((p) => before.get(p) !== after.get(p) && !before.has(p));
  const removed = [...before.keys()].filter((p) => !after.has(p));
  const modified = [...after.keys()].filter((p) => before.has(p) && before.get(p) !== after.get(p));
  const renames = new Map<string, string>();
  for (const a of added) {
    const from = removed.find(
      (r) => before.get(r) === after.get(a) && ![...renames.values()].includes(r),
    );
    if (from) renames.set(a, from);
  }
  const entry = (filename: string, status: DiffEntry["status"], from?: string): DiffEntry => {
    const oldBlob = before.get(from ?? filename);
    const newBlob = after.get(filename);
    const oldText = oldBlob ? blobText(oldBlob) : "";
    const newText = newBlob ? blobText(newBlob) : "";
    const text = oldText !== null && newText !== null;
    const d = text ? unifiedDiff(oldText, newText) : undefined;
    const ref = status === "removed" ? commit.parents[0] : commit.sha;
    return {
      sha: newBlob ?? oldBlob ?? null,
      filename,
      status,
      additions: d?.additions ?? 0,
      deletions: d?.deletions ?? 0,
      changes: (d?.additions ?? 0) + (d?.deletions ?? 0),
      blob_url: `${repo.html_url}/blob/${ref}/${filename}`,
      raw_url: `${repo.html_url}/raw/${ref}/${filename}`,
      contents_url: `${repo.url}/contents/${filename}?ref=${ref}`,
      ...(d && d.patch ? { patch: d.patch } : {}),
      ...(from ? { previous_filename: from } : {}),
    };
  };
  const out = [
    ...modified.map((p) => entry(p, "modified")),
    ...added.map((p) => (renames.has(p) ? entry(p, "renamed", renames.get(p)) : entry(p, "added"))),
    ...removed.filter((p) => ![...renames.values()].includes(p)).map((p) => entry(p, "removed")),
  ].sort((a, b) => a.filename.localeCompare(b.filename));
  diffMemo.set(commit.sha, out);
  return out;
}

// ---- Blame ----

export type BlameLine = { commit: Commit };

export function blame(code: CodeRepo, tip: Commit, path: string): Commit[] | undefined {
  const text = blobText(tip.files.get(path));
  if (text === null) return undefined;
  const lines = splitLines(text);
  const owner: Commit[] = new Array(lines.length);
  // `map` takes a line of this commit's version to a line of the final file.
  const walk = (commit: Commit, version: string[], map: Map<number, number>) => {
    const parents = commit.parents
      .map((p) => code.commits.get(p))
      .filter((p): p is Commit => !!p && p.files.has(path));
    const same = parents.find((p) => p.files.get(path) === commit.files.get(path));
    if (same) return walk(same, version, map);
    const left = new Map(map);
    for (const parent of parents) {
      if (left.size === 0) break;
      const old = splitLines(blobText(parent.files.get(path)) ?? "");
      const next = new Map<number, number>();
      for (const [oi, ni] of matchLines(old, version)) {
        const final = left.get(ni);
        if (final === undefined) continue;
        next.set(oi, final);
        left.delete(ni);
      }
      if (next.size) walk(parent, old, next);
    }
    for (const final of left.values()) owner[final] = commit;
  };
  walk(tip, lines, new Map(lines.map((_, i) => [i, i])));
  return owner;
}

// ---- The REST shapes ----

const isoNoMs = (d: string) => d.replace(/\.\d{3}Z$/, "Z");

function verification(c: Commit): Schemas["verification"] {
  return c.signed
    ? {
        verified: true,
        reason: "valid",
        signature: "-----BEGIN SSH SIGNATURE-----\nU1NIU0lHAAAAAQ\n-----END SSH SIGNATURE-----",
        payload: `tree ${treeSha(c.files)}\nauthor ${c.author.name} <${c.author.email}>`,
        verified_at: isoNoMs(c.date),
      }
    : { verified: false, reason: "unsigned", signature: null, payload: null, verified_at: null };
}

export function restCommit(
  code: CodeRepo,
  repo: Repository,
  c: Commit,
  withFiles = false,
): Schemas["commit"] {
  const files = withFiles ? commitFiles(code, repo, c) : undefined;
  const additions = files?.reduce((n, f) => n + f.additions, 0) ?? 0;
  const deletions = files?.reduce((n, f) => n + f.deletions, 0) ?? 0;
  const tree = treeSha(c.files);
  return {
    url: `${repo.url}/commits/${c.sha}`,
    sha: c.sha,
    node_id: `C_${c.sha.slice(0, 12)}`,
    html_url: `${repo.html_url}/commit/${c.sha}`,
    comments_url: `${repo.url}/commits/${c.sha}/comments`,
    commit: {
      url: `${repo.url}/git/commits/${c.sha}`,
      author: { name: c.author.name, email: c.author.email, date: isoNoMs(c.date) },
      committer: { name: c.committer.name, email: c.committer.email, date: isoNoMs(c.date) },
      message: c.message,
      comment_count: 0,
      tree: { sha: tree, url: `${repo.url}/git/trees/${tree}` },
      verification: verification(c),
    },
    author: c.author.user,
    committer: c.committer.user,
    parents: c.parents.map((p) => ({
      sha: p,
      url: `${repo.url}/commits/${p}`,
      html_url: `${repo.html_url}/commit/${p}`,
    })),
    ...(files ? { stats: { additions, deletions, total: additions + deletions }, files } : {}),
  };
}

export function gitCommit(repo: Repository, c: Commit): Schemas["git-commit"] {
  const tree = treeSha(c.files);
  const v = verification(c);
  return {
    sha: c.sha,
    node_id: `C_${c.sha.slice(0, 12)}`,
    url: `${repo.url}/git/commits/${c.sha}`,
    html_url: `${repo.html_url}/commit/${c.sha}`,
    author: { date: isoNoMs(c.date), name: c.author.name, email: c.author.email },
    committer: { date: isoNoMs(c.date), name: c.committer.name, email: c.committer.email },
    message: c.message,
    tree: { sha: tree, url: `${repo.url}/git/trees/${tree}` },
    parents: c.parents.map((p) => ({
      sha: p,
      url: `${repo.url}/git/commits/${p}`,
      html_url: `${repo.html_url}/commit/${p}`,
    })),
    verification: {
      verified: v.verified,
      reason: v.reason,
      signature: v.signature,
      payload: v.payload,
      verified_at: v.verified_at,
    },
  };
}

type Links = { git: string | null; html: string | null; self: string };

function contentBase(repo: Repository, ref: string, path: string, type: string, key: string) {
  const self = `${repo.url}/contents/${path}?ref=${encodeURIComponent(ref)}`;
  const git = `${repo.url}/git/${type === "dir" ? "trees" : "blobs"}/${key}`;
  const html = `${repo.html_url}/${type === "dir" ? "tree" : "blob"}/${ref}/${path}`;
  const links: Links = { git, html, self };
  return { url: self, git_url: git, html_url: html, _links: links };
}

export function contentFile(
  repo: Repository,
  ref: string,
  path: string,
  key: string,
): Schemas["content-file"] {
  const b = blobs.get(key)!;
  return {
    type: "file",
    encoding: "base64",
    size: b.size,
    name: path.split("/").at(-1)!,
    path,
    content: wrap60(b.b64),
    sha: key,
    download_url: `https://raw.ghes.graftyard.test/${repo.full_name}/${ref}/${path}`,
    ...contentBase(repo, ref, path, "file", key),
  };
}

export function contentDirectory(
  repo: Repository,
  ref: string,
  files: ReadonlyMap<string, string>,
  dir: string,
): Schemas["content-directory"] {
  return listDir(files, dir).map((e) => {
    const type = e.type === "tree" ? ("dir" as const) : ("file" as const);
    return {
      type,
      size: type === "dir" ? 0 : (blobs.get(e.sha)?.size ?? 0),
      name: e.name,
      path: e.path,
      sha: e.sha,
      download_url:
        type === "dir"
          ? null
          : `https://raw.ghes.graftyard.test/${repo.full_name}/${ref}/${e.path}`,
      ...contentBase(repo, ref, e.path, type, e.sha),
    };
  });
}

export function gitBlob(repo: Repository, key: string): Schemas["blob"] | undefined {
  const b = blobs.get(key);
  return (
    b && {
      content: wrap60(b.b64),
      encoding: "base64",
      url: `${repo.url}/git/blobs/${key}`,
      sha: key,
      size: b.size,
      node_id: `B_${key.slice(0, 12)}`,
    }
  );
}

export function gitTree(
  repo: Repository,
  files: ReadonlyMap<string, string>,
  dir: string,
  recursive: boolean,
): Schemas["git-tree"] {
  const entries: Schemas["git-tree"]["tree"] = [];
  const visit = (d: string) => {
    for (const e of listDir(files, d)) {
      const path = dir ? e.path.slice(dir.length + 1) : e.path;
      entries.push(
        e.type === "tree"
          ? {
              path,
              mode: "040000",
              type: "tree",
              sha: e.sha,
              url: `${repo.url}/git/trees/${e.sha}`,
            }
          : {
              path,
              mode: path.endsWith(".sh") ? "100755" : "100644",
              type: "blob",
              sha: e.sha,
              size: blobs.get(e.sha)?.size ?? 0,
              url: `${repo.url}/git/blobs/${e.sha}`,
            },
      );
      if (recursive && e.type === "tree") visit(e.path);
    }
  };
  visit(dir);
  const key = treeSha(files, dir);
  return { sha: key, url: `${repo.url}/git/trees/${key}`, truncated: false, tree: entries };
}

export function gitRef(
  repo: Repository,
  ref: string,
  target: string,
  type = "commit",
): Schemas["git-ref"] {
  return {
    ref,
    node_id: `REF_${sha(`${repo.full_name}:${ref}`).slice(0, 12)}`,
    url: `${repo.url}/git/${ref}`,
    object: {
      type,
      sha: target,
      url: `${repo.url}/git/${type === "tag" ? "tags" : "commits"}/${target}`,
    },
  };
}

// ---- Refs ----

// The commit a ref points at: a branch, a tag (peeled), or a full or abbreviated sha.
export function resolveRef(
  code: CodeRepo,
  ref: string,
): { kind: "branch" | "tag" | "commit"; sha: string } | undefined {
  const branch = code.branches.get(ref.replace(/^refs\/heads\//, ""));
  if (branch) return { kind: "branch", sha: branch };
  const tag = code.tags.get(ref.replace(/^refs\/tags\//, ""));
  if (tag) return { kind: "tag", sha: peel(code, tag) };
  if (/^[0-9a-f]{7,40}$/.test(ref)) {
    const hit = [...code.commits.keys()].find((k) => k.startsWith(ref));
    if (hit) return { kind: "commit", sha: hit };
  }
  return undefined;
}

export const peel = (code: CodeRepo, target: string) =>
  code.tagObjects.get(target)?.object.sha ?? target;

// A splat such as "mira/idempotent-retry/crates/wire" splits at the longest ref prefix.
export function splitSplat(code: CodeRepo, splat: string) {
  const parts = splat.split("/").filter(Boolean);
  for (let n = parts.length; n >= 1; n--) {
    const ref = parts.slice(0, n).join("/");
    const hit = resolveRef(code, ref);
    if (hit) return { ref, path: parts.slice(n).join("/"), ...hit };
  }
  return undefined;
}

export function shortBranch(
  code: CodeRepo,
  repo: Repository,
  name: string,
): Schemas["short-branch"] {
  const target = code.branches.get(name)!;
  const isProtected = code.protectedBranches.has(name);
  return {
    name,
    commit: { sha: target, url: `${repo.url}/commits/${target}` },
    protected: isProtected,
    protection: protection(repo, name, isProtected),
    protection_url: `${repo.url}/branches/${encodeURIComponent(name)}/protection`,
  };
}

function protection(repo: Repository, name: string, on: boolean): Schemas["branch-protection"] {
  const url = `${repo.url}/branches/${encodeURIComponent(name)}/protection`;
  return on
    ? {
        url,
        enabled: true,
        required_status_checks: {
          enforcement_level: "non_admins",
          contexts: ["test (ubuntu-latest)", "rustfmt"],
          checks: [
            { context: "test (ubuntu-latest)", app_id: 15368 },
            { context: "rustfmt", app_id: 15368 },
          ],
        },
        allow_deletions: { enabled: false },
        allow_force_pushes: { enabled: false },
        required_linear_history: { enabled: true },
      }
    : {
        enabled: false,
        required_status_checks: { enforcement_level: "off", contexts: [], checks: [] },
      };
}

export function branchWithProtection(
  code: CodeRepo,
  repo: Repository,
  name: string,
): Schemas["branch-with-protection"] {
  const target = code.commits.get(code.branches.get(name)!)!;
  const isProtected = code.protectedBranches.has(name);
  return {
    name,
    commit: restCommit(code, repo, target),
    _links: {
      html: `${repo.html_url}/tree/${name}`,
      self: `${repo.url}/branches/${encodeURIComponent(name)}`,
    },
    protected: isProtected,
    protection: protection(repo, name, isProtected),
    protection_url: `${repo.url}/branches/${encodeURIComponent(name)}/protection`,
  };
}

export const VALID_REF =
  /^(?!\/|.*(?:\/\.|\/\/|\.\.|@\{|\\))[^\0-\x20~^:?*[\x7f]+(?<![./]|\.lock)$/;

// ---- Statuses ----

let statusId = 5_000_000;

export function addStatus(
  code: CodeRepo,
  repo: Repository,
  target: string,
  input: { state: string; context: string; description: string | null; target_url: string | null },
  creator: SimpleUser,
  at: string,
): Status {
  const id = statusId++;
  const status: Status = {
    url: `${repo.url}/statuses/${target}`,
    avatar_url: creator.avatar_url,
    id,
    node_id: `SC_${sha(`${id}`).slice(0, 12)}`,
    state: input.state,
    description: input.description,
    target_url: input.target_url,
    context: input.context,
    created_at: at,
    updated_at: at,
    creator,
  };
  code.statuses.set(target, [status, ...(code.statuses.get(target) ?? [])]);
  return status;
}

export function combinedStatus(
  code: CodeRepo,
  repo: Repository,
  target: string,
): Schemas["combined-commit-status"] {
  const latest = new Map<string, Status>();
  for (const s of code.statuses.get(target) ?? [])
    if (!latest.has(s.context)) latest.set(s.context, s);
  const list = [...latest.values()];
  const state = list.some((s) => s.state === "error" || s.state === "failure")
    ? "failure"
    : list.length === 0 || list.some((s) => s.state === "pending")
      ? "pending"
      : "success";
  const { license, ...rest } = repo;
  return {
    state,
    statuses: list.map(({ creator, ...s }) => ({ ...s, required: s.context.startsWith("ci/") })),
    sha: target,
    total_count: list.length,
    repository: {
      ...rest,
      license: license && {
        key: license.key,
        name: license.name,
        url: license.url,
        node_id: license.node_id,
        ...(license.spdx_id ? { spdx_id: license.spdx_id } : {}),
      },
    },
    commit_url: `${repo.url}/commits/${target}`,
    url: `${repo.url}/commits/${target}/status`,
  };
}

export const viewer = users.mira;
