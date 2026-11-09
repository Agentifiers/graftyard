import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  CancelCircleIcon,
  CheckmarkCircle02Icon,
  Clock01Icon,
  Delete02Icon,
  Download04Icon,
  EyeIcon,
  FavouriteIcon,
  FileZipIcon,
  Happy01Icon,
  Loading03Icon,
  MinusSignCircleIcon,
  PartyIcon,
  Rocket01Icon,
  ThumbsUpIcon,
} from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api, type Schemas } from "@/api/client";
import { userQuery } from "@/areas/core/queries";
import { Badge, type BadgeProps } from "@/components/reui/badge";
import { FileType, getFileType } from "@/components/reui/file-type";
import { Button } from "@/components/ui/button";
import { cn } from "cn";
import { errorText, saveBlob, send } from "./api";
import { reactionsQuery, releasesKey } from "./queries";
import { useOptimisticWrite } from "./write";

type Repo = { owner: string; repo: string };
type Release = Schemas["release"];
type Asset = Schemas["release-asset"];

export function formatSize(n: number) {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

export function ReleaseBadges({ release, latest }: { release: Release; latest: boolean }) {
  return (
    <>
      {latest && (
        <Badge variant="success-light" data-badge="latest">
          Latest
        </Badge>
      )}
      {release.prerelease && (
        <Badge variant="warning-light" data-badge="prerelease">
          Pre-release
        </Badge>
      )}
      {release.draft && (
        <Badge variant="outline" data-badge="draft">
          Draft
        </Badge>
      )}
    </>
  );
}

// ---- Downloads: assets and source archives come back as files through the mock ----

function useDownload({ owner, repo }: Repo) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (
      target: { kind: "asset"; asset: Asset } | { kind: "tarball" | "zipball"; ref: string },
    ) => {
      if (target.kind === "asset") {
        const { response } = await api.GET("/repos/{owner}/{repo}/releases/assets/{asset_id}", {
          params: { path: { owner, repo, asset_id: target.asset.id } },
          headers: { Accept: "application/octet-stream" },
          parseAs: "stream",
        });
        if (!response.ok) throw new Error(`Download failed (HTTP ${response.status})`);
        return saveBlob(await response.blob(), response, target.asset.name);
      }
      const path =
        target.kind === "tarball"
          ? ("/repos/{owner}/{repo}/tarball/{ref}" as const)
          : ("/repos/{owner}/{repo}/zipball/{ref}" as const);
      const { response } = await api.GET(path, {
        params: { path: { owner, repo, ref: target.ref } },
        parseAs: "stream",
      });
      if (!response.ok) throw new Error(`Download failed (HTTP ${response.status})`);
      return saveBlob(await response.blob(), response, `${repo}-${target.ref}`);
    },
    onSuccess: (name, target) => {
      toast.success(`Downloaded ${name}`);
      if (target.kind === "asset") {
        void queryClient.invalidateQueries({ queryKey: releasesKey({ owner, repo }) });
      }
    },
    onError: (e) => toast.error("Download failed", { description: errorText(e) }),
  });
}

// The release's assets, then its two source archives, as GHES lists them.
export function AssetList({
  release,
  onDelete,
  ...repo
}: Repo & { release: Release; onDelete?: (asset: Asset) => void }) {
  const download = useDownload(repo);
  return (
    <ul className="divide-y border text-sm" data-testid="assets">
      {release.assets.map((a) => (
        <li key={a.id} className="flex items-center gap-2 px-3 py-1.5" data-asset={a.name}>
          <FileType type={getFileType(a.name)} size="mini" />
          <button
            type="button"
            className="text-primary min-w-0 flex-1 truncate text-left font-medium hover:underline"
            title={a.label ?? a.name}
            onClick={() => download.mutate({ kind: "asset", asset: a })}
          >
            {a.label || a.name}
          </button>
          <span className="text-muted-foreground w-16 shrink-0 text-right text-xs tabular-nums">
            {formatSize(a.size)}
          </span>
          <span
            className="text-muted-foreground flex shrink-0 items-center gap-1 text-xs tabular-nums"
            title={`${a.download_count} downloads`}
            data-downloads={a.download_count}
          >
            <HugeiconsIcon icon={Download04Icon} strokeWidth={2} className="size-3.5" />
            <span className="w-8 text-right">{a.download_count}</span>
          </span>
          {onDelete && (
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label={`Delete ${a.name}`}
              onClick={() => onDelete(a)}
            >
              <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
            </Button>
          )}
        </li>
      ))}
      {!release.draft &&
        (["zipball", "tarball"] as const).map((kind) => (
          <li key={kind} className="flex items-center gap-2 px-3 py-1.5">
            <HugeiconsIcon
              icon={FileZipIcon}
              strokeWidth={2}
              className="text-muted-foreground size-4.5"
            />
            <button
              type="button"
              className="text-primary font-medium hover:underline"
              data-archive={kind}
              onClick={() => download.mutate({ kind, ref: release.tag_name })}
            >
              Source code ({kind === "zipball" ? "zip" : "tar.gz"})
            </button>
          </li>
        ))}
    </ul>
  );
}

