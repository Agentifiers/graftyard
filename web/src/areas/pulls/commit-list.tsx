import { Link } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { GitCommitIcon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { Ago, UserAvatar } from "@/components/github";
import { Badge } from "@/components/reui/badge";

type Commit = Schemas["commit"];

const login = (c: Commit): Schemas["simple-user"] | null =>
  c.author && "login" in c.author ? (c.author as Schemas["simple-user"]) : null;

// Commits grouped by day, newest day last, as GitHub lists a pull request's commits.
export function CommitList({
  owner,
  repo,
  commits,
}: {
  owner: string;
  repo: string;
  commits: Commit[];
}) {
  if (commits.length === 0)
    return <p className="text-muted-foreground px-3 py-6 text-center text-sm">No commits.</p>;
  const days = new Map<string, Commit[]>();
  for (const c of commits) {
    const day = (c.commit.author?.date ?? "").slice(0, 10);
    days.set(day, [...(days.get(day) ?? []), c]);
  }
  return (
    <div className="flex flex-col gap-3" data-testid="commits">
      {[...days].map(([day, list]) => (
        <section key={day} className="flex flex-col gap-1.5">
          <h3 className="text-muted-foreground flex items-center gap-1.5 text-xs">
            <HugeiconsIcon icon={GitCommitIcon} strokeWidth={2} className="size-3.5" />
            Commits on{" "}
            {new Date(`${day}T00:00:00Z`).toLocaleDateString("en", {
              month: "short",
              day: "numeric",
              year: "numeric",
              timeZone: "UTC",
            })}
          </h3>
          <ul className="divide-y rounded-sm border">
            {list.map((c) => {
              const [title, ...rest] = c.commit.message.split("\n");
              const who = login(c);
              return (
                <li key={c.sha} className="flex items-start gap-3 px-3 py-2" data-sha={c.sha}>
                  <UserAvatar user={who} />
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium" title={c.commit.message}>
                      {title}
                    </p>
                    {rest.join("\n").trim() && (
                      <p className="text-muted-foreground line-clamp-2 text-xs whitespace-pre-line">
                        {rest.join("\n").trim()}
                      </p>
                    )}
                    <p className="text-muted-foreground text-xs">
                      {who?.login ?? c.commit.author?.name} committed{" "}
                      {c.commit.author?.date && <Ago at={c.commit.author.date} />}
                      {c.parents.length > 1 && " · merge commit"}
                    </p>
                  </div>
                  {c.commit.verification?.verified && (
                    <Badge variant="success-outline" size="sm">
                      Verified
                    </Badge>
                  )}
                  <Link
                    to="/$owner/$repo/commit/$sha"
                    params={{ owner, repo, sha: c.sha }}
                    className="text-muted-foreground hover:text-foreground font-mono text-xs hover:underline"
                  >
                    {c.sha.slice(0, 7)}
                  </Link>
                </li>
              );
            })}
          </ul>
        </section>
      ))}
    </div>
  );
}
