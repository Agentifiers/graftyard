import { createFileRoute, Outlet } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { CheckmarkBadge01Icon, Link01Icon, Location01Icon } from "@hugeicons/core-free-icons";
import { orgQuery } from "@/areas/core/queries";
import { Region } from "@/components/region";
import { TabNav } from "@/components/tab-nav";
import { Badge } from "@/components/reui/badge";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Skeleton } from "@/components/ui/skeleton";

// An organization's header and tabs. Each area adds tabs through "org" nav.
export const Route = createFileRoute("/orgs/$org")({
  loader: ({ context, params }) => void context.queryClient.prefetchQuery(orgQuery(params.org)),
  component: OrgLayout,
});

function OrgLayout() {
  const params = Route.useParams();
  return (
    <>
      <div className="bg-card border-b">
        <div className="mx-auto max-w-6xl px-4 pt-4">
          <Region name="organization" fallback={<Skeleton className="mb-3 h-14 w-2/3" />}>
            <OrgHeader />
          </Region>
          <TabNav slot="org" params={params} label="Organization" />
        </div>
      </div>
      <main className="mx-auto max-w-6xl p-4 sm:p-6">
        <Outlet />
      </main>
    </>
  );
}

function OrgHeader() {
  const { data: org } = useSuspenseQuery(orgQuery(Route.useParams().org));
  return (
    <div className="flex items-start gap-3 pb-3">
      <Avatar size="lg">
        <AvatarImage src={org.avatar_url} alt={org.login} />
        <AvatarFallback>{org.login.slice(0, 2).toUpperCase()}</AvatarFallback>
      </Avatar>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-lg font-semibold">{org.name ?? org.login}</h1>
          {org.is_verified && (
            <Badge variant="info-light" size="sm">
              <HugeiconsIcon icon={CheckmarkBadge01Icon} strokeWidth={2} />
              Verified
            </Badge>
          )}
        </div>
        {org.description && <p className="text-muted-foreground text-sm">{org.description}</p>}
        <div className="text-muted-foreground mt-1 flex flex-wrap gap-4 text-xs">
          {org.location && (
            <span className="flex items-center gap-1">
              <HugeiconsIcon icon={Location01Icon} strokeWidth={2} className="size-3.5" />
              {org.location}
            </span>
          )}
          {org.blog && (
            <span className="flex items-center gap-1">
              <HugeiconsIcon icon={Link01Icon} strokeWidth={2} className="size-3.5" />
              {org.blog}
            </span>
          )}
        </div>
      </div>
    </div>
  );
}
