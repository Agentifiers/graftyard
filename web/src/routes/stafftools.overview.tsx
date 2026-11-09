import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { Bar, BarChart, CartesianGrid, Cell, Pie, PieChart, XAxis, YAxis } from "recharts";
import { HugeiconsIcon } from "@hugeicons/react";
import { Alert02Icon, Certificate01Icon } from "@hugeicons/core-free-icons";
import { licenseQuery, metaQuery, statsQuery } from "@/areas/admin/queries";
import { when } from "@/areas/admin/ui";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import {
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import { Progress } from "@/components/ui/progress";
import { Skeleton } from "@/components/ui/skeleton";

export const Route = createFileRoute("/stafftools/overview")({
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(metaQuery());
    void queryClient.prefetchQuery(statsQuery());
    void queryClient.prefetchQuery(licenseQuery());
  },
  component: Overview,
});

function Overview() {
  return (
    <>
      <PageHeader
        title="Overview"
        description={
          <Region name="instance version" fallback={<Skeleton className="h-4 w-48" />}>
            <Version />
          </Region>
        }
      />
      <div className="flex flex-col gap-4">
        <Region name="license" rows={2}>
          <License />
        </Region>
        <Region name="instance stats" rows={6}>
          <Stats />
        </Region>
      </div>
    </>
  );
}

function Version() {
  const { data } = useSuspenseQuery(metaQuery());
  return (
    <span data-testid="installed-version">
      GitHub Enterprise Server {data.installed_version ?? "unknown"}, served by Graftyard on{" "}
      <code className="text-xs">ghes.graftyard.test</code>
    </span>
  );
}

function License() {
  const { data } = useSuspenseQuery(licenseQuery());
  const seats = Number(data.seats) || 0;
  const used = data.seats_used ?? 0;
  const days = data.days_until_expiration ?? 0;
  const pct = seats ? Math.round((used / seats) * 100) : 0;
  return (
    <Panel
      title={
        <span className="flex items-center gap-2">
          <HugeiconsIcon icon={Certificate01Icon} strokeWidth={2} className="size-4" />
          License
        </span>
      }
      actions={
        <Badge variant="outline" size="sm" className="capitalize">
          {data.kind}
        </Badge>
      }
    >
      <div className="grid gap-4 sm:grid-cols-3">
        <div className="sm:col-span-2">
          <div className="flex items-baseline justify-between text-sm">
            <span>
              <span className="text-2xl font-semibold tabular-nums" data-testid="seats-used">
                {used}
              </span>{" "}
              of {seats} seats used
            </span>
            <span className="text-muted-foreground tabular-nums" data-testid="seats-available">
              {data.seats_available} available
            </span>
          </div>
          <Progress value={pct} className="mt-2" />
          <p className="text-muted-foreground mt-1 text-xs">Suspended users do not hold a seat.</p>
        </div>
        <div className="text-sm">
          <div className="text-muted-foreground text-xs">Expires</div>
          <div className="font-medium" data-testid="license-expiry">
            {when(data.expire_at)}
          </div>
          <div className="text-muted-foreground text-xs">in {days} days</div>
        </div>
      </div>
      {days <= 30 && (
        <Alert variant="warning" className="mt-3">
          <HugeiconsIcon icon={Alert02Icon} strokeWidth={2} />
          <AlertTitle>The license expires in {days} days</AlertTitle>
          <AlertDescription>Upload a renewed license in the management console.</AlertDescription>
        </Alert>
      )}
    </Panel>
  );
}

const pieConfig = {
  active: { label: "Active", color: "var(--success)" },
  admins: { label: "Site admins", color: "var(--info)" },
  suspended: { label: "Suspended", color: "var(--destructive)" },
} satisfies ChartConfig;

const barConfig = {
  open: { label: "Open", color: "var(--warning)" },
  closed: { label: "Closed or merged", color: "var(--muted-foreground)" },
} satisfies ChartConfig;

