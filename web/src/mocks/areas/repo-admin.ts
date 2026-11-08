import { HttpResponse } from "msw";
import { createOpenApiHttp } from "openapi-msw";
import type { UiPaths } from "@/areas/repo-admin/private-api";
import { normalizeRepoName, TOPIC_RE } from "@/areas/repo-admin/domain";
import { gate, http, notFound, page, unprocessable } from "../http";
import { users, type Repository } from "../world";
import { db } from "./core-db";
import { admin, minimal } from "./repo-admin-db";
import { gitignores, licenses } from "./repo-admin-catalog";
import {
  create,
  fork,
  freeName,
  full,
  generate,
  ownerOf,
  remove,
  rename,
  taken,
  transfer,
} from "./repo-admin-repos";

// The repository itself: settings, topics, LFS, transfer, delete, create, generate, and
// fork, plus the gitignore and license catalogs and the private /_t/ui settings paths.

const ui = createOpenApiHttp<UiPaths>({ baseUrl: "/_t/ui" });
const DOCS = "https://docs.github.com/enterprise-server@3.22/rest";

// GHES's validation error with a message, for the 422s whose text the UI shows.
export function invalid(resource: string, field: string, code: string, message: string) {
  return HttpResponse.json(
    {
      message: "Validation Failed",
      documentation_url: DOCS,
      status: "422",
      errors: [{ resource, field, code, message }],
    },
    { status: 422 },
  );
}

export const forbidden = (message: string) =>
  HttpResponse.json({ message, documentation_url: DOCS, status: "403" }, { status: 403 });

// Which fault key a PATCH /repos/{owner}/{repo} answers to, by what it changes.
function patchKey(body: Record<string, unknown>) {
  if ("name" in body) return "admin-repo-rename";
  if ("archived" in body) return "admin-repo-archive";
  if ("visibility" in body || "private" in body) return "admin-repo-visibility";
  if ("default_branch" in body) return "admin-repo-default-branch";
  if (Object.keys(body).some((k) => k.startsWith("allow_") || k.includes("merge")))
    return "admin-repo-merge";
  if (Object.keys(body).some((k) => k.startsWith("has_") || k === "is_template"))
    return "admin-repo-features";
  return "admin-repo-update";
}

