import { Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { Tag01Icon } from "@hugeicons/core-free-icons";
import type { SlotEntry, SlotProps } from "../slots";
import { Ago } from "@/components/github";
import { Panel } from "@/components/panel";
import { Badge } from "@/components/reui/badge";
import { DeploymentBadge } from "./parts";
import {
  allReleasesQuery,
  deploymentHistoryQuery,
  environmentsQuery,
  latestReleaseQuery,
} from "./queries";

type Repo = SlotProps["repo-sidebar"];

function ReleasesWidget(repo: Repo) {
  const { data: releases } = useSuspenseQuery(allReleasesQuery(repo));
  const { data: latest } = useSuspenseQuery(latestReleaseQuery(repo));
  const published = releases.filter((r) => !r.draft).length;
  return (
    <Panel
      title={
        <Link to="/$owner/$repo/releases" params={repo} className="hover:underline">
          Releases{" "}
          <span
            className="text-muted-foreground font-normal tabular-nums"
            data-testid="sidebar-release-count"
          >
            {published}
          </span>
        </Link>
      }
    >
      {latest ? (
        <Link
          to="/$owner/$repo/releases/tag/$tag"
          params={{ ...repo, tag: latest.tag_name }}
          className="flex items-start gap-2 text-sm"
          data-testid="sidebar-latest-release"
        >
          <HugeiconsIcon icon={Tag01Icon} strokeWidth={2} className="text-success mt-0.5 size-4" />
          <span className="min-w-0">
            <span className="flex flex-wrap items-center gap-1.5">
              <span className="font-semibold hover:underline">
                {latest.name || latest.tag_name}
              </span>
              <Badge variant="success-light" size="sm">
                Latest
              </Badge>
            </span>
            <span className="text-muted-foreground text-xs">
              <Ago at={latest.published_at ?? latest.created_at} />
            </span>
          </span>
        </Link>
      ) : (
        <p className="text-muted-foreground text-sm">
          No releases published.{" "}
          <Link
            to="/$owner/$repo/releases/new"
            params={repo}
            className="text-primary hover:underline"
          >
            Create a new release
          </Link>
        </p>
      )}
    </Panel>
  );
}

function EnvironmentsWidget(repo: Repo) {
  const { data: environments } = useSuspenseQuery(environmentsQuery(repo));
  const { data: history } = useSuspenseQuery(deploymentHistoryQuery(repo));
  if (environments.length === 0) return null;
  return (
    <Panel
      title={
        <Link to="/$owner/$repo/deployments" params={repo} className="hover:underline">
          Environments{" "}
          <span className="text-muted-foreground font-normal tabular-nums">
            {environments.length}
          </span>
        </Link>
      }
    >
      <ul className="flex flex-col gap-1.5 text-sm" data-testid="sidebar-environments">
        {environments.map((e) => {
          const last = history.find((row) => row.deployment.environment === e.name);
          return (
            <li key={e.id} className="flex items-center gap-2" data-environment={e.name}>
              <Link
                to="/$owner/$repo/deployments"
                params={repo}
                search={{ environment: e.name }}
                className="min-w-0 flex-1 truncate font-medium hover:underline"
              >
                {e.name}
              </Link>
              {last ? (
                <DeploymentBadge state={last.statuses[0]?.state} />
              ) : (
                <span className="text-muted-foreground text-xs">Never deployed</span>
              )}
            </li>
          );
        })}
      </ul>
    </Panel>
  );
}

// Typed entries rather than slot(): src/areas/slots.tsx imports this module eagerly, so a
// runtime import of its `slot` helper runs before that helper is defined.
export const slots: SlotEntry[] = [
  { slot: "repo-sidebar", region: "releases", order: 10, Component: ReleasesWidget },
  { slot: "repo-sidebar", region: "environments", order: 11, Component: EnvironmentsWidget },
];
