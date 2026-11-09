import { useMemo } from "react";
import { useSuspenseQuery } from "@tanstack/react-query";
import { contributionsQuery } from "./queries";

// The contribution graph: 53 week columns of 7 day squares, Sunday on top, the newest
// week on the right. Square cells, five shades by quartile of the user's busiest day.

const WEEKS = 53;
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const SHADES = [
  "bg-muted",
  "bg-emerald-200 dark:bg-emerald-800",
  "bg-emerald-400 dark:bg-emerald-600",
  "bg-emerald-600 dark:bg-emerald-400",
  "bg-emerald-800 dark:bg-emerald-200",
];

type Day = { date: string; count: number };

// Lays the days out on the grid: the last column ends on today, so it may be short.
export function grid(days: Day[]) {
  const byDate = new Map(days.map((d) => [d.date, d.count]));
  const today = days.length ? new Date(`${days.at(-1)!.date}T00:00:00Z`) : new Date();
  const end = today.getUTCDay();
  const start = new Date(today.getTime() - ((WEEKS - 1) * 7 + end) * 86_400_000);
  const weeks: (Day | null)[][] = [];
  for (let w = 0; w < WEEKS; w++) {
    const col: (Day | null)[] = [];
    for (let d = 0; d < 7; d++) {
      const at = new Date(start.getTime() + (w * 7 + d) * 86_400_000);
      if (at > today) col.push(null);
      else {
        const date = at.toISOString().slice(0, 10);
        col.push({ date, count: byDate.get(date) ?? 0 });
      }
    }
    weeks.push(col);
  }
  return weeks;
}

function describe(d: Day) {
  const day = new Date(`${d.date}T00:00:00Z`).toLocaleDateString(undefined, {
    month: "long",
    day: "numeric",
    timeZone: "UTC",
  });
  return `${d.count === 0 ? "No" : d.count} contribution${d.count === 1 ? "" : "s"} on ${day}`;
}

function level(count: number, max: number) {
  if (count === 0 || max === 0) return 0;
  return Math.min(4, Math.ceil((count / max) * 4));
}

export function ContributionGraph({ login }: { login: string }) {
  const { data } = useSuspenseQuery(contributionsQuery(login));
  const weeks = useMemo(() => grid(data), [data]);
  const cells = weeks.flat().filter((d): d is Day => d !== null);
  const total = cells.reduce((n, d) => n + d.count, 0);
  const max = Math.max(0, ...cells.map((d) => d.count));
  const labels = weeks.map((w, i) => {
    const first = w.find((d) => d !== null);
    if (!first) return "";
    const month = new Date(`${first.date}T00:00:00Z`).getUTCMonth();
    const prev = weeks[i - 1]?.find((d) => d !== null);
    const prevMonth = prev ? new Date(`${prev.date}T00:00:00Z`).getUTCMonth() : -1;
    return month !== prevMonth && i < WEEKS - 2 ? MONTHS[month] : "";
  });
  return (
    <div className="flex flex-col gap-2" data-testid="contributions" data-total={total}>
      <h2 className="text-sm">
        <strong>{total.toLocaleString()}</strong> contributions in the last year
      </h2>
      <div className="overflow-x-auto rounded-sm border p-3">
        <div className="inline-flex flex-col gap-1">
          <div className="text-muted-foreground ml-7 flex gap-[3px] text-[10px]" aria-hidden>
            {labels.map((l, i) => (
              <span key={i} className="w-[10px] overflow-visible whitespace-nowrap">
                {l}
              </span>
            ))}
          </div>
          <div className="flex gap-1">
            <div
              className="text-muted-foreground flex w-6 flex-col gap-[3px] text-[10px] leading-[10px]"
              aria-hidden
            >
              {["", "Mon", "", "Wed", "", "Fri", ""].map((d, i) => (
                <span key={i} className="h-[10px]">
                  {d}
                </span>
              ))}
            </div>
            <div className="flex gap-[3px]" role="grid" aria-label="Contributions by day">
              {weeks.map((w, i) => (
                <div key={i} className="flex flex-col gap-[3px]" role="row">
                  {w.map((d, j) =>
                    d ? (
                      <span
                        key={j}
                        role="gridcell"
                        data-date={d.date}
                        data-count={d.count}
                        data-level={level(d.count, max)}
                        title={describe(d)}
                        aria-label={describe(d)}
                        className={`block size-[10px] ${SHADES[level(d.count, max)]}`}
                      />
                    ) : (
                      <span key={j} className="block size-[10px]" />
                    ),
                  )}
                </div>
              ))}
            </div>
          </div>
          <div className="text-muted-foreground flex items-center justify-end gap-1 text-[10px]">
            Less
            {SHADES.map((s) => (
              <span key={s} className={`block size-[10px] ${s}`} />
            ))}
            More
          </div>
        </div>
      </div>
    </div>
  );
}