export const handlers = [
  http.patch("/repos/{owner}/{repo}", async ({ params, request, response }) => {
    const body = (await request.json()) ?? {};
    const fault = await gate(patchKey(body));
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    if (!found) return response.untyped(notFound());
    const { repo, admin: rec } = found;
    const onlyUnarchive = Object.keys(body).every((k) => k === "archived");
    if (repo.archived && !onlyUnarchive) {
      return response.untyped(forbidden("Repository was archived so is read-only."));
    }
    if (body.name !== undefined) {
      const name = normalizeRepoName(body.name);
      if (!name) return response.untyped(unprocessable("Repository", "name", "missing_field"));
      if (name.toLowerCase() !== repo.name.toLowerCase() && taken(repo.owner.login, name)) {
        return response.untyped(
          invalid("Repository", "name", "already_exists", "name already exists on this account"),
        );
      }
    }
    if (body.visibility === "internal" && repo.owner.type !== "Organization") {
      return response.untyped(
        invalid(
          "Repository",
          "visibility",
          "invalid",
          "Only organization-owned repositories can be internal",
        ),
      );
    }
    if (body.default_branch !== undefined && !rec.branches.includes(body.default_branch)) {
      return response.untyped(
        invalid(
          "Repository",
          "default_branch",
          "invalid",
          `Branch ${body.default_branch} does not exist`,
        ),
      );
    }
    const methods = {
      merge: body.allow_merge_commit ?? repo.allow_merge_commit,
      squash: body.allow_squash_merge ?? repo.allow_squash_merge,
      rebase: body.allow_rebase_merge ?? repo.allow_rebase_merge,
    };
    if (!methods.merge && !methods.squash && !methods.rebase) {
      return response.untyped(
        invalid(
          "Repository",
          "merge_methods",
          "invalid",
          "You must allow at least one merge method",
        ),
      );
    }
    if (body.homepage && !/^https?:\/\/\S+$/.test(body.homepage)) {
      return response.untyped(
        invalid("Repository", "homepage", "invalid", "homepage is not a valid URL"),
      );
    }

    const { name, visibility, private: priv, security_and_analysis, ...rest } = body;
    void security_and_analysis;
    Object.assign(repo, rest);
    if (visibility !== undefined) {
      repo.visibility = visibility;
      repo.private = visibility !== "public";
    } else if (priv !== undefined) {
      repo.visibility = priv ? "private" : "public";
      repo.private = priv;
    }
    repo.updated_at = new Date().toISOString();
    const result = name !== undefined ? rename(repo, normalizeRepoName(name)) : repo;
    return response(200).json(full(result));
  }),

  http.delete("/repos/{owner}/{repo}", async ({ params, response }) => {
    const fault = await gate("admin-repo-delete");
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    if (!found) return response.untyped(notFound());
    remove(found.repo);
    return response(204).empty();
  }),

  http.post("/repos/{owner}/{repo}/transfer", async ({ params, request, response }) => {
    const fault = await gate("admin-repo-transfer");
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    if (!found) return response.untyped(notFound());
    const body = await request.json();
    const owner =
      ownerOf(body.new_owner) ?? Object.values(users).find((u) => u.login === body.new_owner);
    if (!owner) {
      return response.untyped(
        invalid(
          "Repository",
          "new_owner",
          "invalid",
          `${body.new_owner} is not a user or organization`,
        ),
      );
    }
    if (owner.login === found.repo.owner.login) {
      return response.untyped(
        invalid(
          "Repository",
          "new_owner",
          "invalid",
          "The repository already belongs to this owner",
        ),
      );
    }
    const name = body.new_name ? normalizeRepoName(body.new_name) : found.repo.name;
    if (taken(owner.login, name)) {
      return response.untyped(
        invalid(
          "Repository",
          "new_name",
          "already_exists",
          `${owner.login} already has a repository named ${name}`,
        ),
      );
    }
    return response(202).json(minimal(transfer(found.repo, owner, name)));
  }),

  http.get("/repos/{owner}/{repo}/topics", async ({ params, response }) => {
    const fault = await gate("admin-repo-topics");
    if (fault) return response.untyped(fault);
    const repo = db.state(params.owner, params.repo)?.repo;
    return repo ? response(200).json({ names: repo.topics ?? [] }) : response.untyped(notFound());
  }),

  http.put("/repos/{owner}/{repo}/topics", async ({ params, request, response }) => {
    const fault = await gate("admin-repo-topics-save");
    if (fault) return response.untyped(fault);
    const repo = db.state(params.owner, params.repo)?.repo;
    if (!repo) return response.untyped(notFound());
    if (repo.archived)
      return response.untyped(forbidden("Repository was archived so is read-only."));
    const { names } = await request.json();
    const bad = names.filter((n) => !TOPIC_RE.test(n));
    if (bad.length || names.length > 20) {
      return response(422).json({
        message: "Validation Failed",
        documentation_url: `${DOCS}/repos/repos#replace-all-repository-topics`,
        errors: bad.length
          ? bad.map(
              (n) =>
                `${n} is not a valid topic. Topics must start with a lowercase letter or number, consist of 50 characters or less, and can include hyphens.`,
            )
          : ["A repository cannot have more than 20 topics."],
      });
    }
    repo.topics = [...new Set(names)];
    return response(200).json({ names: repo.topics });
  }),

  http.put("/repos/{owner}/{repo}/lfs", async ({ params, response }) => {
    const fault = await gate("admin-repo-lfs");
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    if (!found) return response.untyped(notFound());
    const rec = found.admin;
    if (rec.lfs === "disabled") {
      // Enabling migrates the repository's storage in the background.
      rec.lfs = "enabling";
      setTimeout(() => {
        if (rec.lfs === "enabling") rec.lfs = "enabled";
      }, 3500);
    }
    return response(202).json({});
  }),

  http.delete("/repos/{owner}/{repo}/lfs", async ({ params, response }) => {
    const fault = await gate("admin-repo-lfs");
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    if (!found) return response.untyped(notFound());
    found.admin.lfs = "disabled";
    return response(204).empty();
  }),

  http.get("/repos/{owner}/{repo}/forks", async ({ params, query, response }) => {
    const fault = await gate("admin-repo-forks");
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    if (!found) return response.untyped(notFound());
    const live = db
      .repos()
      .filter((r) => admin.state(r.owner.login, r.name)?.admin.parent?.id === found.repo.id);
    const sort = query.get("sort") ?? "newest";
    const at = (r: Repository) => Date.parse(r.created_at ?? "");
    const forks = [...live, ...found.admin.seededForks].sort((a, b) =>
      sort === "oldest"
        ? at(a) - at(b)
        : sort === "stargazers"
          ? b.stargazers_count - a.stargazers_count
          : at(b) - at(a),
    );
    return response(200).json(page(forks.map(minimal), query));
  }),

  http.post("/repos/{owner}/{repo}/forks", async ({ params, request, response }) => {
    const fault = await gate("admin-repo-fork");
    if (fault) return response.untyped(fault);
    const parent = db.state(params.owner, params.repo)?.repo;
    if (!parent) return response.untyped(notFound());
    const body = (await request.json()) ?? {};
    const owner = body.organization ? ownerOf(body.organization) : users.mira;
    if (!owner || (body.organization && owner.type !== "Organization")) {
      return response.untyped(
        invalid(
          "Repository",
          "organization",
          "invalid",
          `${body.organization} is not an organization you belong to`,
        ),
      );
    }
    const wanted = body.name ? normalizeRepoName(body.name) : null;
    if (wanted && taken(owner.login, wanted)) {
      return response.untyped(
        invalid("Repository", "name", "already_exists", "name already exists on this account"),
      );
    }
    const name = wanted ?? freeName(owner.login, parent.name);
    return response(202).json(fork(parent, owner, name, body.default_branch_only ?? false));
  }),

  http.post("/user/repos", async ({ request, response }) => {
    const fault = await gate("admin-repo-create");
    if (fault) return response.untyped(fault);
    const body = await request.json();
    const name = normalizeRepoName(body.name ?? "");
    if (!name) return response.untyped(unprocessable("Repository", "name", "missing_field"));
    if (taken(users.mira.login, name)) {
      return response.untyped(
        invalid("Repository", "name", "already_exists", "name already exists on this account"),
      );
    }
    return response(201).json(
      create({
        owner: users.mira,
        name,
        description: body.description,
        homepage: body.homepage,
        visibility: body.private ? "private" : "public",
        autoInit: body.auto_init ?? false,
        gitignore: body.gitignore_template,
        license: licenses.find((l) => l.key === body.license_template),
        isTemplate: body.is_template,
        hasIssues: body.has_issues,
        hasWiki: body.has_wiki,
        hasProjects: body.has_projects,
      }),
    );
  }),

  http.post("/orgs/{org}/repos", async ({ params, request, response }) => {
    const fault = await gate("admin-repo-create");
    if (fault) return response.untyped(fault);
    const owner = ownerOf(params.org);
    if (!owner || owner.type !== "Organization") return response.untyped(notFound());
    const body = await request.json();
    const name = normalizeRepoName(body.name ?? "");
    if (!name) return response.untyped(unprocessable("Repository", "name", "missing_field"));
    if (taken(owner.login, name)) {
      return response.untyped(
        invalid("Repository", "name", "already_exists", "name already exists on this account"),
      );
    }
    const visibility = body.visibility ?? (body.private ? "private" : "public");
    if (visibility === "public") {
      // world.ts's org forbids members from creating public repositories.
      return response.untyped(
        invalid(
          "Repository",
          "visibility",
          "invalid",
          "Members of graftyard cannot create public repositories",
        ),
      );
    }
    return response(201).json(
      create({
        owner,
        name,
        description: body.description,
        homepage: body.homepage,
        visibility,
        autoInit: body.auto_init ?? false,
        gitignore: body.gitignore_template,
        license: licenses.find((l) => l.key === body.license_template),
        isTemplate: body.is_template,
        hasIssues: body.has_issues,
        hasWiki: body.has_wiki,
        hasProjects: body.has_projects,
      }),
    );
  }),

  http.post(
    "/repos/{template_owner}/{template_repo}/generate",
    async ({ params, request, response }) => {
      const fault = await gate("admin-repo-generate");
      if (fault) return response.untyped(fault);
      const template = db.state(params.template_owner, params.template_repo)?.repo;
      if (!template) return response.untyped(notFound());
      if (!template.is_template) {
        return response.untyped(
          invalid(
            "Repository",
            "template",
            "invalid",
            `${template.full_name} is not a template repository`,
          ),
        );
      }
      const body = await request.json();
      const owner = ownerOf(body.owner ?? users.mira.login);
      const name = normalizeRepoName(body.name);
      if (!owner) return response.untyped(notFound());
      if (!name) return response.untyped(unprocessable("Repository", "name", "missing_field"));
      if (taken(owner.login, name)) {
        return response.untyped(
          invalid("Repository", "name", "already_exists", "name already exists on this account"),
        );
      }
      return response(201).json(
        generate(
          template,
          owner,
          name,
          body.description,
          body.private ?? false,
          body.include_all_branches ?? false,
        ),
      );
    },
  ),

  http.get("/gitignore/templates", async ({ response }) => {
    const fault = await gate("admin-repo-gitignore");
    if (fault) return response.untyped(fault);
    return response(200).json(gitignores.map((g) => g.name));
  }),

  http.get("/gitignore/templates/{name}", async ({ params, response }) => {
    const fault = await gate("admin-repo-gitignore");
    if (fault) return response.untyped(fault);
    const t = gitignores.find((g) => g.name === params.name);
    return t ? response(200).json(t) : response.untyped(notFound());
  }),

  http.get("/licenses", async ({ query, response }) => {
    const fault = await gate("admin-repo-licenses");
    if (fault) return response.untyped(fault);
    const featured = query.get("featured");
    const list = licenses
      .filter((l) => featured !== "true" || l.featured)
      .map(({ featured, ...l }) => (void featured, l));
    return response(200).json(page(list, query));
  }),

  http.get("/licenses/{license}", async ({ params, response }) => {
    const fault = await gate("admin-repo-licenses");
    if (fault) return response.untyped(fault);
    const l = licenses.find((x) => x.key === params.license);
    if (!l) return response.untyped(notFound());
    return response(200).json({
      key: l.key,
      name: l.name,
      spdx_id: l.spdx_id,
      url: l.url,
      node_id: l.node_id,
      html_url: l.html_url ?? "",
      featured: l.featured,
      description: `${l.name}, as GitHub's license picker describes it.`,
      implementation:
        "Create a text file (typically named LICENSE or LICENSE.md) in the root of your source code and copy the text of the license into the file.",
      permissions: ["commercial-use", "modifications", "distribution", "private-use"],
      conditions: ["include-copyright"],
      limitations: ["liability", "warranty"],
      body: `${l.name}\n\nCopyright (c) [year] [fullname]\n`,
    });
  }),

  http.get("/repos/{owner}/{repo}/installation", async ({ params, response }) => {
    const fault = await gate("admin-repo-installations");
    if (fault) return response.untyped(fault);
    const repo = db.state(params.owner, params.repo)?.repo;
    // The authenticated app is Graftyard's agent app; the answer is its installation.
    const install =
      repo && admin.installationsFor(repo).find((i) => i.app_slug === "graftyard-agents");
    return install ? response(200).json(install) : response.untyped(notFound());
  }),

  // ---- Private /_t/ui paths ----

  ui.get("/repos/{owner}/{repo}/admin-settings", async ({ params, response }) => {
    const fault = await gate("admin-repo-general");
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    if (!found) return response.untyped(notFound());
    return response(200).json({ lfs: found.admin.lfs, lfs_bytes: found.admin.lfsBytes });
  }),

  ui.patch("/repos/{owner}/{repo}/admin-settings", async ({ params, request, response }) => {
    const fault = await gate("admin-repo-features");
    if (fault) return response.untyped(fault);
    const repo = db.state(params.owner, params.repo)?.repo;
    if (!repo) return response.untyped(notFound());
    if (repo.archived)
      return response.untyped(forbidden("Repository was archived so is read-only."));
    const body = await request.json();
    if (body.has_discussions !== undefined) repo.has_discussions = body.has_discussions;
    if (body.allow_auto_merge !== undefined) repo.allow_auto_merge = body.allow_auto_merge;
    return response(200).json(full(repo));
  }),

  ui.get("/repos/{owner}/{repo}/installations", async ({ params, response }) => {
    const fault = await gate("admin-repo-installations");
    if (fault) return response.untyped(fault);
    const repo = db.state(params.owner, params.repo)?.repo;
    return repo ? response(200).json(admin.installationsFor(repo)) : response.untyped(notFound());
  }),
];
