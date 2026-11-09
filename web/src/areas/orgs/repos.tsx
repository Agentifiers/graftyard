import { Link } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { GitForkIcon, StarIcon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { Ago } from "@/components/github";
import { Badge } from "@/components/reui/badge";
import { EmptyRow } from "./kit";

// Repository rows and cards the profile, the org overview, and the org repository list
// share, plus the language colours GitHub uses.

// The fields the rows and cards read, which minimal-repository and repository share.
type Repo = Pick<
  Schemas["minimal-repository"],
  | "id"
  | "name"
  | "full_name"
  | "owner"
  | "description"
  | "topics"
  | "language"
  | "stargazers_count"
  | "forks_count"
  | "pushed_at"
  | "visibility"
  | "private"
  | "archived"
>;

const LANGUAGE_COLOURS: Record<string, string> = {
  Rust: "#dea584",
  TypeScript: "#3178c6",
  JavaScript: "#f1e05a",
  Shell: "#89e051",
  Go: "#00add8",
  Python: "#3572a5",
  HTML: "#e34c26",
  CSS: "#563d7c",
};

export function Language({ name }: { name: string | null | undefined }) {
  if (!name) return null;
  return (
    <span className="flex items-center gap-1" data-language={name}>
      <span
        className="inline-block size-2.5 rounded-sm"
        style={{ backgroundColor: LANGUAGE_COLOURS[name] ?? "#8b949e" }}
        aria-hidden
      />
      {name}
    </span>
  );
}

export function Visibility({ repo }: { repo: Pick<Repo, "visibility" | "private" | "archived"> }) {
  const v = repo.visibility ?? (repo.private ? "private" : "public");
  return (
    <>
      <Badge variant="outline" size="sm" className="capitalize">
        {v}
      </Badge>
      {repo.archived && (
        <Badge variant="warning-light" size="sm">
          Archived
        </Badge>
      )}
    </>
  );
}

export function RepoRows({ repos, empty }: { repos: Repo[]; empty: string }) {
  if (!repos.length) return <EmptyRow>{empty}</EmptyRow>;
  return (
    <ul className="divide-y" data-testid="repo-rows">
      {repos.map((r) => (
        <li key={r.id} className="flex flex-col gap-1 px-4 py-3" data-repo={r.full_name}>
          <div className="flex flex-wrap items-center gap-2">
            <Link
              to="/$owner/$repo"
              params={{ owner: r.owner.login, repo: r.name }}
              className="text-primary text-base font-semibold hover:underline"
            >
              {r.name}
            </Link>
            <Visibility repo={r} />
          </div>
          {r.description && <p className="text-muted-foreground text-sm">{r.description}</p>}
          {r.topics && r.topics.length > 0 && (
            <div className="flex flex-wrap gap-1">
              {r.topics.map((t) => (
                <Badge key={t} variant="info-light" size="sm">
                  {t}
                </Badge>
              ))}
            </div>
          )}
          <div className="text-muted-foreground flex flex-wrap items-center gap-4 text-xs">
            <Language name={r.language} />
            <Link
              to="/$owner/$repo/stargazers"
              params={{ owner: r.owner.login, repo: r.name }}
              className="hover:text-primary flex items-center gap-1"
            >
              <HugeiconsIcon icon={StarIcon} strokeWidth={2} className="size-3.5" />
              {r.stargazers_count ?? 0}
            </Link>
            <span className="flex items-center gap-1">
              <HugeiconsIcon icon={GitForkIcon} strokeWidth={2} className="size-3.5" />
              {r.forks_count ?? 0}
            </span>
            {r.pushed_at && (
              <span>
                Updated <Ago at={r.pushed_at} />
              </span>
            )}
          </div>
        </li>
      ))}
    </ul>
  );
}

export function RepoCards({ repos }: { repos: Repo[] }) {
  return (
    <ul className="grid gap-3 sm:grid-cols-2" data-testid="repo-cards">
      {repos.map((r) => (
        <li
          key={r.id}
          className="bg-card flex flex-col gap-2 rounded-sm border p-3"
          data-pinned={r.full_name}
        >
          <div className="flex items-center gap-2">
            <Link
              to="/$owner/$repo"
              params={{ owner: r.owner.login, repo: r.name }}
              className="text-primary truncate text-sm font-semibold hover:underline"
            >
              {r.owner.type === "Organization" ? r.full_name : r.name}
            </Link>
            <Visibility repo={r} />
          </div>
          <p className="text-muted-foreground line-clamp-2 flex-1 text-xs">
            {r.description ?? "No description."}
          </p>
          <div className="text-muted-foreground flex items-center gap-4 text-xs">
            <Language name={r.language} />
            <span className="flex items-center gap-1">
              <HugeiconsIcon icon={StarIcon} strokeWidth={2} className="size-3.5" />
              {r.stargazers_count ?? 0}
            </span>
            <span className="flex items-center gap-1">
              <HugeiconsIcon icon={GitForkIcon} strokeWidth={2} className="size-3.5" />
              {r.forks_count ?? 0}
            </span>
          </div>
        </li>
      ))}
    </ul>
  );
}

// Filters the repository lists apply on the client, after the server's type and sort.
export function matchRepo(r: Repo, q: string, language: string) {
  const text = q.trim().toLowerCase();
  const inText =
    !text ||
    r.name.toLowerCase().includes(text) ||
    (r.description ?? "").toLowerCase().includes(text) ||
    (r.topics ?? []).some((t) => t.includes(text));
  return inText && (!language || r.language === language);
}
