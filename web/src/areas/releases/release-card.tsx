import type { ReactNode } from "react";
import { Link } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { GitBranchIcon, Tag01Icon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { Ago, UserAvatar } from "@/components/github";
import { MarkdownBody } from "@/components/markdown";
import { Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { AssetList, Reactions, ReleaseBadges } from "./parts";

type Repo = { owner: string; repo: string };
type Release = Schemas["release"];

// One release as the list and the release page draw it: when and by whom on the left,
// then the title, badges, notes, assets, source archives, and reactions.
export function ReleaseCard({
  release,
  latest,
  actions,
  ...repo
}: Repo & { release: Release; latest: boolean; actions?: ReactNode }) {
  const title = release.name || release.tag_name;
  return (
    <article
      className="grid items-start gap-3 md:grid-cols-[10rem_minmax(0,1fr)]"
      data-release={release.tag_name}
    >
      <aside className="text-muted-foreground flex flex-row flex-wrap gap-x-4 gap-y-1.5 text-xs md:flex-col md:pt-2">
        <span className="text-foreground text-sm">
          <Ago at={release.published_at ?? release.created_at} />
        </span>
        <span className="flex items-center gap-1.5">
          <UserAvatar user={release.author} />
          <span className="text-foreground font-medium">{release.author.login}</span>
        </span>
        <span className="flex items-center gap-1" title="Tag">
          <HugeiconsIcon icon={Tag01Icon} strokeWidth={2} className="size-3.5" />
          <code className="font-mono">{release.tag_name}</code>
        </span>
        <span className="flex items-center gap-1" title="Target">
          <HugeiconsIcon icon={GitBranchIcon} strokeWidth={2} className="size-3.5" />
          <code className="font-mono">{release.target_commitish}</code>
        </span>
      </aside>
      <Panel
        title={
          <span className="flex flex-wrap items-center gap-2">
            {release.draft ? (
              <Link
                to="/$owner/$repo/releases/edit/$tag"
                params={{ ...repo, tag: release.tag_name }}
                className="text-base font-semibold hover:underline"
              >
                {title}
              </Link>
            ) : (
              <Link
                to="/$owner/$repo/releases/tag/$tag"
                params={{ ...repo, tag: release.tag_name }}
                className="text-base font-semibold hover:underline"
              >
                {title}
              </Link>
            )}
            <ReleaseBadges release={release} latest={latest} />
          </span>
        }
        actions={actions}
      >
        <div className="flex flex-col gap-3">
          {release.body ? (
            <MarkdownBody text={release.body} />
          ) : (
            <p className="text-muted-foreground text-sm italic">No release notes.</p>
          )}
          <section className="flex flex-col gap-1.5">
            <h3 className="text-sm font-semibold">
              Assets{" "}
              <span className="text-muted-foreground font-normal tabular-nums">
                {release.assets.length + (release.draft ? 0 : 2)}
              </span>
            </h3>
            {release.assets.length === 0 && release.draft ? (
              <p className="text-muted-foreground text-sm">No assets yet.</p>
            ) : (
              <AssetList {...repo} release={release} />
            )}
          </section>
          {!release.draft && (
            <Region name="reactions" rows={1}>
              <Reactions {...repo} id={release.id} />
            </Region>
          )}
        </div>
      </Panel>
    </article>
  );
}