function Stats() {
  const { data } = useSuspenseQuery(statsQuery());
  const cards: { label: string; value: number; detail: string; to?: string }[] = [
    {
      label: "Repositories",
      value: data.repos?.total_repos ?? 0,
      detail: `${data.repos?.fork_repos ?? 0} forks, ${data.repos?.org_repos ?? 0} in orgs`,
      to: "/stafftools/repositories",
    },
    {
      label: "Users",
      value: data.users?.total_users ?? 0,
      detail: `${data.users?.admin_users ?? 0} admins, ${data.users?.suspended_users ?? 0} suspended`,
      to: "/stafftools/users",
    },
    {
      label: "Organizations",
      value: data.orgs?.total_orgs ?? 0,
      detail: `${data.orgs?.total_teams ?? 0} teams`,
      to: "/stafftools/organizations",
    },
    {
      label: "Issues",
      value: data.issues?.total_issues ?? 0,
      detail: `${data.issues?.open_issues ?? 0} open`,
    },
    {
      label: "Pull requests",
      value: data.pulls?.total_pulls ?? 0,
      detail: `${data.pulls?.merged_pulls ?? 0} merged`,
    },
    {
      label: "Comments",
      value:
        (data.comments?.total_issue_comments ?? 0) +
        (data.comments?.total_pull_request_comments ?? 0) +
        (data.comments?.total_commit_comments ?? 0) +
        (data.comments?.total_gist_comments ?? 0),
      detail: `${data.comments?.total_issue_comments ?? 0} on issues`,
    },
    {
      label: "Global hooks",
      value: data.hooks?.total_hooks ?? 0,
      detail: `${data.hooks?.active_hooks ?? 0} active`,
      to: "/stafftools/hooks",
    },
    { label: "Pages sites", value: data.pages?.total_pages ?? 0, detail: "GitHub Pages" },
    {
      label: "Gists",
      value: data.gists?.total_gists ?? 0,
      detail: `${data.gists?.private_gists ?? 0} secret`,
    },
  ];
  const users = data.users!;
  const people = [
    { key: "active", value: users.total_users - users.admin_users - users.suspended_users },
    { key: "admins", value: users.admin_users },
    { key: "suspended", value: users.suspended_users },
  ];
  const work = [
    {
      kind: "Issues",
      open: data.issues?.open_issues ?? 0,
      closed: data.issues?.closed_issues ?? 0,
    },
    {
      kind: "Pull requests",
      open: (data.pulls?.total_pulls ?? 0) - (data.pulls?.merged_pulls ?? 0),
      closed: data.pulls?.merged_pulls ?? 0,
    },
  ];
  return (
    <div className="flex flex-col gap-4">
      <div
        className="grid grid-cols-2 gap-px overflow-hidden rounded-sm border bg-border sm:grid-cols-3"
        data-testid="stat-cards"
      >
        {cards.map((c) => {
          const body = (
            <>
              <div className="text-muted-foreground text-xs">{c.label}</div>
              <div className="text-2xl font-semibold tabular-nums" data-stat={c.label}>
                {c.value.toLocaleString()}
              </div>
              <div className="text-muted-foreground text-xs">{c.detail}</div>
            </>
          );
          return c.to ? (
            <Link key={c.label} to={c.to} className="bg-card hover:bg-muted/50 p-3">
              {body}
            </Link>
          ) : (
            <div key={c.label} className="bg-card p-3">
              {body}
            </div>
          );
        })}
      </div>
      <div className="grid gap-4 md:grid-cols-2">
        <Panel title="Accounts">
          <ChartContainer config={pieConfig} className="aspect-auto h-48">
            <PieChart>
              <ChartTooltip content={<ChartTooltipContent nameKey="key" hideLabel />} />
              <Pie data={people} dataKey="value" nameKey="key" innerRadius={45} strokeWidth={2}>
                {people.map((p) => (
                  <Cell key={p.key} fill={`var(--color-${p.key})`} />
                ))}
              </Pie>
            </PieChart>
          </ChartContainer>
          <ul className="flex justify-center gap-4 text-xs">
            {people.map((p) => (
              <li key={p.key} className="flex items-center gap-1.5">
                <span
                  className="size-2.5 rounded-sm"
                  style={{ background: pieConfig[p.key as keyof typeof pieConfig].color }}
                />
                {pieConfig[p.key as keyof typeof pieConfig].label} {p.value}
              </li>
            ))}
          </ul>
        </Panel>
        <Panel title="Issues and pull requests">
          <ChartContainer config={barConfig} className="aspect-auto h-56">
            <BarChart data={work} margin={{ left: -20 }}>
              <CartesianGrid vertical={false} />
              <XAxis dataKey="kind" tickLine={false} axisLine={false} />
              <YAxis allowDecimals={false} tickLine={false} axisLine={false} />
              <ChartTooltip content={<ChartTooltipContent />} />
              <Bar dataKey="open" stackId="a" fill="var(--color-open)" radius={0} />
              <Bar dataKey="closed" stackId="a" fill="var(--color-closed)" radius={0} />
              <ChartLegend content={<ChartLegendContent />} />
            </BarChart>
          </ChartContainer>
        </Panel>
      </div>
    </div>
  );
}
