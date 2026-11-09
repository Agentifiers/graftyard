import type { Schemas } from "@/api/client";
import { normalizeRepoName } from "@/areas/repo-admin/domain";
import { fullRepository, org, repository, users, type Repository, type SimpleUser } from "../world";
import { db } from "./core-db";
import { admin } from "./repo-admin-db";

// The repository lifecycle: create, generate from a template, fork, rename, transfer, and
// delete. Each goes through core-db's addRepo, moveRepo, and deleteRepo, so every area's
// reads follow it.

export const owners: SimpleUser[] = [users.mira, users.org];
export const ownerOf = (login: string) =>
  owners.find((o) => o.login.toLowerCase() === login.toLowerCase());

export const taken = (owner: string, name: string) => Boolean(db.state(owner, name));

// Every URL a repository carries names it, so a rename or transfer rebuilds them from
// world.ts's repository() and keeps the id and every setting.
export function relocate(repo: Repository, owner: SimpleUser, name: string): Repository {
  const fresh = repository(name, repo.description ?? "", visibilityOf(repo), {
    language: repo.language,
    stars: repo.stargazers_count,
    topics: repo.topics ?? [],
    pushed: repo.pushed_at ?? new Date().toISOString(),
    owner,
  });
  const urls = Object.fromEntries(
    Object.entries(fresh).filter(([k]) => k === "url" || k.endsWith("_url")),
  );
  return {
    ...repo,
    ...urls,
    name,
    full_name: fresh.full_name,
    owner,
    node_id: repo.node_id,
    updated_at: new Date().toISOString(),
  };
}

export function visibilityOf(repo: Pick<Repository, "visibility" | "private">) {
  const v = repo.visibility;
  return v === "public" || v === "internal" || v === "private"
    ? v
    : repo.private
      ? "private"
      : "public";
}

export type CreateInput = {
  owner: SimpleUser;
  name: string;
  description?: string;
  homepage?: string;
  visibility: "public" | "internal" | "private";
  autoInit: boolean;
  gitignore?: string;
  license?: Schemas["license-simple"];
  isTemplate?: boolean;
  hasIssues?: boolean;
  hasWiki?: boolean;
  hasProjects?: boolean;
};

export function create(input: CreateInput): Schemas["full-repository"] {
  const now = new Date().toISOString();
  const repo = repository(input.name, input.description ?? "", input.visibility, {
    language: null,
    stars: 0,
    topics: [],
    pushed: now,
    owner: input.owner,
  });
  repo.created_at = now;
  repo.forks = repo.forks_count = 0;
  repo.size = input.autoInit ? 1 : 0;
  repo.homepage = input.homepage || null;
  repo.is_template = input.isTemplate ?? false;
  repo.has_issues = input.hasIssues ?? true;
  repo.has_wiki = input.hasWiki ?? false;
  repo.has_projects = input.hasProjects ?? false;
  repo.license = input.license
    ? {
        key: input.license.key,
        name: input.license.name,
        url: input.license.url,
        spdx_id: input.license.spdx_id,
        node_id: input.license.node_id,
      }
    : null;
  db.addRepo(repo);
  // An empty repository has no default branch until its first push.
  admin.create(repo, {
    branches: input.autoInit || input.license || input.gitignore ? ["main"] : [],
  });
  return full(repo);
}

export function generate(
  template: Repository,
  owner: SimpleUser,
  name: string,
  description: string | undefined,
  isPrivate: boolean,
  allBranches: boolean,
): Schemas["full-repository"] {
  const created = create({
    owner,
    name,
    description: description ?? template.description ?? "",
    visibility: isPrivate ? "private" : "public",
    autoInit: true,
  });
  const repo = db.state(owner.login, name)!.repo;
  repo.language = template.language;
  repo.topics = [...(template.topics ?? [])];
  const source = admin.state(template.owner.login, template.name)!.admin;
  admin.create(repo, {
    branches: allBranches ? [...source.branches] : [template.default_branch],
    template,
  });
  return { ...created, template_repository: template };
}

export function fork(
  parent: Repository,
  owner: SimpleUser,
  name: string,
  defaultBranchOnly: boolean,
): Schemas["full-repository"] {
  const now = new Date().toISOString();
  const repo = repository(name, parent.description ?? "", visibilityOf(parent), {
    language: parent.language,
    stars: 0,
    topics: [...(parent.topics ?? [])],
    pushed: parent.pushed_at ?? now,
    owner,
  });
  repo.fork = true;
  repo.created_at = now;
  repo.forks = repo.forks_count = 0;
  repo.license = parent.license;
  repo.default_branch = parent.default_branch;
  db.addRepo(repo);
  const source = admin.state(parent.owner.login, parent.name)!.admin;
  admin.create(repo, {
    branches: defaultBranchOnly ? [parent.default_branch] : [...source.branches],
    parent,
  });
  parent.forks_count += 1;
  parent.forks = parent.forks_count;
  return { ...full(repo), parent, source: parent };
}

// The repository as GET /repos/{owner}/{repo} answers it, plus its parent or template.
export function full(repo: Repository): Schemas["full-repository"] {
  const rec = admin.state(repo.owner.login, repo.name)?.admin;
  return {
    ...fullRepository(repo),
    organization: repo.owner.type === "Organization" ? users.org : null,
    ...(rec?.parent && { parent: rec.parent, source: rec.parent }),
    ...(rec?.template && { template_repository: rec.template }),
  };
}

export function rename(repo: Repository, name: string) {
  const moved = relocate(repo, repo.owner, name);
  db.moveRepo(repo.full_name, moved);
  return moved;
}

export function transfer(repo: Repository, owner: SimpleUser, name?: string) {
  const moved = relocate(repo, owner, name ?? repo.name);
  db.moveRepo(repo.full_name, moved);
  return moved;
}

export function remove(repo: Repository) {
  db.deleteRepo(repo.full_name);
  admin.drop(repo.id);
}

// The first free name for a fork: GHES suffixes -1, -2, and so on.
export function freeName(owner: string, wanted: string) {
  const base = normalizeRepoName(wanted);
  if (!taken(owner, base)) return base;
  for (let i = 1; ; i++) if (!taken(owner, `${base}-${i}`)) return `${base}-${i}`;
}

export const orgLogin = org.login;