// ---- Reactions ----

const REACTIONS = [
  ["+1", ThumbsUpIcon],
  ["laugh", Happy01Icon],
  ["hooray", PartyIcon],
  ["heart", FavouriteIcon],
  ["rocket", Rocket01Icon],
  ["eyes", EyeIcon],
] as const;
type ReactionContent = (typeof REACTIONS)[number][0];

export function Reactions({ id, ...repo }: Repo & { id: number }) {
  const query = reactionsQuery({ ...repo, id });
  const { data: reactions } = useSuspenseQuery(query);
  const { data: viewer } = useSuspenseQuery(userQuery());
  const mine = (c: ReactionContent) =>
    reactions.find((r) => r.content === c && r.user?.login === viewer.login);
  const toggle = useOptimisticWrite({
    key: query.queryKey,
    invalidate: [query.queryKey, releasesKey(repo)],
    failure: "Reaction not saved",
    apply: (list: Schemas["reaction"][], c: ReactionContent) => {
      const own = list.find((r) => r.content === c && r.user?.login === viewer.login);
      if (own) return list.filter((r) => r !== own);
      return [
        ...list,
        {
          id: -Date.now(),
          node_id: "",
          user: viewer,
          content: c,
          created_at: new Date().toISOString(),
        },
      ];
    },
    write: async (c: ReactionContent) => {
      const own = mine(c);
      if (own) {
        await send(
          api.DELETE("/repos/{owner}/{repo}/releases/{release_id}/reactions/{reaction_id}", {
            params: { path: { ...repo, release_id: id, reaction_id: own.id } },
          }),
        );
        return;
      }
      await send(
        api.POST("/repos/{owner}/{repo}/releases/{release_id}/reactions", {
          params: { path: { ...repo, release_id: id } },
          body: { content: c },
        }),
      );
    },
  });
  return (
    <div className="flex flex-wrap gap-1" data-testid="reactions">
      {REACTIONS.map(([c, icon]) => {
        const count = reactions.filter((r) => r.content === c).length;
        const on = Boolean(mine(c));
        return (
          <button
            key={c}
            type="button"
            aria-pressed={on}
            aria-label={`${c} (${count})`}
            data-reaction={c}
            onClick={() => toggle.mutate(c)}
            className={cn(
              "hover:bg-muted flex h-6 items-center gap-1 rounded-sm border px-1.5 text-xs tabular-nums",
              on && "border-primary/50 bg-primary/10",
              count === 0 && !on && "text-muted-foreground opacity-60 hover:opacity-100",
            )}
          >
            <HugeiconsIcon icon={icon} strokeWidth={2} className="size-3.5" aria-hidden />
            {count > 0 && count}
          </button>
        );
      })}
    </div>
  );
}

// ---- Deployment states ----

type DeploymentState = Schemas["deployment-status"]["state"];

const STATES: Record<
  DeploymentState,
  { label: string; variant: BadgeProps["variant"]; icon: typeof Clock01Icon; spin?: boolean }
> = {
  queued: { label: "Queued", variant: "outline", icon: Clock01Icon },
  pending: { label: "Pending", variant: "outline", icon: Clock01Icon },
  in_progress: { label: "In progress", variant: "warning-light", icon: Loading03Icon, spin: true },
  success: { label: "Active", variant: "success-light", icon: CheckmarkCircle02Icon },
  failure: { label: "Failed", variant: "destructive-light", icon: CancelCircleIcon },
  error: { label: "Error", variant: "destructive-light", icon: CancelCircleIcon },
  inactive: { label: "Inactive", variant: "secondary", icon: MinusSignCircleIcon },
};

export function DeploymentBadge({ state }: { state: DeploymentState | undefined }) {
  const s = STATES[state ?? "pending"];
  return (
    <Badge
      variant={s.variant}
      data-state={state}
      className={cn("shrink-0", (state === "failure" || state === "error") && "dark:text-red-300")}
    >
      <HugeiconsIcon icon={s.icon} strokeWidth={2} className={cn(s.spin && "animate-spin")} />
      {s.label}
    </Badge>
  );
}

export function EnvironmentLink({ name, ...repo }: Repo & { name: string }) {
  return (
    <Link
      to="/$owner/$repo/deployments"
      params={repo}
      search={{ environment: name }}
      className="font-medium hover:underline"
    >
      {name}
    </Link>
  );
}
