import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { blameQuery, resolveQuery } from "@/areas/code/queries";
import {
  Breadcrumbs,
  RefKind,
  RefPicker,
  firstLine,
  joinPath,
  short,
  useSwitchRef,
} from "@/areas/code/refs";
import { Ago, UserAvatar } from "@/components/github";
import { Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";

// Blame: each run of lines with the commit that last changed it, and a heat bar from
// the oldest change (cool) to the newest (hot).

export const Route = createFileRoute("/$owner/$repo/blame/$")({
  loader: async ({ context, params }) => {
    const { owner, repo, _splat = "" } = params;
    const hit = await context.queryClient
      .ensureQueryData(resolveQuery({ owner, repo, splat: _splat }))
      .catch(() => undefined);
    if (hit?.type === "file")
      void context.queryClient.prefetchQuery(
        blameQuery({ owner, repo, ref: hit.ref, path: hit.path }),
      );
  },
  component: BlamePage,
});

const HEAT = [
  "#ffdf5d",
  "#ffc54d",
  "#fb9a46",
  "#f57d3d",
  "#ea5d31",
  "#d9412b",
  "#bd2c2b",
  "#9a2232",
  "#7a1c37",
  "#59163a",
].reverse();

function BlamePage() {
  const { owner, repo, _splat = "" } = Route.useParams();
  return (
    <Region name="blame" fallback={<Skeleton className="h-64" />}>
      <BlameBody owner={owner} repo={repo} splat={_splat} />
    </Region>
  );
}

function BlameBody({ owner, repo, splat }: { owner: string; repo: string; splat: string }) {
  const { data: at } = useSuspenseQuery(resolveQuery({ owner, repo, splat }));
  const switchRef = useSwitchRef("blame", { owner, repo }, at.path);
  if (at.type !== "file") throw new Error(`${at.path || repo} is not a file`);
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <RefPicker owner={owner} repo={repo} current={at.ref} onPick={switchRef} />
        <RefKind kind={at.kind} />
        <Breadcrumbs owner={owner} repo={repo} ref={at.ref} path={at.path} />
        <Button
          size="sm"
          variant="outline"
          className="ml-auto"
          render={<Link to="/$owner/$repo/blob/$" params={{ owner, repo, _splat: splat }} />}
        >
          View file
        </Button>
      </div>
      <Region name="blame lines" rows={12}>
        <BlameTable owner={owner} repo={repo} refName={at.ref} path={at.path} />
      </Region>
    </div>
  );
}

function BlameTable({
  owner,
  repo,
  refName,
  path,
}: {
  owner: string;
  repo: string;
  refName: string;
  path: string;
}) {
  const { data } = useSuspenseQuery(blameQuery({ owner, repo, ref: refName, path }));
  const oldest = Date.parse(data.oldest);
  const span = Math.max(1, Date.parse(data.newest) - oldest);
  const heat = (date: string) =>
    HEAT[Math.min(HEAT.length - 1, Math.floor(((Date.parse(date) - oldest) / span) * HEAT.length))];
  const commits = new Set(data.ranges.map((r) => r.commit.sha)).size;
  return (
    <Panel
      flush
      title={
        <span className="text-xs font-normal">
          <strong>{data.lines.length}</strong> lines from <strong>{commits}</strong> commits
        </span>
      }
      actions={
        <span
          className="text-muted-foreground flex items-center gap-1 text-xs"
          aria-label="Age legend"
        >
          Older
          {HEAT.map((c) => (
            <span key={c} className="h-2.5 w-2" style={{ backgroundColor: c }} />
          ))}
          Newer
        </span>
      }
    >
      <div className="overflow-x-auto">
        <table className="w-full border-collapse font-mono text-xs leading-5" data-testid="blame">
          <tbody>
            {data.ranges.map((range) => (
              <tr
                key={range.start}
                className="border-t align-top first:border-t-0"
                data-blame-sha={range.commit.sha}
              >
                <td
                  className="bg-muted/30 w-80 max-w-80 border-r border-l-4 px-2 py-1 font-sans"
                  style={{ borderLeftColor: heat(range.commit.date) }}
                >
                  <div className="flex items-center gap-1.5">
                    <UserAvatar
                      user={
                        range.commit.author.login
                          ? {
                              login: range.commit.author.login,
                              avatar_url: range.commit.author.avatar_url ?? "",
                            }
                          : null
                      }
                    />
                    <Link
                      to="/$owner/$repo/commit/$sha"
                      params={{ owner, repo, sha: range.commit.sha }}
                      className="min-w-0 flex-1 truncate hover:underline"
                      title={range.commit.message}
                    >
                      {firstLine(range.commit.message)}
                    </Link>
                  </div>
                  <div className="text-muted-foreground mt-0.5 flex items-center gap-2 pl-7 text-[11px]">
                    <span>{range.commit.author.login ?? range.commit.author.name}</span>
                    <Ago at={range.commit.date} />
                    <Link
                      to="/$owner/$repo/blame/$"
                      params={{ owner, repo, _splat: joinPath(range.commit.sha, path) }}
                      className="ml-auto font-mono hover:underline"
                      title="Blame at this commit"
                    >
                      {short(range.commit.sha)}
                    </Link>
                  </div>
                </td>
                <td className="p-0">
                  {data.lines.slice(range.start - 1, range.end).map((line, i) => {
                    const n = range.start + i;
                    return (
                      <div key={n} id={`L${n}`} className="hover:bg-muted/40 flex">
                        <Link
                          to="/$owner/$repo/blob/$"
                          params={{ owner, repo, _splat: joinPath(refName, path) }}
                          hash={`L${n}`}
                          className="text-muted-foreground hover:text-foreground w-12 shrink-0 pr-3 text-right select-none"
                        >
                          {n}
                        </Link>
                        <span className="pr-3 whitespace-pre">{line || " "}</span>
                      </div>
                    );
                  })}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Panel>
  );
}
