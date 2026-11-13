import { createOpenApiHttp } from "openapi-msw";
import { HttpResponse } from "msw";
import { decodeText } from "@/areas/code/base64";
import { UI_BASE, type CommitSummary, type UiPaths } from "@/areas/code/ui-api";
import { gate, http, notFound, page, unprocessable } from "../http";
import { sha, users } from "../world";
import {
  addStatus,
  blame,
  blob,
  branchWithProtection,
  combinedStatus,
  contentDirectory,
  contentFile,
  findTree,
  gitBlob,
  gitCommit,
  gitRef,
  gitTree,
  history,
  isDir,
  lastTouching,
  listDir,
  makeCommit,
  peel,
  person,
  putBlob,
  resolveRef,
  restCommit,
  shortBranch,
  splitSplat,
  touches,
  VALID_REF,
  type CodeRepo,
  type Commit,
} from "./code-db";
import { codeRepo } from "./code-repos";
import {
  codeFrequency,
  commitActivity,
  contributors,
  contributorStats,
  languages,
  participation,
  punchCard,
  traffic,
} from "./code-stats";

// The code browser's handlers: contents, git objects, commits, refs, statuses, and stats,
// plus the private /_t/ui reads. Every write changes the store in code-db.ts, so the
// reads that follow show it.

const ui = createOpenApiHttp<UiPaths>({ baseUrl: UI_BASE });

const conflict = (message: string) =>
  HttpResponse.json(
    {
      message,
      documentation_url: "https://docs.github.com/enterprise-server@3.22/rest",
      status: "409",
    },
    { status: 409 },
  );

const now = () => new Date().toISOString().replace(/\.\d{3}Z$/, "Z");

function summary(c: Commit): CommitSummary {
  return {
    sha: c.sha,
    message: c.message,
    date: c.date,
    author: {
      login: c.author.user?.login ?? null,
      name: c.author.name,
      avatar_url: c.author.user?.avatar_url ?? null,
    },
  };
}

function at(code: CodeRepo, ref: string | null, fallback: string) {
  const hit = resolveRef(code, ref || fallback);
  return hit && code.commits.get(hit.sha);
}

function findReadme(params: { owner: string; repo: string }, ref: string | null) {
  const r = codeRepo(params.owner, params.repo);
  const c = r && at(r.code, ref, r.repo.default_branch ?? "main");
  const path = c && [...c.files.keys()].find((p) => /^readme(\.md|\.markdown|\.txt)?$/i.test(p));
  if (!r || !c || !path) return undefined;
  return { r, path, key: c.files.get(path)!, ref: ref || (r.repo.default_branch ?? "main") };
}

// The changes PUT and DELETE contents write as one new commit on a branch.
function commitTo(
  code: CodeRepo,
  branch: string,
  message: string,
  change: (files: Map<string, string>) => void,
) {
  const parent = code.commits.get(code.branches.get(branch)!)!;
  const files = new Map(parent.files);
  change(files);
  const me = person(users.mira);
  const c = makeCommit(code, {
    message,
    author: me,
    committer: me,
    date: now(),
    parents: [parent.sha],
    files,
  });
  code.branches.set(branch, c.sha);
  return c;
}

function fileCommit(code: CodeRepo, repo: Parameters<typeof restCommit>[1], c: Commit) {
  const rest = restCommit(code, repo, c);
  return {
    sha: c.sha,
    node_id: rest.node_id,
    url: rest.commit.url,
    html_url: rest.html_url,
    author: { date: c.date, name: c.author.name, email: c.author.email },
    committer: { date: c.date, name: c.committer.name, email: c.committer.email },
    message: c.message,
    tree: rest.commit.tree,
    parents: rest.parents,
    verification: {
      verified: c.signed,
      reason: c.signed ? "valid" : "unsigned",
      signature: null,
      payload: null,
      verified_at: c.signed ? c.date : null,
    },
  };
}

