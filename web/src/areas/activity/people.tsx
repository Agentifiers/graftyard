import type { ReactNode } from "react";
import { Link } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { UserGroupIcon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { Ago, UserAvatar } from "@/components/github";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";

// The grid of people the stargazers and watchers pages share.

type Person = Schemas["simple-user"];

export function PeopleGrid({
  people,
  empty,
}: {
  people: Person[];
  empty: { title: string; body: string };
}) {
  if (!people.length) {
    return (
      <Empty className="py-12" data-testid="people-empty">
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <HugeiconsIcon icon={UserGroupIcon} strokeWidth={2} />
          </EmptyMedia>
          <EmptyTitle>{empty.title}</EmptyTitle>
          <EmptyDescription>{empty.body}</EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  }
  return (
    <ul className="grid gap-px sm:grid-cols-2 lg:grid-cols-3" data-testid="people">
      {people.map((u) => (
        <li key={u.login} className="flex items-center gap-3 border-b p-3 sm:border-r">
          <UserAvatar user={u} size="default" />
          <div className="min-w-0">
            <Link
              to="/$owner"
              params={{ owner: u.login }}
              className="text-primary block truncate text-sm font-semibold hover:underline"
            >
              {u.login}
            </Link>
            {u.name && <p className="text-muted-foreground truncate text-xs">{u.name}</p>}
            {u.starred_at && (
              <p className="text-muted-foreground text-xs">
                Starred <Ago at={u.starred_at} />
              </p>
            )}
          </div>
        </li>
      ))}
    </ul>
  );
}

export function PeopleTabs({
  owner,
  repo,
  active,
}: {
  owner: string;
  repo: string;
  active: "stargazers" | "watchers";
}) {
  const tab = (
    to: "/$owner/$repo/stargazers" | "/$owner/$repo/watchers",
    label: ReactNode,
    key: string,
  ) => (
    <Link
      to={to}
      params={{ owner, repo }}
      role="tab"
      aria-selected={active === key}
      className="text-muted-foreground aria-selected:bg-muted aria-selected:text-foreground rounded-sm px-2.5 py-1 text-sm font-medium"
    >
      {label}
    </Link>
  );
  return (
    <div
      className="bg-background flex w-fit rounded-sm border p-0.5"
      role="tablist"
      aria-label="People"
    >
      {tab("/$owner/$repo/stargazers", "Stargazers", "stargazers")}
      {tab("/$owner/$repo/watchers", "Watchers", "watchers")}
    </div>
  );
}
