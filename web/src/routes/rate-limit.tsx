import { useEffect, useState } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import type { Schemas } from "@/api/client";
import { metaQuery, rateLimitQuery } from "@/areas/activity/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { ProgressIndicator, ProgressTrack } from "@/components/ui/progress";
import { Progress as ProgressPrimitive } from "@base-ui/react/progress";
import { cn } from "cn";

// The viewer's GET /rate_limit buckets with reset countdowns, and GET /meta.

export const Route = createFileRoute("/rate-limit")({
  loader: ({ context }) => {
    void context.queryClient.prefetchQuery(rateLimitQuery());
    void context.queryClient.prefetchQuery(metaQuery());
  },
  component: RateLimitPage,
});

function RateLimitPage() {
  return (
    <main className="mx-auto max-w-4xl p-4 sm:p-6">
      <PageHeader
        title="API status"
        description="Your rate limits on this GHES instance. Each bucket refills when its window resets."
      />
      <div className="grid items-start gap-4 md:grid-cols-[minmax(0,1fr)_16rem]">
        <Panel title="Rate limits" flush>
          <Region name="rate limit" rows={6}>
            <Buckets />
          </Region>
        </Panel>
        <Panel title="Instance">
          <Region name="meta" rows={2}>
            <Meta />
          </Region>
        </Panel>
      </div>
    </main>
  );
}

const NAMES: Record<string, string> = {
  core: "REST API",
  graphql: "GraphQL",
  search: "Search",
  code_search: "Code search",
  integration_manifest: "App manifests",
  source_import: "Source imports",
};

// Seconds since the epoch, ticking once a second, so countdowns move without refetching.
function useNow() {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const t = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000);
    return () => clearInterval(t);
  }, []);
  return now;
}

function countdown(seconds: number) {
  if (seconds <= 0) return "resetting";
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return m ? `${m}m ${String(s).padStart(2, "0")}s` : `${s}s`;
}

function Buckets() {
  const { data } = useSuspenseQuery(rateLimitQuery());
  const now = useNow();
  const entries = Object.entries(data.resources).filter((e): e is [string, Schemas["rate-limit"]] =>
    Boolean(e[1]),
  );
  return (
    <ul className="divide-y" data-testid="rate-buckets">
      {entries.map(([name, b]) => {
        const left = b.limit ? b.remaining / b.limit : 0;
        const tone = left < 0.1 ? "bg-destructive" : left < 0.25 ? "bg-warning" : "bg-primary";
        return (
          <li key={name} className="px-4 py-3" data-bucket={name} data-remaining={b.remaining}>
            <div className="flex items-center gap-2 text-sm">
              <span className="font-medium">{NAMES[name] ?? name}</span>
              <code className="text-muted-foreground font-mono text-xs">{name}</code>
              <span className="text-muted-foreground ml-auto tabular-nums" data-testid="remaining">
                {b.remaining.toLocaleString()} of {b.limit.toLocaleString()} left
              </span>
            </div>
            <ProgressPrimitive.Root
              value={b.limit ? (b.used / b.limit) * 100 : 0}
              aria-label={`${NAMES[name] ?? name} used`}
              className="mt-1.5"
            >
              <ProgressTrack className="h-1.5">
                <ProgressIndicator className={tone} />
              </ProgressTrack>
            </ProgressPrimitive.Root>
            <div className="mt-1.5 flex items-center justify-between text-xs">
              <span className="text-muted-foreground">{b.used.toLocaleString()} used</span>
              <span
                className={cn(
                  "tabular-nums",
                  b.reset - now < 60 ? "text-foreground" : "text-muted-foreground",
                )}
                data-testid="reset"
              >
                Resets in {countdown(b.reset - now)}
              </span>
            </div>
          </li>
        );
      })}
    </ul>
  );
}

function Meta() {
  const { data } = useSuspenseQuery(metaQuery());
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-2 text-sm" data-testid="meta">
      <dt className="text-muted-foreground">Version</dt>
      <dd>
        <Badge variant="primary-light" data-testid="installed-version">
          GHES {data.installed_version ?? "unknown"}
        </Badge>
      </dd>
      <dt className="text-muted-foreground">Password auth</dt>
      <dd>{data.verifiable_password_authentication ? "Allowed" : "Off"}</dd>
      {data.domains?.website?.length ? (
        <>
          <dt className="text-muted-foreground">Website</dt>
          <dd className="font-mono text-xs break-all">{data.domains.website[0]}</dd>
        </>
      ) : null}
    </dl>
  );
}
