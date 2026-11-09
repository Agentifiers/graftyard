import { useState } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { useHotkey } from "@tanstack/react-hotkeys";
import { useDebouncedValue } from "@tanstack/react-pacer";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  DashboardSpeed01Icon,
  Home01Icon,
  InboxIcon,
  Notification03Icon,
  NotificationOff02Icon,
  RepositoryIcon,
  Search01Icon,
  StarIcon,
  UserIcon,
} from "@hugeicons/core-free-icons";
import { issueState, pullState, StateIcon, UserAvatar } from "@/components/github";
import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
  CommandShortcut,
} from "@/components/ui/command";
import { Kbd } from "@/components/ui/kbd";
import { Spinner } from "@/components/ui/spinner";
import { userQuery } from "@/areas/core/queries";
import { inboxQuery, searchQuery } from "./queries";

// The top bar's search box and notifications bell. The box opens a command palette that
// searches repositories, issues, pull requests, and people as you type.

export function SearchBox() {
  const [open, setOpen] = useState(false);
  useHotkey("/", () => setOpen(true));
  useHotkey("Mod+K", () => setOpen((o) => !o));
  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="text-muted-foreground hover:border-ring bg-background flex h-7 w-full max-w-72 min-w-0 items-center gap-2 rounded-sm border px-2 text-left text-xs"
        aria-label="Search or jump to"
        data-testid="search-box"
      >
        <HugeiconsIcon icon={Search01Icon} strokeWidth={2} className="size-3.5 shrink-0" />
        <span className="flex-1 truncate">Search or jump to…</span>
        <Kbd>/</Kbd>
      </button>
      <CommandDialog
        open={open}
        onOpenChange={setOpen}
        title="Search or jump to"
        description="Search repositories, issues, pull requests, and people, or go to a page."
        className="rounded-sm! sm:max-w-xl"
      >
        {open && <Palette close={() => setOpen(false)} />}
      </CommandDialog>
    </>
  );
}

const GO_TO = [
  { label: "Home", to: "/", icon: Home01Icon, keys: "g d" },
  { label: "Notifications", to: "/notifications", icon: InboxIcon, keys: "g n" },
  { label: "API status and rate limit", to: "/rate-limit", icon: DashboardSpeed01Icon, keys: "" },
] as const;

