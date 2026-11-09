import { useState } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  ArrowDown01Icon,
  Copy01Icon,
  GitBranchIcon,
  GitCommitIcon,
  Tag01Icon,
  Tick02Icon,
} from "@hugeicons/core-free-icons";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { Ago, UserAvatar } from "@/components/github";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "cn";
import { branchesQuery, tagsQuery } from "./queries";
import type { CommitSummary } from "./ui-api";

// Pieces every code page shares: the splat, the branch and tag picker, breadcrumbs, the
// latest-commit bar, and copy buttons.

export type Repo = { owner: string; repo: string };

export const short = (s: string) => s.slice(0, 7);
export const firstLine = (m: string) => m.split("\n")[0];
export const joinPath = (...parts: string[]) => parts.filter(Boolean).join("/");

export function CopyButton({
  value,
  label,
  className,
}: {
  value: string;
  label: string;
  className?: string;
}) {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  return (
    <Button
      variant="ghost"
      size="icon-xs"
      className={className}
      aria-label={isCopied ? "Copied" : label}
      title={label}
      onClick={() => copyToClipboard(value)}
    >
      <HugeiconsIcon icon={isCopied ? Tick02Icon : Copy01Icon} strokeWidth={2} />
    </Button>
  );
}

// Branches and tags, filtered as you type. `to` decides where a pick goes.
export function RefPicker({
  owner,
  repo,
  current,
  onPick,
  label = "Switch branches or tags",
}: Repo & { current: string; onPick: (ref: string) => void; label?: string }) {
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState<"branches" | "tags">("branches");
  const [q, setQ] = useState("");
  const branches = useQuery({ ...branchesQuery({ owner, repo }), enabled: open });
  const tags = useQuery({ ...tagsQuery({ owner, repo }), enabled: open });
  const names = (tab === "branches" ? branches.data : tags.data)?.map((b) => b.name) ?? [];
  const shown = names.filter((n) => n.toLowerCase().includes(q.toLowerCase()));
  const loading = tab === "branches" ? branches.isPending : tags.isPending;
  const error = tab === "branches" ? branches.error : tags.error;
  const isSha = /^[0-9a-f]{40}$/.test(current);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        render={
          <Button
            variant="outline"
            size="sm"
            className="max-w-56"
            aria-label={label}
            data-testid="ref-picker"
          >
            <HugeiconsIcon
              icon={isSha ? GitCommitIcon : GitBranchIcon}
              strokeWidth={2}
              data-icon="inline-start"
            />
            <span className="truncate font-mono text-xs">{isSha ? short(current) : current}</span>
            <HugeiconsIcon icon={ArrowDown01Icon} strokeWidth={2} data-icon="inline-end" />
          </Button>
        }
      />
      <PopoverContent align="start" className="w-72 gap-2 p-2">
        <p className="text-xs font-semibold">{label}</p>
        <Input
          autoFocus
          placeholder={tab === "branches" ? "Find a branch" : "Find a tag"}
          value={q}
          onChange={(e) => setQ(e.target.value)}
          aria-label="Filter refs"
        />
        <div className="flex border-b text-xs" role="tablist">
          {(["branches", "tags"] as const).map((t) => (
            <button
              key={t}
              type="button"
              role="tab"
              aria-selected={tab === t}
              onClick={() => setTab(t)}
              className="text-muted-foreground aria-selected:border-primary aria-selected:text-foreground -mb-px border-b-2 border-transparent px-2 py-1 font-medium capitalize"
            >
              {t}
            </button>
          ))}
        </div>
        <ul className="-mx-1 max-h-64 overflow-y-auto" role="listbox" aria-label={tab}>
          {loading && (
            <li className="flex flex-col gap-1 p-1">
              <Skeleton className="h-5" />
              <Skeleton className="h-5 w-2/3" />
            </li>
          )}
          {error && (
            <li className="text-destructive p-2 text-xs">
              Could not load {tab}: {error.message}
            </li>
          )}
          {!loading && !error && shown.length === 0 && (
            <li className="text-muted-foreground p-2 text-xs">Nothing to show.</li>
          )}
          {shown.map((name) => (
            <li key={name}>
              <button
                type="button"
                role="option"
                aria-selected={name === current}
                className="hover:bg-muted flex w-full items-center gap-2 rounded-sm px-2 py-1 text-left text-sm"
                onClick={() => {
                  setOpen(false);
                  onPick(name);
                }}
              >
                <HugeiconsIcon
                  icon={Tick02Icon}
                  strokeWidth={2}
                  className={cn("size-3.5 shrink-0", name !== current && "invisible")}
                />
                <HugeiconsIcon
                  icon={tab === "tags" ? Tag01Icon : GitBranchIcon}
                  strokeWidth={2}
                  className="text-muted-foreground size-3.5 shrink-0"
                />
                <span className="truncate font-mono text-xs">{name}</span>
              </button>
            </li>
          ))}
        </ul>
        <div className="flex gap-2 border-t pt-2 text-xs">
          <Link
            to="/$owner/$repo/branches"
            params={{ owner, repo }}
            className="text-primary hover:underline"
          >
            View all branches
          </Link>
          <Link
            to="/$owner/$repo/tags"
            params={{ owner, repo }}
            className="text-primary ml-auto hover:underline"
          >
            View all tags
          </Link>
        </div>
      </PopoverContent>
    </Popover>
  );
}

