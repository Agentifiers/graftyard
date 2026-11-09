import { useLayoutEffect, useRef, useState } from "react";
import { Link } from "@tanstack/react-router";
import type { Schemas } from "@/api/client";
import { Badge } from "@/components/reui/badge";
import type { GraphJob } from "./ui-api";
import { Duration, StatusIcon } from "./status";

type Ref = { owner: string; repo: string; runId: number };

// Columns by depth: a job sits one column right of the deepest job it needs.
export function columnsOf(jobs: GraphJob[]): GraphJob[][] {
  const depth = new Map<string, number>();
  const visit = (j: GraphJob, seen: Set<string>): number => {
    if (depth.has(j.name)) return depth.get(j.name)!;
    if (seen.has(j.name)) return 0;
    seen.add(j.name);
    const d = Math.max(
      0,
      ...j.needs.map((n) => {
        const dep = jobs.find((x) => x.name === n);
        return dep ? visit(dep, seen) + 1 : 0;
      }),
    );
    depth.set(j.name, d);
    return d;
  };
  for (const j of jobs) visit(j, new Set());
  const cols: GraphJob[][] = [];
  for (const j of jobs) (cols[depth.get(j.name)!] ??= []).push(j);
  return cols;
}

type Edge = { from: string; to: string; d: string };

// Each job is a card linking to its log; `needs` edges are drawn behind the cards.
export function JobGraph({
  graph,
  jobs,
  ...ref
}: Ref & { graph: GraphJob[]; jobs: Schemas["job"][] }) {
  const box = useRef<HTMLDivElement>(null);
  const [edges, setEdges] = useState<Edge[]>([]);
  const cols = columnsOf(graph);

  useLayoutEffect(() => {
    const root = box.current;
    if (!root) return;
    const measure = () => {
      const origin = root.getBoundingClientRect();
      const at = (name: string) =>
        root
          .querySelector<HTMLElement>(`[data-graph-job="${CSS.escape(name)}"]`)
          ?.getBoundingClientRect();
      const next: Edge[] = [];
      for (const j of graph)
        for (const n of j.needs) {
          const a = at(n);
          const b = at(j.name);
          if (!a || !b) continue;
          const x1 = a.right - origin.left;
          const y1 = a.top + a.height / 2 - origin.top;
          const x2 = b.left - origin.left;
          const y2 = b.top + b.height / 2 - origin.top;
          const mid = (x1 + x2) / 2;
          next.push({
            from: n,
            to: j.name,
            d: `M${x1},${y1} C${mid},${y1} ${mid},${y2} ${x2},${y2}`,
          });
        }
      setEdges(next);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    return () => observer.disconnect();
  }, [graph]);

  return (
    <div className="overflow-x-auto p-4" data-testid="job-graph">
      <div ref={box} className="relative flex w-max items-center gap-14">
        <svg
          className="pointer-events-none absolute inset-0 size-full overflow-visible"
          aria-hidden
        >
          {edges.map((e) => (
            <path
              key={`${e.from}->${e.to}`}
              d={e.d}
              data-edge={`${e.from}->${e.to}`}
              className="stroke-border fill-none"
              strokeWidth={2}
            />
          ))}
        </svg>
        {cols.map((col, i) => (
          <div key={i} className="relative flex flex-col gap-3">
            {col.map((g) => {
              const job = jobs.find((j) => j.id === g.job_id);
              return (
                <Link
                  key={g.name}
                  to="/$owner/$repo/actions/runs/$runId/job/$jobId"
                  params={{ ...ref, jobId: g.job_id }}
                  data-graph-job={g.name}
                  className="bg-card hover:border-primary/60 flex w-56 items-center gap-2 rounded-sm border px-3 py-2 text-sm shadow-xs"
                >
                  {job && <StatusIcon item={job} />}
                  <span className="min-w-0 flex-1 truncate font-medium">{g.name}</span>
                  {g.environment && (
                    <Badge variant="outline" size="sm">
                      {g.environment}
                    </Badge>
                  )}
                  <span className="text-muted-foreground text-xs">
                    {job && job.status !== "queued" && job.status !== "waiting" && (
                      <Duration
                        start={job.started_at}
                        end={job.completed_at}
                        running={job.status === "in_progress"}
                      />
                    )}
                  </span>
                </Link>
              );
            })}
          </div>
        ))}
      </div>
    </div>
  );
}