function Palette({ close }: { close: () => void }) {
  const navigate = useNavigate();
  const [input, setInput] = useState("");
  // Typing fires one search per pause, not one per key.
  const [q] = useDebouncedValue(input.trim(), { wait: 250 });
  const enabled = q.length >= 2;
  const repos = useQuery({ ...searchQuery("repositories", { q, perPage: 5 }), enabled });
  const issues = useQuery({ ...searchQuery("issues", { q, perPage: 5 }), enabled });
  const pulls = useQuery({ ...searchQuery("pullrequests", { q, perPage: 5 }), enabled });
  const people = useQuery({ ...searchQuery("users", { q, perPage: 5 }), enabled });
  const all = [repos, issues, pulls, people];
  const loading = enabled && (input.trim() !== q || all.some((r) => r.isFetching));
  const failed = all.filter((r) => r.isError).length;
  const items = <T,>(r: { data?: { kind: string; items?: T[] } }) =>
    r.data?.kind === "ok" ? ((r.data as { items: T[] }).items ?? []) : [];

  const go = (fn: () => void) => {
    close();
    fn();
  };
  const viewer = useQuery(userQuery()).data?.login ?? "";
  const goTo = GO_TO.filter((g) => !input || g.label.toLowerCase().includes(input.toLowerCase()));

  return (
    <Command shouldFilter={false} className="rounded-sm!" loop>
      <CommandInput
        placeholder="Search or type a command"
        value={input}
        onValueChange={setInput}
        aria-label="Search"
      />
      <CommandList className="max-h-96" data-testid="palette-results">
        {loading && (
          <div className="text-muted-foreground flex items-center gap-2 px-3 py-2 text-xs">
            <Spinner className="size-3" /> Searching…
          </div>
        )}
        {failed > 0 && !loading && (
          <p className="text-destructive px-3 py-2 text-xs" data-testid="palette-error">
            Search is unavailable for{" "}
            {failed === all.length ? "every type" : `${failed} of ${all.length} types`}.
          </p>
        )}
        <CommandEmpty>{enabled ? "Nothing matches." : "Type two or more characters."}</CommandEmpty>
        {enabled && input.trim() && (
          <CommandGroup heading="Search">
            <CommandItem
              value={`search:${input}`}
              onSelect={() => go(() => navigate({ to: "/search", search: { q: input.trim() } }))}
            >
              <HugeiconsIcon icon={Search01Icon} strokeWidth={2} />
              <span className="flex-1 truncate">
                Search everything for <strong>{input.trim()}</strong>
              </span>
              <CommandShortcut>Enter</CommandShortcut>
            </CommandItem>
          </CommandGroup>
        )}
        {items(repos).length > 0 && (
          <CommandGroup heading="Repositories">
            {items<{
              id: number;
              full_name: string;
              owner: { login: string } | null;
              name: string;
              stargazers_count: number;
            }>(repos).map((r) => (
              <CommandItem
                key={r.id}
                value={`repo:${r.full_name}`}
                onSelect={() =>
                  go(() =>
                    navigate({
                      to: "/$owner/$repo",
                      params: { owner: r.owner?.login ?? "", repo: r.name },
                    }),
                  )
                }
              >
                <HugeiconsIcon icon={RepositoryIcon} strokeWidth={2} />
                <span className="flex-1 truncate">{r.full_name}</span>
                <span className="text-muted-foreground ml-auto flex items-center gap-1 text-xs">
                  <HugeiconsIcon icon={StarIcon} strokeWidth={2} className="size-3" />
                  {r.stargazers_count}
                </span>
              </CommandItem>
            ))}
          </CommandGroup>
        )}
        {[
          { heading: "Issues", r: issues },
          { heading: "Pull requests", r: pulls },
        ].map(({ heading, r }) =>
          items(r).length > 0 ? (
            <CommandGroup key={heading} heading={heading}>
              {items<IssueHit>(r).map((i) => {
                const [owner, repo] = i.repository_url.split("/repos/")[1].split("/");
                return (
                  <CommandItem
                    key={i.id}
                    value={`issue:${i.id}`}
                    onSelect={() =>
                      go(() =>
                        i.pull_request
                          ? navigate({
                              to: "/$owner/$repo/pull/$number",
                              params: { owner, repo, number: i.number },
                            })
                          : navigate({
                              to: "/$owner/$repo/issues/$number",
                              params: { owner, repo, number: i.number },
                            }),
                      )
                    }
                  >
                    <StateIcon
                      state={
                        i.pull_request
                          ? pullState({
                              state: i.state,
                              draft: i.draft,
                              merged_at: i.pull_request.merged_at ?? null,
                            })
                          : issueState(i)
                      }
                    />
                    <span className="flex-1 truncate">{i.title}</span>
                    <span className="text-muted-foreground ml-auto shrink-0 text-xs">
                      {repo}#{i.number}
                    </span>
                  </CommandItem>
                );
              })}
            </CommandGroup>
          ) : null,
        )}
        {items(people).length > 0 && (
          <CommandGroup heading="People">
            {items<{ id: number; login: string; avatar_url: string; name?: string | null }>(
              people,
            ).map((u) => (
              <CommandItem
                key={u.id}
                value={`user:${u.login}`}
                onSelect={() => go(() => navigate({ to: "/$owner", params: { owner: u.login } }))}
              >
                <UserAvatar user={u} />
                <span>{u.login}</span>
                <span className="text-muted-foreground flex-1 truncate text-xs">{u.name}</span>
              </CommandItem>
            ))}
          </CommandGroup>
        )}
        {(goTo.length > 0 || !input) && (
          <>
            <CommandSeparator />
            <CommandGroup heading="Go to">
              {goTo.map((g) => (
                <CommandItem
                  key={g.to}
                  value={`go:${g.to}`}
                  onSelect={() => go(() => navigate({ to: g.to }))}
                >
                  <HugeiconsIcon icon={g.icon} strokeWidth={2} />
                  {g.label}
                </CommandItem>
              ))}
              {(!input || "your profile".includes(input.toLowerCase())) && (
                <CommandItem
                  value="go:profile"
                  onSelect={() => go(() => navigate({ to: "/$owner", params: { owner: viewer } }))}
                >
                  <HugeiconsIcon icon={UserIcon} strokeWidth={2} />
                  Your profile
                </CommandItem>
              )}
            </CommandGroup>
          </>
        )}
      </CommandList>
    </Command>
  );
}

type IssueHit = {
  id: number;
  number: number;
  title: string;
  state: string;
  draft?: boolean;
  repository_url: string;
  pull_request?: { merged_at?: string | null };
};

// The bell: a square dot when anything in the inbox is unread. It shares the inbox's
// polled query, so a new notification lights it without a reload. It does not suspend:
// the top bar slot is one bare region, and a slow or failing inbox must not hide search.
export function NotificationBell() {
  const { data, isError } = useQuery(inboxQuery());
  const unread = data?.threads.filter((t) => t.unread).length ?? 0;
  return (
    <Link
      to="/notifications"
      className="hover:bg-muted relative flex size-7 shrink-0 items-center justify-center rounded-sm"
      aria-label={
        isError
          ? "Notifications (could not load)"
          : unread
            ? `Notifications, ${unread} unread`
            : "Notifications"
      }
      title={isError ? "Could not load notifications" : undefined}
      data-testid="notification-bell"
      data-unread={unread}
      data-error={isError || undefined}
    >
      <HugeiconsIcon
        icon={isError ? NotificationOff02Icon : Notification03Icon}
        strokeWidth={2}
        className={isError ? "text-muted-foreground size-4" : "size-4"}
      />
      {unread > 0 && (
        <span
          className="bg-primary ring-card absolute top-1 right-1 size-2 ring-2"
          data-testid="unread-dot"
          aria-hidden
        />
      )}
    </Link>
  );
}