// A picker that keeps the same kind of page and path, at another ref.
export function useSwitchRef(
  kind: "tree" | "blob" | "blame" | "commits",
  { owner, repo }: Repo,
  path: string,
) {
  const navigate = useNavigate();
  return (ref: string) =>
    navigate({
      to: `/$owner/$repo/${kind}/$`,
      params: { owner, repo, _splat: joinPath(ref, path) },
    });
}

export function Breadcrumbs({
  owner,
  repo,
  ref,
  path,
  leaf = "blob",
}: Repo & { ref: string; path: string; leaf?: "blob" | "tree" }) {
  const parts = path.split("/").filter(Boolean);
  return (
    <nav
      aria-label="Path"
      className="flex min-w-0 flex-wrap items-center gap-1 text-sm"
      data-testid="breadcrumbs"
    >
      <Link
        to="/$owner/$repo/tree/$"
        params={{ owner, repo, _splat: ref }}
        className="text-primary font-semibold hover:underline"
      >
        {repo}
      </Link>
      {parts.map((part, i) => {
        const sub = parts.slice(0, i + 1).join("/");
        const last = i === parts.length - 1;
        return (
          <span key={sub} className="flex items-center gap-1">
            <span className="text-muted-foreground">/</span>
            {last ? (
              <span className="font-semibold" aria-current="page">
                {part}
              </span>
            ) : (
              <Link
                to={leaf === "tree" || !last ? "/$owner/$repo/tree/$" : "/$owner/$repo/blob/$"}
                params={{ owner, repo, _splat: joinPath(ref, sub) }}
                className="text-primary hover:underline"
              >
                {part}
              </Link>
            )}
          </span>
        );
      })}
    </nav>
  );
}

export function Author({ author }: { author: CommitSummary["author"] }) {
  const user = author.login ? { login: author.login, avatar_url: author.avatar_url ?? "" } : null;
  return (
    <span className="flex min-w-0 items-center gap-1.5">
      <UserAvatar user={user} />
      <span className="truncate font-semibold">{author.login ?? author.name}</span>
    </span>
  );
}

// The commit bar above a tree or file: who changed it last, when, and the history link.
export function LatestCommit({
  owner,
  repo,
  commit,
  count,
  historyTo,
}: Repo & { commit: CommitSummary; count?: number; historyTo: string }) {
  return (
    <div
      className="bg-muted/40 flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2 text-sm"
      data-testid="latest-commit"
    >
      <Author author={commit.author} />
      <Link
        to="/$owner/$repo/commit/$sha"
        params={{ owner, repo, sha: commit.sha }}
        className="text-muted-foreground hover:text-foreground min-w-0 flex-1 truncate hover:underline"
      >
        {firstLine(commit.message)}
      </Link>
      <span className="text-muted-foreground flex items-center gap-2 text-xs">
        <Link
          to="/$owner/$repo/commit/$sha"
          params={{ owner, repo, sha: commit.sha }}
          className="font-mono hover:underline"
        >
          {short(commit.sha)}
        </Link>
        <span>·</span>
        <Ago at={commit.date} />
        <Link
          to="/$owner/$repo/commits/$"
          params={{ owner, repo, _splat: historyTo }}
          className="text-foreground flex items-center gap-1 font-medium hover:underline"
        >
          <HugeiconsIcon icon={GitCommitIcon} strokeWidth={2} className="size-3.5" />
          {count === undefined ? "History" : `${count} ${count === 1 ? "commit" : "commits"}`}
        </Link>
      </span>
    </div>
  );
}

export function RefKind({ kind }: { kind: "branch" | "tag" | "commit" }) {
  return (
    <Badge variant="outline" size="sm" className="capitalize">
      {kind}
    </Badge>
  );
}

// The branch names the editor can commit to. Suspends.
export function useBranchNames(r: Repo) {
  return useSuspenseQuery({ ...branchesQuery(r), select: (b) => b.map((x) => x.name) }).data;
}
