import { Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { StarIcon } from "@hugeicons/core-free-icons";
import { Panel } from "@/components/panel";
// Type-only: the registry imports this module while it is still evaluating, so calling
// its `slot()` here would run before `slot` exists. Plain entries typed as SlotEntry get
// the same prop checking.
import type { SlotEntry } from "../slots";
import { ActivityFeed } from "./feed";
import { myStarsQuery } from "./queries";
import { StarButton, WatchButton } from "./repo-actions";
import { NotificationBell, SearchBox } from "./topbar";

function TopBar() {
  return (
    <div className="flex min-w-0 flex-1 items-center justify-end gap-1 sm:justify-start">
      <SearchBox />
      <span className="flex-1" />
      <NotificationBell />
    </div>
  );
}

function RepoActions(r: { owner: string; repo: string }) {
  return (
    <div className="flex items-center gap-2">
      <WatchButton {...r} />
      <StarButton {...r} />
    </div>
  );
}

function YourStars() {
  const { data } = useSuspenseQuery(myStarsQuery());
  return (
    <Panel title="Your stars" flush>
      {data.length === 0 ? (
        <p className="text-muted-foreground px-3 py-4 text-sm">
          Star a repository to keep it here.
        </p>
      ) : (
        <ul className="divide-y text-sm" data-testid="your-stars">
          {data.map((r) => (
            <li key={r.id} className="flex items-center gap-2 px-3 py-2">
              <HugeiconsIcon icon={StarIcon} strokeWidth={2} className="text-warning size-3.5" />
              <Link
                to="/$owner/$repo"
                params={{ owner: r.owner.login, repo: r.name }}
                className="truncate font-medium hover:underline"
              >
                {r.full_name}
              </Link>
              <span className="text-muted-foreground ml-auto text-xs tabular-nums">
                {r.stargazers_count}
              </span>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

export const slots: SlotEntry[] = [
  { slot: "topbar", region: "notifications", order: 0, Component: TopBar },
  { slot: "home", region: "activity feed", order: 0, Component: ActivityFeed },
  { slot: "home", region: "your stars", order: 1, Component: YourStars },
  { slot: "repo-actions", region: "watch and star", order: 10, Component: RepoActions },
];
