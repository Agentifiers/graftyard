import type { Schemas } from "@/api/client";
import { negatedValues, parseQuery, values, type ParsedQuery } from "@/areas/activity/search-query";
import { API, branches, me, sha, users, type Repository } from "../world";
import { LABELS, db as core, label } from "./core-db";
import { minimal, people, profiles } from "./activity-db";

// GHES search over every fixture the shared store and world.ts expose. Each `search*`
// takes a parsed query and returns scored items with text_matches, unsorted; the
// handler sorts, counts, and pages them.

type TextMatches = Schemas["search-result-text-matches"];

// One text_matches entry: the fragment around every term and the indices of each hit.
function match(
  objectUrl: string,
  objectType: string,
  property: string,
  text: string,
  terms: string[],
): TextMatches[number] | undefined {
  if (!terms.length || !text) return undefined;
  const lower = text.toLowerCase();
  const first = Math.min(...terms.map((t) => lower.indexOf(t.toLowerCase())).filter((i) => i >= 0));
  if (!Number.isFinite(first)) return undefined;
  const start = Math.max(0, first - 60);
  const fragment = text.slice(start, start + 200);
  const fl = fragment.toLowerCase();
  const matches: { text: string; indices: number[] }[] = [];
  for (const t of terms) {
    const tl = t.toLowerCase();
    for (let i = fl.indexOf(tl); i >= 0; i = fl.indexOf(tl, i + tl.length)) {
      matches.push({ text: fragment.slice(i, i + t.length), indices: [i, i + t.length] });
    }
  }
  return matches.length
    ? { object_url: objectUrl, object_type: objectType, property, fragment, matches }
    : undefined;
}

function textMatches(
  url: string,
  type: string,
  fields: [property: string, text: string | null | undefined][],
  terms: string[],
): TextMatches {
  return fields.flatMap(([p, t]) => {
    const m = match(url, type, p, t ?? "", terms);
    return m ? [m] : [];
  });
}

// Every term appears in one of the fields. No terms matches everything.
function hits(fields: (string | null | undefined)[], terms: string[]) {
  const hay = fields.join("\n").toLowerCase();
  return terms.every((t) => hay.includes(t.toLowerCase()));
}

// A crude relevance score: term hits in the first field count most.
function score(fields: (string | null | undefined)[], terms: string[]) {
  return terms.reduce(
    (s, t) =>
      s +
      fields.reduce(
        (f, x, i) => f + ((x ?? "").toLowerCase().includes(t.toLowerCase()) ? 1 / (i + 1) : 0),
        0,
      ),
    1,
  );
}

function repoFilter(p: ParsedQuery) {
  const wanted = values(p, "repo");
  const owners = [...values(p, "org"), ...values(p, "user")];
  return (r: { full_name: string; owner: { login: string } | null }) =>
    (!wanted.length || wanted.includes(r.full_name.toLowerCase())) &&
    (!owners.length || owners.includes(r.owner?.login.toLowerCase() ?? ""));
}

// `stars:>100`, `stars:10..50`, `stars:<=5`.
function numberQualifier(spec: string | undefined, n: number) {
  if (!spec) return true;
  const range = spec.match(/^(\d+)\.\.(\d+)$/);
  if (range) return n >= Number(range[1]) && n <= Number(range[2]);
  const cmp = spec.match(/^(>=|<=|>|<)?(\d+)$/);
  if (!cmp) return true;
  const v = Number(cmp[2]);
  return { ">": n > v, ">=": n >= v, "<": n < v, "<=": n <= v, "": n === v }[cmp[1] ?? ""];
}

// ---- Repositories ----

export function searchRepositories(p: ParsedQuery): Schemas["repo-search-result-item"][] {
  const inRepo = repoFilter(p);
  const langs = values(p, "language");
  const topics = values(p, "topic");
  const visibility = values(p, "is");
  const stars = values(p, "stars")[0];
  return core
    .repos()
    .filter(
      (r) =>
        inRepo(r) &&
        (!langs.length || langs.includes((r.language ?? "").toLowerCase())) &&
        topics.every((t) => r.topics?.includes(t)) &&
        visibility.every((v) => v === r.visibility || (v === "private" && r.private)) &&
        numberQualifier(stars, r.stargazers_count) &&
        hits([r.name, r.description, r.topics?.join(" ")], p.terms),
    )
    .map((r) => repoItem(r, p.terms));
}

function repoItem(r: Repository, terms: string[]): Schemas["repo-search-result-item"] {
  const { temp_clone_token, anonymous_access_enabled, ...rest } = r;
  return {
    ...rest,
    created_at: r.created_at ?? "",
    updated_at: r.updated_at ?? "",
    pushed_at: r.pushed_at ?? "",
    score: score([r.name, r.description], terms),
    text_matches: textMatches(
      r.url,
      "Repository",
      [
        ["name", r.name],
        ["description", r.description],
      ],
      terms,
    ),
  };
}

// ---- Issues and pull requests ----