export const handlers = [
  // ---- Contents ----

  http.get("/repos/{owner}/{repo}/readme", async ({ params, query, response }) => {
    const fault = await gate("code-readme");
    if (fault) return response.untyped(fault);
    const hit = findReadme(params, query.get("ref"));
    if (!hit) return response.untyped(notFound());
    return response(200).json(contentFile(hit.r.repo, hit.ref, hit.path, hit.key));
  }),

  http.get("/repos/{owner}/{repo}/contents/{path}", async ({ params, query, response }) => {
    const fault = await gate("code-contents");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    const ref = query.get("ref") || (r?.repo.default_branch ?? "main");
    const c = r && at(r.code, ref, ref);
    if (!r || !c) return response.untyped(notFound());
    const path = params.path.replace(/^\/+|\/+$/g, "");
    const key = c.files.get(path);
    if (key) return response(200).json(contentFile(r.repo, ref, path, key));
    if (isDir(c.files, path))
      return response(200).json(contentDirectory(r.repo, ref, c.files, path));
    return response.untyped(notFound());
  }),

  http.put("/repos/{owner}/{repo}/contents/{path}", async ({ params, request, response }) => {
    const fault = await gate("code-put-file");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const body = await request.json();
    const path = params.path.replace(/^\/+|\/+$/g, "");
    const branch = body.branch || (r.repo.default_branch ?? "main");
    if (!body.message?.trim())
      return response.untyped(unprocessable("Commit", "message", "missing_field"));
    if (!path || path.split("/").some((s) => s === "" || s === "." || s === ".." || s === ".git")) {
      return response.untyped(unprocessable("Content", "path", "invalid"));
    }
    if (!r.code.branches.has(branch))
      return response.untyped(unprocessable("Commit", "branch", "invalid"));
    if (r.code.protectedBranches.has(branch) && path.startsWith(".github/workflows/")) {
      return response.untyped(
        conflict(`Changes to workflows on ${branch} must go through a pull request.`),
      );
    }
    const head = r.code.commits.get(r.code.branches.get(branch)!)!;
    const existing = head.files.get(path);
    if (isDir(head.files, path))
      return response.untyped(unprocessable("Content", "path", "already_exists"));
    if (existing && body.sha !== existing) {
      return response.untyped(
        existing && body.sha
          ? conflict(`${path} does not match ${body.sha}`)
          : unprocessable("Content", "sha", body.sha ? "invalid" : "missing_field"),
      );
    }
    if (!existing && body.sha) return response.untyped(unprocessable("Content", "sha", "invalid"));
    let key: string;
    try {
      key = putBlob(body.content);
    } catch {
      return response.untyped(unprocessable("Content", "content", "invalid"));
    }
    if (key === existing) return response.untyped(unprocessable("Commit", "content", "no_change"));
    const c = commitTo(r.code, branch, body.message.trim(), (files) => files.set(path, key));
    const file = contentFile(r.repo, branch, path, key);
    const payload = {
      content: {
        name: file.name,
        path: file.path,
        sha: file.sha,
        size: file.size,
        url: file.url,
        html_url: file.html_url ?? undefined,
        git_url: file.git_url ?? undefined,
        download_url: file.download_url ?? undefined,
        type: file.type,
        _links: {
          self: file._links.self,
          git: file._links.git ?? undefined,
          html: file._links.html ?? undefined,
        },
      },
      commit: fileCommit(r.code, r.repo, c),
    };
    return existing ? response(200).json(payload) : response(201).json(payload);
  }),

  http.delete("/repos/{owner}/{repo}/contents/{path}", async ({ params, request, response }) => {
    const fault = await gate("code-delete-file");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const body = await request.json();
    const path = params.path.replace(/^\/+|\/+$/g, "");
    const branch = body.branch || (r.repo.default_branch ?? "main");
    if (!body.message?.trim())
      return response.untyped(unprocessable("Commit", "message", "missing_field"));
    if (!r.code.branches.has(branch))
      return response.untyped(unprocessable("Commit", "branch", "invalid"));
    const head = r.code.commits.get(r.code.branches.get(branch)!)!;
    const existing = head.files.get(path);
    if (!existing) return response.untyped(notFound());
    if (body.sha !== existing)
      return response.untyped(conflict(`${path} does not match ${body.sha}`));
    const c = commitTo(r.code, branch, body.message.trim(), (files) => files.delete(path));
    return response(200).json({ content: null, commit: fileCommit(r.code, r.repo, c) });
  }),

  // ---- Git objects ----

  http.get("/repos/{owner}/{repo}/git/trees/{tree_sha}", async ({ params, query, response }) => {
    const fault = await gate("code-tree");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const recursive = Boolean(query.get("recursive"));
    const known = findTree(params.tree_sha);
    if (known) return response(200).json(gitTree(r.repo, known.files, known.dir, recursive));
    const c = at(r.code, params.tree_sha, params.tree_sha);
    return c
      ? response(200).json(gitTree(r.repo, c.files, "", recursive))
      : response.untyped(notFound());
  }),

  http.get("/repos/{owner}/{repo}/git/blobs/{file_sha}", async ({ params, response }) => {
    const fault = await gate("code-blob");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    const b = r && gitBlob(r.repo, params.file_sha);
    return b ? response(200).json(b) : response.untyped(notFound());
  }),

  http.post("/repos/{owner}/{repo}/git/blobs", async ({ params, request, response }) => {
    const fault = await gate("code-git-write");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const body = await request.json();
    const key =
      body.encoding === "base64"
        ? putBlob(body.content)
        : putBlob(btoa(unescape(encodeURIComponent(body.content))));
    return response(201).json({ url: `${r.repo.url}/git/blobs/${key}`, sha: key });
  }),

  http.get("/repos/{owner}/{repo}/git/commits/{commit_sha}", async ({ params, response }) => {
    const fault = await gate("code-commit");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    const c = r?.code.commits.get(params.commit_sha);
    return r && c ? response(200).json(gitCommit(r.repo, c)) : response.untyped(notFound());
  }),

  http.post("/repos/{owner}/{repo}/git/trees", async ({ params, request, response }) => {
    const fault = await gate("code-git-write");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const body = await request.json();
    const base = body.base_tree ? findTree(body.base_tree) : undefined;
    if (body.base_tree && !base)
      return response.untyped(unprocessable("Tree", "base_tree", "invalid"));
    const files = new Map(base?.files ?? []);
    for (const e of body.tree) {
      if (!e.path) return response.untyped(unprocessable("Tree", "path", "missing_field"));
      if (e.sha === null) files.delete(e.path);
      else if (e.content !== undefined)
        files.set(e.path, putBlob(btoa(unescape(encodeURIComponent(e.content)))));
      else if (e.sha && blob(e.sha)) files.set(e.path, e.sha);
      else return response.untyped(unprocessable("Tree", "sha", "invalid"));
    }
    return response(201).json(gitTree(r.repo, files, "", false));
  }),

  http.post("/repos/{owner}/{repo}/git/commits", async ({ params, request, response }) => {
    const fault = await gate("code-git-write");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const body = await request.json();
    const tree = findTree(body.tree);
    if (!tree || tree.dir !== "")
      return response.untyped(unprocessable("Commit", "tree", "invalid"));
    const parents = body.parents ?? [];
    if (parents.some((p) => !r.code.commits.has(p))) {
      return response.untyped(unprocessable("Commit", "parents", "invalid"));
    }
    const me = person(users.mira);
    const author = body.author
      ? { user: null, name: body.author.name, email: body.author.email }
      : me;
    const c = makeCommit(r.code, {
      message: body.message,
      author,
      committer: me,
      date: body.author?.date ?? now(),
      parents,
      files: new Map(tree.files),
    });
    return response(201).json(gitCommit(r.repo, c));
  }),

  http.get("/repos/{owner}/{repo}/git/ref/{ref}", async ({ params, response }) => {
    const fault = await gate("code-refs");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const [kind, ...rest] = params.ref.split("/");
    const name = rest.join("/");
    const map = kind === "heads" ? r.code.branches : kind === "tags" ? r.code.tags : undefined;
    const target = map?.get(name);
    if (!target) return response.untyped(notFound());
    const type = r.code.tagObjects.has(target) ? "tag" : "commit";
    return response(200).json(gitRef(r.repo, `refs/${params.ref}`, target, type));
  }),

  http.get("/repos/{owner}/{repo}/git/matching-refs/{ref}", async ({ params, response }) => {
    const fault = await gate("code-refs");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const all = [
      ...[...r.code.branches].map(([n, s]) => gitRef(r.repo, `refs/heads/${n}`, s)),
      ...[...r.code.tags].map(([n, s]) =>
        gitRef(r.repo, `refs/tags/${n}`, s, r.code.tagObjects.has(s) ? "tag" : "commit"),
      ),
    ];
    return response(200).json(all.filter((g) => g.ref.startsWith(`refs/${params.ref}`)));
  }),

  http.post("/repos/{owner}/{repo}/git/refs", async ({ params, request, response }) => {
    const fault = await gate("code-create-branch");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const body = await request.json();
    const m = /^refs\/(heads|tags)\/(.+)$/.exec(body.ref ?? "");
    if (!m || !VALID_REF.test(m[2]))
      return response.untyped(unprocessable("Reference", "ref", "invalid"));
    const map = m[1] === "heads" ? r.code.branches : r.code.tags;
    if (map.has(m[2])) return response.untyped(unprocessable("Reference", "ref", "already_exists"));
    if (!r.code.commits.has(body.sha) && !r.code.tagObjects.has(body.sha)) {
      return response.untyped(unprocessable("Reference", "sha", "invalid"));
    }
    map.set(m[2], body.sha);
    const type = r.code.tagObjects.has(body.sha) ? "tag" : "commit";
    return response(201).json(gitRef(r.repo, body.ref, body.sha, type));
  }),

  http.patch("/repos/{owner}/{repo}/git/refs/{ref}", async ({ params, request, response }) => {
    const fault = await gate("code-update-ref");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const name = params.ref.replace(/^heads\//, "");
    const current = r.code.branches.get(name);
    if (!current || !params.ref.startsWith("heads/"))
      return response.untyped(unprocessable("Reference", "ref", "invalid"));
    const body = await request.json();
    if (!r.code.commits.has(body.sha))
      return response.untyped(unprocessable("Reference", "sha", "invalid"));
    const fastForward = history(r.code, body.sha).some((c) => c.sha === current);
    if (!fastForward && !body.force)
      return response.untyped(unprocessable("Reference", "sha", "not_fast_forward"));
    r.code.branches.set(name, body.sha);
    return response(200).json(gitRef(r.repo, `refs/heads/${name}`, body.sha));
  }),

  http.delete("/repos/{owner}/{repo}/git/refs/{ref}", async ({ params, response }) => {
    const fault = await gate("code-delete-branch");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const [kind, ...rest] = params.ref.split("/");
    const name = rest.join("/");
    if (kind === "heads") {
      if (!r.code.branches.has(name)) return response(422).empty();
      if (name === r.repo.default_branch || r.code.protectedBranches.has(name)) {
        return response.untyped(conflict(`Cannot delete the protected branch ${name}.`));
      }
      r.code.branches.delete(name);
      return response(204).empty();
    }
    if (kind === "tags" && r.code.tags.delete(name)) return response(204).empty();
    return response(422).empty();
  }),

  http.get("/repos/{owner}/{repo}/git/tags/{tag_sha}", async ({ params, response }) => {
    const fault = await gate("code-tags");
    if (fault) return response.untyped(fault);
    const t = codeRepo(params.owner, params.repo)?.code.tagObjects.get(params.tag_sha);
    return t ? response(200).json(t) : response.untyped(notFound());
  }),

  http.post("/repos/{owner}/{repo}/git/tags", async ({ params, request, response }) => {
    const fault = await gate("code-create-tag");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const body = await request.json();
    if (!VALID_REF.test(body.tag)) return response.untyped(unprocessable("Tag", "tag", "invalid"));
    if (!r.code.commits.has(body.object))
      return response.untyped(unprocessable("Tag", "object", "invalid"));
    const key = sha(`tagobj:${r.repo.full_name}:${body.tag}:${body.object}:${body.message}`);
    const tagger = body.tagger ?? { name: "Mira Okafor", email: "mira@graftyard.test" };
    const t = {
      node_id: `TAG_${key.slice(0, 12)}`,
      tag: body.tag,
      sha: key,
      url: `${r.repo.url}/git/tags/${key}`,
      message: body.message,
      tagger: { date: tagger.date ?? now(), name: tagger.name, email: tagger.email },
      object: {
        sha: body.object,
        type: body.type,
        url: `${r.repo.url}/git/commits/${body.object}`,
      },
      verification: {
        verified: false,
        reason: "unsigned",
        signature: null,
        payload: null,
        verified_at: null,
      },
    };
    r.code.tagObjects.set(key, t);
    return response(201).json(t);
  }),

  // ---- Commits and statuses ----

  http.get("/repos/{owner}/{repo}/commits", async ({ params, query, response }) => {
    const fault = await gate("code-commits");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const tip = at(r.code, query.get("sha"), r.repo.default_branch ?? "main");
    if (!tip) return response.untyped(notFound());
    const path = query.get("path")?.replace(/^\/+|\/+$/g, "");
    const author = query.get("author")?.toLowerCase();
    const since = query.get("since");
    const until = query.get("until");
    const list = history(r.code, tip.sha).filter(
      (c) =>
        (!path || touches(r.code, c, path)) &&
        (!author ||
          c.author.user?.login.toLowerCase() === author ||
          c.author.email.toLowerCase() === author) &&
        (!since || c.date >= since) &&
        (!until || c.date <= until),
    );
    const items = page(list, query).map((c) => restCommit(r.code, r.repo, c));
    const perPage = Math.min(100, Number(query.get("per_page")) || 30);
    const pageNo = Math.max(1, Number(query.get("page")) || 1);
    const last = Math.max(1, Math.ceil(list.length / perPage));
    const link =
      last > 1
        ? [
            pageNo < last ? `<${r.repo.url}/commits?page=${pageNo + 1}>; rel="next"` : "",
            pageNo < last ? `<${r.repo.url}/commits?page=${last}>; rel="last"` : "",
            pageNo > 1 ? `<${r.repo.url}/commits?page=1>; rel="first"` : "",
            pageNo > 1 ? `<${r.repo.url}/commits?page=${pageNo - 1}>; rel="prev"` : "",
          ]
            .filter(Boolean)
            .join(", ")
        : "";
    return response(200).json(items, { headers: link ? { Link: link } : {} });
  }),

  http.get("/repos/{owner}/{repo}/commits/{ref}", async ({ params, response }) => {
    const fault = await gate("code-commit");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    const c = r && at(r.code, params.ref, params.ref);
    if (!r || !c) {
      return r
        ? response.untyped(unprocessable("Commit", "sha", "invalid"))
        : response.untyped(notFound());
    }
    return response(200).json(restCommit(r.code, r.repo, c, true));
  }),

  http.get("/repos/{owner}/{repo}/commits/{ref}/status", async ({ params, response }) => {
    const fault = await gate("code-statuses");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    const c = r && at(r.code, params.ref, params.ref);
    return r && c
      ? response(200).json(combinedStatus(r.code, r.repo, c.sha))
      : response.untyped(notFound());
  }),

  http.get("/repos/{owner}/{repo}/commits/{ref}/statuses", async ({ params, query, response }) => {
    const fault = await gate("code-statuses");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    const c = r && at(r.code, params.ref, params.ref);
    return r && c
      ? response(200).json(page(r.code.statuses.get(c.sha) ?? [], query))
      : response.untyped(notFound());
  }),

  http.post("/repos/{owner}/{repo}/statuses/{sha}", async ({ params, request, response }) => {
    const fault = await gate("code-create-status");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    if (!r.code.commits.has(params.sha))
      return response.untyped(unprocessable("Status", "sha", "invalid"));
    const body = await request.json();
    if (!["error", "failure", "pending", "success"].includes(body.state)) {
      return response.untyped(unprocessable("Status", "state", "invalid"));
    }
    if (body.target_url && !/^https?:\/\/[^\s/]+/.test(body.target_url)) {
      return response.untyped(unprocessable("Status", "target_url", "invalid"));
    }
    const status = addStatus(
      r.code,
      r.repo,
      params.sha,
      {
        state: body.state,
        context: body.context || "default",
        description: body.description ?? null,
        target_url: body.target_url ?? null,
      },
      users.mira,
      now(),
    );
    return response(201).json(status);
  }),

  // ---- Branches and tags ----

  http.get("/repos/{owner}/{repo}/branches", async ({ params, query, response }) => {
    const fault = await gate("code-branches");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const only = query.get("protected");
    const names = [...r.code.branches.keys()]
      .filter((n) => only === null || (only === "true") === r.code.protectedBranches.has(n))
      .sort();
    return response(200).json(page(names, query).map((n) => shortBranch(r.code, r.repo, n)));
  }),

  http.get("/repos/{owner}/{repo}/branches/{branch}", async ({ params, response }) => {
    const fault = await gate("code-branch");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r || !r.code.branches.has(params.branch)) return response.untyped(notFound());
    return response(200).json(branchWithProtection(r.code, r.repo, params.branch));
  }),

  http.post(
    "/repos/{owner}/{repo}/branches/{branch}/rename",
    async ({ params, request, response }) => {
      const fault = await gate("code-rename-branch");
      if (fault) return response.untyped(fault);
      const r = codeRepo(params.owner, params.repo);
      if (!r || !r.code.branches.has(params.branch)) return response.untyped(notFound());
      const { new_name } = await request.json();
      const name = new_name.trim().replace(/^refs\/heads\//, "");
      if (!name) return response.untyped(unprocessable("Branch", "new_name", "missing_field"));
      if (!VALID_REF.test(name))
        return response.untyped(unprocessable("Branch", "new_name", "invalid"));
      if (r.code.branches.has(name))
        return response.untyped(unprocessable("Branch", "new_name", "already_exists"));
      const target = r.code.branches.get(params.branch)!;
      r.code.branches.delete(params.branch);
      r.code.branches.set(name, target);
      if (r.code.protectedBranches.delete(params.branch)) r.code.protectedBranches.add(name);
      if (r.repo.default_branch === params.branch) r.repo.default_branch = name;
      // Open pull requests follow a renamed base or head, as GHES retargets them.
      for (const pr of r.state.pulls) {
        if (pr.base.ref === params.branch) pr.base.ref = name;
        if (pr.head.ref === params.branch) {
          pr.head.ref = name;
          pr.head.label = `${r.repo.owner.login}:${name}`;
        }
      }
      return response(201).json(branchWithProtection(r.code, r.repo, name));
    },
  ),

  http.get("/repos/{owner}/{repo}/tags", async ({ params, query, response }) => {
    const fault = await gate("code-tags");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const list = [...r.code.tags]
      .map(([name, target]) => ({ name, commit: r.code.commits.get(peel(r.code, target))! }))
      .sort((a, b) => b.commit.date.localeCompare(a.commit.date))
      .map(({ name, commit }) => ({
        name,
        commit: { sha: commit.sha, url: `${r.repo.url}/commits/${commit.sha}` },
        zipball_url: `${r.repo.url}/zipball/refs/tags/${name}`,
        tarball_url: `${r.repo.url}/tarball/refs/tags/${name}`,
        node_id: `REF_${commit.sha.slice(0, 12)}`,
      }));
    return response(200).json(page(list, query));
  }),

  // ---- Languages, contributors, and stats ----

  http.get("/repos/{owner}/{repo}/languages", async ({ params, response }) => {
    const fault = await gate("code-languages");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    const c = r && at(r.code, null, r.repo.default_branch ?? "main");
    return r ? response(200).json(c ? languages(c) : {}) : response.untyped(notFound());
  }),

  http.get("/repos/{owner}/{repo}/contributors", async ({ params, query, response }) => {
    const fault = await gate("code-contributors");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const c = at(r.code, null, r.repo.default_branch ?? "main");
    if (!c) return response(204).empty();
    return response(200).json(page(contributors(r.code, c), query));
  }),

  http.get("/repos/{owner}/{repo}/stats/contributors", async ({ params, response }) => {
    const fault = await gate("code-insights");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    const c = r && at(r.code, null, r.repo.default_branch ?? "main");
    if (!r) return response.untyped(notFound());
    return c ? response(200).json(contributorStats(r.code, c)) : response(204).empty();
  }),

  http.get("/repos/{owner}/{repo}/stats/commit_activity", async ({ params, response }) => {
    const fault = await gate("code-insights");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    const c = r && at(r.code, null, r.repo.default_branch ?? "main");
    if (!r) return response.untyped(notFound());
    return c ? response(200).json(commitActivity(r.code, c)) : response(204).empty();
  }),

  http.get("/repos/{owner}/{repo}/stats/code_frequency", async ({ params, response }) => {
    const fault = await gate("code-insights");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    const c = r && at(r.code, null, r.repo.default_branch ?? "main");
    if (!r) return response.untyped(notFound());
    return c ? response(200).json(codeFrequency(r.code, r.repo, c)) : response(204).empty();
  }),

  http.get("/repos/{owner}/{repo}/stats/participation", async ({ params, response }) => {
    const fault = await gate("code-insights");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    const c = r && at(r.code, null, r.repo.default_branch ?? "main");
    if (!r) return response.untyped(notFound());
    return response(200).json(c ? participation(r.code, r.repo, c) : { all: [], owner: [] });
  }),

  http.get("/repos/{owner}/{repo}/stats/punch_card", async ({ params, response }) => {
    const fault = await gate("code-punch-card");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    const c = r && at(r.code, null, r.repo.default_branch ?? "main");
    if (!r) return response.untyped(notFound());
    return c ? response(200).json(punchCard(r.code, c)) : response(204).empty();
  }),

  // ---- The private /_t/ui API ----

  ui.get("/repos/{owner}/{repo}/resolve/{splat}", async ({ params, response }) => {
    const fault = await gate("code-resolve");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    const hit = r && splitSplat(r.code, params.splat);
    if (!r || !hit) return response.untyped(notFound());
    const c = r.code.commits.get(hit.sha)!;
    const type =
      hit.path === "" || isDir(c.files, hit.path) ? "dir" : c.files.has(hit.path) ? "file" : null;
    return response(200).json({ ...hit, type, default_branch: r.repo.default_branch ?? "main" });
  }),

  ...(
    [
      "/repos/{owner}/{repo}/tree-commits/{ref}",
      "/repos/{owner}/{repo}/tree-commits/{ref}/{path}",
    ] as const
  ).map((route) =>
    ui.get(route, async ({ params, response }) => {
      const fault = await gate("code-tree-commits");
      if (fault) return response.untyped(fault);
      const r = codeRepo(params.owner, params.repo);
      const c = r && at(r.code, params.ref, params.ref);
      if (!r || !c) return response.untyped(notFound());
      const dir = ("path" in params ? params.path : "").replace(/^\/+|\/+$/g, "");
      if (dir && !isDir(c.files, dir)) return response.untyped(notFound());
      const all = history(r.code, c.sha);
      const touching = all.filter((x) => touches(r.code, x, dir));
      const entries = listDir(c.files, dir).map((e) => ({
        name: e.name,
        path: e.path,
        type: e.type === "tree" ? ("dir" as const) : ("file" as const),
        commit: summary(lastTouching(r.code, c.sha, e.path) ?? c),
      }));
      return response(200).json({
        latest: summary(touching[0] ?? c),
        count: touching.length,
        entries,
      });
    }),
  ),

  ui.get("/repos/{owner}/{repo}/blame/{ref}/{path}", async ({ params, response }) => {
    const fault = await gate("code-blame");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    const c = r && at(r.code, params.ref, params.ref);
    const owners = r && c && blame(r.code, c, params.path);
    if (!c || !owners) return response.untyped(notFound());
    const lines = (decodeText(blob(c.files.get(params.path)!)!.b64) ?? "").split("\n");
    if (lines.at(-1) === "") lines.pop();
    const ranges: { start: number; end: number; commit: CommitSummary }[] = [];
    owners.forEach((commit, i) => {
      const last = ranges.at(-1);
      if (last && last.commit.sha === commit.sha) last.end = i + 1;
      else ranges.push({ start: i + 1, end: i + 1, commit: summary(commit) });
    });
    const dates = owners.map((o) => o.date).sort();
    return response(200).json({
      path: params.path,
      lines,
      ranges,
      oldest: dates[0] ?? c.date,
      newest: dates.at(-1) ?? c.date,
    });
  }),

  // The README decoded, so the overview needs no base64 (the edge serves this, not REST's).
  ui.get("/repos/{owner}/{repo}/readme", async ({ params, query, response }) => {
    const fault = await gate("code-readme");
    if (fault) return response.untyped(fault);
    const hit = findReadme(params, query.get("ref"));
    if (!hit) return response.untyped(notFound());
    const b = blob(hit.key)!;
    return response(200).json({
      name: hit.path.split("/").at(-1)!,
      path: hit.path,
      sha: hit.key,
      size: b.size,
      text: b.text ?? "",
    });
  }),

  ui.get("/repos/{owner}/{repo}/branch-summaries", async ({ params, response }) => {
    const fault = await gate("code-branches");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const base = r.repo.default_branch ?? "main";
    const baseTip = r.code.branches.get(base);
    const baseSet = new Set(baseTip ? history(r.code, baseTip).map((c) => c.sha) : []);
    const list = [...r.code.branches].map(([name, target]) => {
      const tip = r.code.commits.get(target)!;
      const mine = history(r.code, target);
      const mineSet = new Set(mine.map((c) => c.sha));
      const pr = r.state.pulls.find((p) => p.head.ref === name);
      const status = combinedStatus(r.code, r.repo, target);
      return {
        name,
        sha: target,
        ahead: mine.filter((c) => !baseSet.has(c.sha)).length,
        behind: [...baseSet].filter((s) => !mineSet.has(s)).length,
        updated_at: tip.date,
        author: summary(tip).author,
        protected: r.code.protectedBranches.has(name),
        default: name === base,
        status: status.total_count ? (status.state as "success" | "failure" | "pending") : null,
        pull: pr
          ? {
              number: pr.number,
              state: pr.merged_at
                ? ("merged" as const)
                : pr.state === "closed"
                  ? ("closed" as const)
                  : pr.draft
                    ? ("draft" as const)
                    : ("open" as const),
            }
          : null,
      };
    });
    return response(200).json(list.sort((a, b) => b.updated_at.localeCompare(a.updated_at)));
  }),

  ui.get("/repos/{owner}/{repo}/traffic/views", async ({ params, query, response }) => {
    const fault = await gate("code-traffic");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const points = traffic(r.repo, "views", query.get("per") === "week" ? "week" : "day");
    return response(200).json({ ...points.totals, views: points.points });
  }),

  ui.get("/repos/{owner}/{repo}/traffic/clones", async ({ params, query, response }) => {
    const fault = await gate("code-traffic");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const points = traffic(r.repo, "clones", query.get("per") === "week" ? "week" : "day");
    return response(200).json({ ...points.totals, clones: points.points });
  }),

  ui.get("/repos/{owner}/{repo}/traffic/popular/referrers", async ({ params, response }) => {
    const fault = await gate("code-traffic");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    // Shares of the same 14 days the views chart counts, so no table outnumbers it.
    const { totals } = traffic(r.repo, "views", "day");
    return response(200).json(
      [
        ["ghes.graftyard.test", 0.34],
        ["graftyard.dev", 0.19],
        ["chat.graftyard.test", 0.08],
        ["search.graftyard.test", 0.05],
        ["Bing", 0.01],
      ].map(([referrer, share]) => ({
        referrer: String(referrer),
        count: Math.round(totals.count * Number(share)),
        uniques: Math.max(1, Math.round(totals.uniques * Number(share))),
      })),
    );
  }),

  ui.get("/repos/{owner}/{repo}/traffic/popular/paths", async ({ params, response }) => {
    const fault = await gate("code-traffic");
    if (fault) return response.untyped(fault);
    const r = codeRepo(params.owner, params.repo);
    const c = r && at(r.code, null, r.repo.default_branch ?? "main");
    if (!r) return response.untyped(notFound());
    const files = c
      ? [...c.files.keys()].filter((p) => /\.(rs|md|ts|tsx|toml)$/.test(p)).slice(0, 6)
      : [];
    const { totals } = traffic(r.repo, "views", "day");
    const share = (f: number) => ({
      count: Math.round(totals.count * f),
      uniques: Math.max(1, Math.round(totals.uniques * f * 1.2)),
    });
    return response(200).json([
      { path: `/${r.repo.full_name}`, title: r.repo.full_name, ...share(0.42) },
      ...files.map((p, i) => ({
        path: `/${r.repo.full_name}/blob/main/${p}`,
        title: `${p} at main`,
        ...share(0.12 - i * 0.017),
      })),
    ]);
  }),
];
