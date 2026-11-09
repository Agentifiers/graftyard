import { useMemo } from "react";
import { Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  Line,
  LineChart,
  Scatter,
  ScatterChart,
  XAxis,
  YAxis,
  ZAxis,
} from "recharts";
import { Ago, UserAvatar } from "@/components/github";
import { Panel } from "@/components/panel";
import { Region } from "@/components/region";
import {
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import {
  clonesQuery,
  codeFrequencyQuery,
  commitActivityQuery,
  commitsQuery,
  contributorStatsQuery,
  participationQuery,
  popularPathsQuery,
  punchCardQuery,
  referrersQuery,
  viewsQuery,
} from "./queries";
import { firstLine, short, type Repo } from "./refs";

// The Insights pages: one chart per GHES statistics endpoint, plus traffic from the
// private API. Each panel is its own Region.

const theme = (light: string, dark: string) => ({ theme: { light, dark } });
const BLUE = theme("#0969da", "#58a6ff");
const GREEN = theme("#1a7f37", "#3fb950");
const RED = theme("#cf222e", "#f85149");
const PURPLE = theme("#8250df", "#bc8cff");

const day = (unix: number) =>
  new Date(unix * 1000).toLocaleDateString("en", {
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  });
// A rounded top, so the highest gridline carries a label. Recharts' own "auto" max can
// land between ticks and leave the top line unlabeled.
const NICE: [number, (max: number) => number] = [
  0,
  (max) => {
    // Four steps of 1, 2, or 5 times a power of ten.
    const raw = Math.max(1, max / 4);
    const pow = 10 ** Math.floor(Math.log10(raw));
    const step = [1, 2, 5, 10].map((u) => u * pow).find((u) => u >= raw)!;
    return step * 4;
  },
];
// Recharts paints ticks #666 inline, under 4.5:1 on the dark card. Use the theme's muted
// text color, which passes in both themes.
const TICK = { fill: "var(--muted-foreground)" };
const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

function NoData({ what }: { what: string }) {
  return (
    <Empty className="py-8">
      <EmptyHeader>
        <EmptyTitle>No {what} yet</EmptyTitle>
        <EmptyDescription>GHES computes this once the default branch has commits.</EmptyDescription>
      </EmptyHeader>
    </Empty>
  );
}

// ---- Pulse ----

export function Pulse({ owner, repo }: Repo) {
  return (
    <div className="flex flex-col gap-4">
      <Region name="participation" rows={4}>
        <Participation owner={owner} repo={repo} />
      </Region>
      <Region name="recent commits" rows={6}>
        <RecentCommits owner={owner} repo={repo} />
      </Region>
    </div>
  );
}

function Participation({ owner, repo }: Repo) {
  const { data } = useSuspenseQuery(participationQuery({ owner, repo }));
  const rows = data.all.map((all, i) => ({
    week: `W${i + 1}`,
    others: all - (data.owner[i] ?? 0),
    owner: data.owner[i] ?? 0,
  }));
  const total = data.all.reduce((a, b) => a + b, 0);
  const last4 = data.all.slice(-4).reduce((a, b) => a + b, 0);
  const config = {
    owner: { label: owner, ...PURPLE },
    others: { label: "Everyone else", ...BLUE },
  } satisfies ChartConfig;
  return (
    <Panel title="Participation over the last 52 weeks">
      <div className="mb-3 grid grid-cols-3 gap-2 text-center" data-testid="pulse-stats">
        <Stat label="Commits in 52 weeks" value={total} />
        <Stat label="Commits in 4 weeks" value={last4} />
        <Stat label="Busiest week" value={Math.max(0, ...data.all)} />
      </div>
      {total === 0 ? (
        <NoData what="commits" />
      ) : (
        <ChartContainer config={config} className="aspect-auto h-48 w-full">
          <BarChart data={rows} margin={{ left: -20, top: 8 }}>
            <CartesianGrid vertical={false} />
            <XAxis tick={TICK} dataKey="week" tickLine={false} axisLine={false} interval={7} />
            <YAxis
              tick={TICK}
              allowDecimals={false}
              tickLine={false}
              axisLine={false}
              domain={NICE}
              interval={0}
            />
            <ChartTooltip content={<ChartTooltipContent />} />
            <ChartLegend content={<ChartLegendContent />} />
            <Bar dataKey="owner" stackId="a" fill="var(--color-owner)" />
            <Bar dataKey="others" stackId="a" fill="var(--color-others)" />
          </BarChart>
        </ChartContainer>
      )}
    </Panel>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-sm border px-2 py-2">
      <div className="text-xl font-semibold tabular-nums">{value}</div>
      <div className="text-muted-foreground text-xs">{label}</div>
    </div>
  );
}

function RecentCommits({ owner, repo }: Repo) {
  const { data } = useSuspenseQuery(commitsQuery({ owner, repo, sha: "main", page: 1 }));
  return (
    <Panel title="Latest commits on main" flush>
      <ul className="divide-y text-sm">
        {data.commits.slice(0, 8).map((c) => {
          const author =
            c.author && "login" in c.author
              ? { login: c.author.login, avatar_url: c.author.avatar_url }
              : null;
          return (
            <li key={c.sha} className="flex items-center gap-2 px-3 py-1.5">
              <UserAvatar user={author} />
              <Link
                to="/$owner/$repo/commit/$sha"
                params={{ owner, repo, sha: c.sha }}
                className="min-w-0 flex-1 truncate hover:underline"
              >
                {firstLine(c.commit.message)}
              </Link>
              <span className="text-muted-foreground font-mono text-xs">{short(c.sha)}</span>
              <span className="text-muted-foreground w-24 text-right text-xs">
                <Ago at={c.commit.author?.date ?? ""} />
              </span>
            </li>
          );
        })}
      </ul>
    </Panel>
  );
}

// ---- Contributors ----

export function Contributors({ owner, repo }: Repo) {
  const { data } = useSuspenseQuery(contributorStatsQuery({ owner, repo }));
  const sorted = [...data].sort((a, b) => b.total - a.total);
  const all = useMemo(() => {
    const weeks = new Map<number, number>();
    for (const c of data)
      for (const w of c.weeks) weeks.set(w.w ?? 0, (weeks.get(w.w ?? 0) ?? 0) + (w.c ?? 0));
    return [...weeks].sort((a, b) => a[0] - b[0]).map(([w, c]) => ({ week: day(w), commits: c }));
  }, [data]);
  if (data.length === 0) return <NoData what="contributors" />;
  const config = { commits: { label: "Commits", ...BLUE } } satisfies ChartConfig;
  return (
    <div className="flex flex-col gap-4" data-testid="contributor-graphs">
      <Panel title="Commits over time, everyone">
        <ChartContainer config={config} className="aspect-auto h-48 w-full">
          <AreaChart data={all} margin={{ left: -20, top: 8 }}>
            <CartesianGrid vertical={false} />
            <XAxis tick={TICK} dataKey="week" tickLine={false} axisLine={false} minTickGap={40} />
            <YAxis
              tick={TICK}
              allowDecimals={false}
              tickLine={false}
              axisLine={false}
              domain={NICE}
              interval={0}
            />
            <ChartTooltip content={<ChartTooltipContent />} />
            <Area
              dataKey="commits"
              type="step"
              fill="var(--color-commits)"
              fillOpacity={0.25}
              stroke="var(--color-commits)"
            />
          </AreaChart>
        </ChartContainer>
      </Panel>
      <div className="grid gap-4 md:grid-cols-2">
        {sorted.map((c, rank) => {
          const adds = c.weeks.reduce((n, w) => n + (w.a ?? 0), 0);
          const dels = c.weeks.reduce((n, w) => n + (w.d ?? 0), 0);
          const rows = c.weeks.map((w) => ({ week: day(w.w ?? 0), commits: w.c ?? 0 }));
          return (
            <Panel
              key={c.author?.login}
              title={
                <span className="flex items-center gap-2">
                  <UserAvatar user={c.author} />
                  {c.author?.login}
                  <span className="text-muted-foreground font-normal">#{rank + 1}</span>
                </span>
              }
              actions={
                <span className="text-xs">
                  {c.total} commits <span className="text-success">{adds} ++</span>{" "}
                  <span className="text-destructive">{dels} --</span>
                </span>
              }
            >
              <ChartContainer config={config} className="aspect-auto h-24 w-full">
                <AreaChart data={rows} margin={{ left: -30, top: 8 }}>
                  <XAxis dataKey="week" hide />
                  <YAxis
                    tick={TICK}
                    allowDecimals={false}
                    tickLine={false}
                    axisLine={false}
                    domain={NICE}
                    interval={0}
                  />
                  <ChartTooltip content={<ChartTooltipContent />} />
                  <Area
                    dataKey="commits"
                    type="step"
                    fill="var(--color-commits)"
                    fillOpacity={0.25}
                    stroke="var(--color-commits)"
                  />
                </AreaChart>
              </ChartContainer>
            </Panel>
          );
        })}
      </div>
    </div>
  );
}

// ---- Commit activity ----

export function CommitActivity({ owner, repo }: Repo) {
  const { data } = useSuspenseQuery(commitActivityQuery({ owner, repo }));
  if (data.length === 0) return <NoData what="commit activity" />;
  const rows = data.map((w) => ({ week: day(w.week), commits: w.total }));
  const latest = data.at(-1)!;
  const config = { commits: { label: "Commits", ...GREEN } } satisfies ChartConfig;
  const dayConfig = { commits: { label: "Commits", ...BLUE } } satisfies ChartConfig;
  return (
    <div className="flex flex-col gap-4">
      <Panel title="Commits per week, last year">
        <ChartContainer
          config={config}
          className="aspect-auto h-56 w-full"
          data-testid="commit-activity"
        >
          <BarChart data={rows} margin={{ left: -20, top: 8 }}>
            <CartesianGrid vertical={false} />
            <XAxis tick={TICK} dataKey="week" tickLine={false} axisLine={false} minTickGap={40} />
            <YAxis
              tick={TICK}
              allowDecimals={false}
              tickLine={false}
              axisLine={false}
              domain={NICE}
              interval={0}
            />
            <ChartTooltip content={<ChartTooltipContent />} />
            <Bar dataKey="commits" fill="var(--color-commits)" />
          </BarChart>
        </ChartContainer>
      </Panel>
      <Panel title={`By day, week of ${day(latest.week)}`}>
        <ChartContainer config={dayConfig} className="aspect-auto h-40 w-full">
          <BarChart
            data={latest.days.map((n, i) => ({ day: DAYS[i], commits: n }))}
            margin={{ left: -20, top: 8 }}
          >
            <XAxis tick={TICK} dataKey="day" tickLine={false} axisLine={false} />
            <YAxis
              tick={TICK}
              allowDecimals={false}
              tickLine={false}
              axisLine={false}
              domain={NICE}
              interval={0}
            />
            <ChartTooltip content={<ChartTooltipContent />} />
            <Bar dataKey="commits" fill="var(--color-commits)" />
          </BarChart>
        </ChartContainer>
      </Panel>
    </div>
  );
}

// ---- Code frequency ----

export function CodeFrequency({ owner, repo }: Repo) {
  const { data } = useSuspenseQuery(codeFrequencyQuery({ owner, repo }));
  if (data.length === 0) return <NoData what="code frequency" />;
  const rows = data.map(([w, a, d]) => ({ week: day(w), additions: a, deletions: d }));
  const config = {
    additions: { label: "Additions", ...GREEN },
    deletions: { label: "Deletions", ...RED },
  } satisfies ChartConfig;
  return (
    <Panel title="Additions and deletions per week">
      <ChartContainer
        config={config}
        className="aspect-auto h-64 w-full"
        data-testid="code-frequency"
      >
        <AreaChart data={rows} margin={{ left: -10, top: 8 }} stackOffset="sign">
          <CartesianGrid vertical={false} />
          <XAxis tick={TICK} dataKey="week" tickLine={false} axisLine={false} minTickGap={40} />
          <YAxis tick={TICK} tickLine={false} axisLine={false} />
          <ChartTooltip content={<ChartTooltipContent />} />
          <ChartLegend content={<ChartLegendContent />} />
          <Area
            dataKey="additions"
            type="monotone"
            fill="var(--color-additions)"
            fillOpacity={0.35}
            stroke="var(--color-additions)"
          />
          <Area
            dataKey="deletions"
            type="monotone"
            fill="var(--color-deletions)"
            fillOpacity={0.35}
            stroke="var(--color-deletions)"
          />
        </AreaChart>
      </ChartContainer>
    </Panel>
  );
}

// ---- Punch card ----

export function PunchCard({ owner, repo }: Repo) {
  const { data } = useSuspenseQuery(punchCardQuery({ owner, repo }));
  if (data.length === 0) return <NoData what="punch card" />;
  const rows = data.map(([d, h, n]) => ({ hour: h, day: 6 - d, commits: n }));
  const max = Math.max(1, ...rows.map((r) => r.commits));
  const config = { commits: { label: "Commits", ...PURPLE } } satisfies ChartConfig;
  return (
    <Panel title="Commits by day and hour (UTC)">
      <ChartContainer config={config} className="aspect-auto h-72 w-full" data-testid="punch-card">
        <ScatterChart margin={{ left: 0, right: 10 }}>
          <XAxis
            type="number"
            dataKey="hour"
            domain={[-0.5, 23.5]}
            ticks={[0, 3, 6, 9, 12, 15, 18, 21]}
            tickFormatter={(h) => `${h}:00`}
            tickLine={false}
            tick={TICK}
          />
          <YAxis
            type="number"
            dataKey="day"
            domain={[-0.5, 6.5]}
            ticks={[0, 1, 2, 3, 4, 5, 6]}
            tickFormatter={(d) => DAYS[6 - d]}
            tickLine={false}
            tick={TICK}
            width={40}
          />
          <ZAxis type="number" dataKey="commits" range={[0, 260]} domain={[0, max]} />
          <ChartTooltip
            cursor={false}
            content={
              <ChartTooltipContent
                hideLabel
                formatter={(_v, _n, item) => {
                  const p = item.payload as { hour: number; day: number; commits: number };
                  return `${p.commits} commits on ${DAYS[6 - p.day]} at ${p.hour}:00`;
                }}
              />
            }
          />
          <Scatter data={rows} fill="var(--color-commits)" />
        </ScatterChart>
      </ChartContainer>
    </Panel>
  );
}

// ---- Traffic ----

export function Traffic({ owner, repo, per }: Repo & { per: "day" | "week" }) {
  return (
    <div className="flex flex-col gap-4">
      <div className="grid gap-4 md:grid-cols-2">
        <Region name="views" rows={4}>
          <Views owner={owner} repo={repo} per={per} />
        </Region>
        <Region name="clones" rows={4}>
          <Clones owner={owner} repo={repo} per={per} />
        </Region>
      </div>
      <div className="grid gap-4 md:grid-cols-2">
        <Region name="referrers" rows={4}>
          <Referrers owner={owner} repo={repo} />
        </Region>
        <Region name="popular content" rows={4}>
          <PopularPaths owner={owner} repo={repo} />
        </Region>
      </div>
    </div>
  );
}

const pointLabel = (ts: string) =>
  new Date(ts).toLocaleDateString("en", { month: "short", day: "numeric", timeZone: "UTC" });

function TrafficChart({
  points,
  title,
  count,
  uniques,
}: {
  points: { timestamp: string; count: number; uniques: number }[];
  title: string;
  count: number;
  uniques: number;
}) {
  const config = {
    count: { label: "Total", ...GREEN },
    uniques: { label: "Unique", ...BLUE },
  } satisfies ChartConfig;
  return (
    <Panel
      title={title}
      actions={
        <span className="text-xs">
          <strong>{count}</strong> total · <strong>{uniques}</strong> unique
        </span>
      }
    >
      <ChartContainer config={config} className="aspect-auto h-44 w-full">
        <LineChart
          data={points.map((p) => ({ ...p, label: pointLabel(p.timestamp) }))}
          margin={{ left: -20, top: 8 }}
        >
          <CartesianGrid vertical={false} />
          <XAxis tick={TICK} dataKey="label" tickLine={false} axisLine={false} minTickGap={24} />
          <YAxis
            tick={TICK}
            allowDecimals={false}
            tickLine={false}
            axisLine={false}
            domain={NICE}
            interval={0}
          />
          <ChartTooltip content={<ChartTooltipContent />} />
          <ChartLegend content={<ChartLegendContent />} />
          <Line dataKey="count" stroke="var(--color-count)" strokeWidth={2} dot={false} />
          <Line dataKey="uniques" stroke="var(--color-uniques)" strokeWidth={2} dot={false} />
        </LineChart>
      </ChartContainer>
    </Panel>
  );
}

function Views({ owner, repo, per }: Repo & { per: "day" | "week" }) {
  const { data } = useSuspenseQuery(viewsQuery({ owner, repo, per }));
  return (
    <TrafficChart title="Views" points={data.views} count={data.count} uniques={data.uniques} />
  );
}

function Clones({ owner, repo, per }: Repo & { per: "day" | "week" }) {
  const { data } = useSuspenseQuery(clonesQuery({ owner, repo, per }));
  return (
    <TrafficChart
      title="Git clones"
      points={data.clones}
      count={data.count}
      uniques={data.uniques}
    />
  );
}

function Referrers({ owner, repo }: Repo) {
  const { data } = useSuspenseQuery(referrersQuery({ owner, repo }));
  return (
    <Panel title="Referring sites" flush>
      <table className="w-full text-sm" data-testid="referrers">
        <thead className="text-muted-foreground border-b text-xs">
          <tr>
            <th className="px-3 py-1.5 text-left font-medium">Site</th>
            <th className="px-3 py-1.5 text-right font-medium">Views</th>
            <th className="px-3 py-1.5 text-right font-medium">Unique</th>
          </tr>
        </thead>
        <tbody className="divide-y">
          {data.map((r) => (
            <tr key={r.referrer}>
              <td className="px-3 py-1.5">{r.referrer}</td>
              <td className="px-3 py-1.5 text-right tabular-nums">{r.count}</td>
              <td className="px-3 py-1.5 text-right tabular-nums">{r.uniques}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </Panel>
  );
}

function PopularPaths({ owner, repo }: Repo) {
  const { data } = useSuspenseQuery(popularPathsQuery({ owner, repo }));
  return (
    <Panel title="Popular content" flush>
      <table className="w-full table-fixed text-sm" data-testid="popular-paths">
        <thead className="text-muted-foreground border-b text-xs">
          <tr>
            <th className="px-3 py-1.5 text-left font-medium">Content</th>
            <th className="w-16 px-3 py-1.5 text-right font-medium">Views</th>
            <th className="w-16 px-3 py-1.5 text-right font-medium">Unique</th>
          </tr>
        </thead>
        <tbody className="divide-y">
          {data.map((p) => (
            <tr key={p.path}>
              <td className="truncate px-3 py-1.5" title={p.path}>
                {p.title}
              </td>
              <td className="px-3 py-1.5 text-right tabular-nums">{p.count}</td>
              <td className="px-3 py-1.5 text-right tabular-nums">{p.uniques}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </Panel>
  );
}