export function searchIssues(p: ParsedQuery): Schemas["issue-search-result-item"][] {
  const inRepo = repoFilter(p);
  const is = values(p, "is");
  const notIs = negatedValues(p, "is");
  const kinds = [...is, ...values(p, "type")];
  const labels = values(p, "label");
  const notLabels = negatedValues(p, "label");
  const authors = values(p, "author").map((a) => (a === "@me" ? me.login : a));
  const assignees = values(p, "assignee");
  const states = [...is.filter((v) => v === "open" || v === "closed"), ...values(p, "state")];
  const out: Schemas["issue-search-result-item"][] = [];
  for (const state of core.repos().map((r) => core.state(r.owner.login, r.name)!)) {
    if (!inRepo(state.repo)) continue;
    for (const i of state.issues) {
      const isPull = Boolean(i.pull_request);
      const pr = isPull ? state.pulls.find((x) => x.number === i.number) : undefined;
      const names = i.labels.map((l) => (typeof l === "string" ? l : (l.name ?? "")).toLowerCase());
      const ok =
        (!kinds.includes("pr") || isPull) &&
        (!kinds.includes("issue") || !isPull) &&
        (!is.includes("merged") || Boolean(pr?.merged_at)) &&
        (!is.includes("draft") || Boolean(pr?.draft)) &&
        (!notIs.includes("draft") || !pr?.draft) &&
        (!states.length || states.includes(i.state)) &&
        notIs.every((v) => v !== i.state) &&
        labels.every((l) => names.includes(l)) &&
        notLabels.every((l) => !names.includes(l)) &&
        (!authors.length || authors.includes(i.user?.login.toLowerCase() ?? "")) &&
        (!assignees.length ||
          assignees.some((a) => i.assignees?.some((x) => x.login.toLowerCase() === a))) &&
        (hits([i.title, i.body], p.terms) ||
          (p.terms.length === 1 && p.terms[0].replace(/^#/, "") === String(i.number)));
      if (!ok) continue;
      const { type, ...rest } = i;
      out.push({
        ...rest,
        labels: i.labels.map((l) => {
          const full = typeof l === "string" ? label(state.repo, l) : l;
          return { ...full, color: full.color ?? undefined };
        }),
        state_reason: i.state_reason ?? null,
        body: i.body ?? undefined,
        user: i.user,
        repository: state.repo,
        score: score([i.title, i.body], p.terms),
        author_association: i.author_association ?? "NONE",
        text_matches: textMatches(
          i.url,
          "Issue",
          [
            ["title", i.title],
            ["body", i.body],
          ],
          p.terms,
        ),
      });
    }
  }
  return out;
}

// ---- Users ----

export function searchUsers(p: ParsedQuery): Schemas["user-search-result-item"][] {
  const types = values(p, "type");
  const locations = values(p, "location");
  const followers = values(p, "followers")[0];
  return [...people, users.org]
    .filter((u) => {
      const prof = profiles[u.login];
      return (
        (!types.length || types.includes(u.type.toLowerCase())) &&
        (!locations.length || locations.some((l) => prof?.location.toLowerCase().includes(l))) &&
        numberQualifier(followers, prof?.followers ?? 0) &&
        hits([u.login, u.name, prof?.bio], p.terms)
      );
    })
    .map((u) => {
      const prof = profiles[u.login];
      const repos = core.repos().filter((r) => r.owner.login === u.login).length;
      return {
        ...u,
        name: u.name ?? null,
        bio: prof?.bio ?? null,
        location: prof?.location ?? null,
        followers: prof?.followers ?? 40,
        public_repos: repos,
        created_at: "2025-03-02T09:00:00Z",
        score: score([u.login, u.name, prof?.bio], p.terms),
        text_matches: textMatches(
          u.url,
          "User",
          [
            ["login", u.login],
            ["name", u.name],
            ["bio", prof?.bio],
          ],
          p.terms,
        ),
      };
    });
}

// ---- Labels (one repository, by repository_id) ----

export function searchLabels(
  repositoryId: number,
  p: ParsedQuery,
): Schemas["label-search-result-item"][] | undefined {
  const repo = core.repos().find((r) => r.id === repositoryId);
  if (!repo) return undefined;
  return Object.keys(LABELS)
    .map((name) => label(repo, name))
    .filter((l) => hits([l.name, l.description], p.terms))
    .map((l) => ({
      ...l,
      archived_at: null,
      score: score([l.name, l.description], p.terms),
      text_matches: textMatches(
        l.url,
        "Label",
        [
          ["name", l.name],
          ["description", l.description],
        ],
        p.terms,
      ),
    }));
}

// ---- Code: the files the pull requests touch, with the head-side text ----

const EXT: Record<string, string> = {
  rs: "Rust",
  ts: "TypeScript",
  tsx: "TypeScript",
  md: "Markdown",
};

export function searchCode(p: ParsedQuery): Schemas["code-search-result-item"][] {
  const inRepo = repoFilter(p);
  const langs = values(p, "language");
  const paths = values(p, "path");
  const exts = values(p, "extension");
  const out: Schemas["code-search-result-item"][] = [];
  const seen = new Set<string>();
  for (const state of core.repos().map((r) => core.state(r.owner.login, r.name)!)) {
    if (!inRepo(state.repo)) continue;
    for (const pr of state.pulls) {
      for (const f of state.files.get(pr.number) ?? []) {
        const k = `${state.repo.full_name}:${f.filename}`;
        if (seen.has(k) || f.status === "removed") continue;
        // The head side of the patch: context lines and added lines.
        const lines = (f.patch ?? "")
          .split("\n")
          .filter((l) => !l.startsWith("-") && !l.startsWith("@@"))
          .map((l) => l.slice(1));
        const text = lines.join("\n");
        const name = f.filename.split("/").at(-1)!;
        const ext = name.split(".").at(-1)!.toLowerCase();
        const language = EXT[ext] ?? null;
        if (langs.length && !langs.includes((language ?? "").toLowerCase())) continue;
        if (paths.length && !paths.some((x) => f.filename.toLowerCase().includes(x))) continue;
        if (exts.length && !exts.includes(ext)) continue;
        if (!p.terms.length || !hits([text, f.filename], p.terms)) continue;
        seen.add(k);
        const lineNumbers = lines
          .map((l, i) =>
            p.terms.some((t) => l.toLowerCase().includes(t.toLowerCase())) ? String(i + 1) : "",
          )
          .filter(Boolean);
        out.push({
          name,
          path: f.filename,
          sha: f.sha ?? "",
          url: `${state.repo.url}/contents/${f.filename}?ref=${pr.head.sha}`,
          git_url: `${state.repo.url}/git/blobs/${f.sha ?? ""}`,
          html_url: `${state.repo.html_url}/blob/${pr.head.sha}/${f.filename}`,
          repository: minimal(state.repo),
          score: score([text, f.filename], p.terms),
          file_size: text.length,
          language,
          last_modified_at: pr.updated_at,
          line_numbers: lineNumbers,
          text_matches: textMatches(
            `${state.repo.url}/contents/${f.filename}`,
            "FileContent",
            [["content", text]],
            p.terms,
          ),
        });
      }
    }
  }
  return out;
}

// ---- Commits: each branch's head commit, with the message its PR or branch implies ----

export function searchCommits(p: ParsedQuery): Schemas["commit-search-result-item"][] {
  const inRepo = repoFilter(p);
  const authors = values(p, "author");
  const out: Schemas["commit-search-result-item"][] = [];
  for (const repo of core.repos()) {
    if (!inRepo(repo)) continue;
    const state = core.state(repo.owner.login, repo.name)!;
    for (const branch of branches[repo.full_name] ?? ["main"]) {
      const pr = state.pulls.find((x) => x.head.ref === branch);
      const author = pr?.user ?? users.mira;
      const message = pr
        ? `${pr.title}\n\n${pr.body ?? ""}`.trim()
        : branch === "main"
          ? `Merge the latest into ${repo.name}`
          : `Start ${branch}`;
      const date = pr?.updated_at ?? repo.pushed_at ?? repo.created_at ?? "";
      if (authors.length && !authors.includes(author.login.toLowerCase())) continue;
      if (!hits([message], p.terms)) continue;
      const commitSha = sha(`${repo.full_name}:${branch}`);
      const url = `${repo.url}/commits/${commitSha}`;
      const git = {
        name: author.name ?? author.login,
        email: `${author.login}@graftyard.test`,
        date,
      };
      out.push({
        url,
        sha: commitSha,
        html_url: `${repo.html_url}/commit/${commitSha}`,
        comments_url: `${url}/comments`,
        commit: {
          author: git,
          committer: git,
          comment_count: 0,
          message,
          tree: {
            sha: sha(`tree:${commitSha}`),
            url: `${repo.url}/git/trees/${sha(`tree:${commitSha}`)}`,
          },
          url: `${repo.url}/git/commits/${commitSha}`,
          verification: {
            verified: true,
            reason: "valid",
            payload: null,
            signature: null,
            verified_at: date,
          },
        },
        author,
        committer: git,
        parents: [
          {
            sha: sha(`${repo.full_name}:main`),
            url: `${API}/repos/${repo.full_name}/commits/${sha(`${repo.full_name}:main`)}`,
          },
        ],
        repository: minimal(repo),
        score: score([message], p.terms),
        node_id: `C_${commitSha.slice(0, 12)}`,
        text_matches: textMatches(url, "Commit", [["message", message]], p.terms),
      });
    }
  }
  return out;
}

// ---- Sorting ----

type Order = "asc" | "desc";

export function sortBy<T>(items: T[], key: ((x: T) => number | string) | undefined, order: Order) {
  if (!key) return items;
  const dir = order === "asc" ? 1 : -1;
  return [...items].sort((a, b) => {
    const x = key(a);
    const y = key(b);
    return (x < y ? -1 : x > y ? 1 : 0) * dir;
  });
}

export { parseQuery };
