import type { Schemas } from "@/api/client";
import type { TrafficPoint } from "@/areas/code/ui-api";
import { sha, type Repository, type SimpleUser } from "../world";
import { blob, commitFiles, history, type CodeRepo, type Commit } from "./code-db";

// Statistics derived from the commit graph on the default branch, as GHES computes them
// for /languages, /contributors, and /stats/*. Traffic has no git source, so it is a
// seeded series scaled by the repository's stars.

const LANGUAGE: [RegExp, string][] = [
  [/\.rs$/, "Rust"],
  [/\.tsx?$/, "TypeScript"],
  [/\.css$/, "CSS"],
  [/\.sh$/, "Shell"],
  [/\.toml$/, "TOML"],
  [/(^|\/)Dockerfile$/, "Dockerfile"],
];

export function languages(tip: Commit): Schemas["language"] {
  const out: Record<string, number> = {};
  for (const [path, key] of tip.files) {
    const lang = LANGUAGE.find(([re]) => re.test(path))?.[1];
    if (lang && lang !== "TOML") out[lang] = (out[lang] ?? 0) + (blob(key)?.size ?? 0) * 7;
  }
  return Object.fromEntries(Object.entries(out).sort((a, b) => b[1] - a[1]));
}

function byAuthor(code: CodeRepo, tip: Commit) {
  const map = new Map<string, { user: SimpleUser; commits: Commit[] }>();
  for (const c of history(code, tip.sha)) {
    const u = c.author.user;
    if (!u) continue;
    const entry = map.get(u.login) ?? { user: u, commits: [] };
    entry.commits.push(c);
    map.set(u.login, entry);
  }
  return [...map.values()].sort((a, b) => b.commits.length - a.commits.length);
}

export function contributors(code: CodeRepo, tip: Commit): Schemas["contributor"][] {
  return byAuthor(code, tip).map(({ user: { name, email, ...user }, commits }) => ({
    ...user,
    ...(name ? { name } : {}),
    ...(email ? { email } : {}),
    contributions: commits.length,
  }));
}

const WEEK = 7 * 86400;
const weekOf = (date: string) => {
  const t = Math.floor(Date.parse(date) / 1000);
  const d = new Date(t * 1000);
  return t - d.getUTCDay() * 86400 - (t % 86400);
};

function weeks(from: number, to: number) {
  const out: number[] = [];
  for (let w = from; w <= to; w += WEEK) out.push(w);
  return out;
}

const NOW = Date.parse("2026-11-08T12:00:00Z");

export function contributorStats(code: CodeRepo, tip: Commit): Schemas["contributor-activity"][] {
  const all = history(code, tip.sha);
  const span = weeks(weekOf(all.at(-1)!.date), weekOf(new Date(NOW).toISOString()));
  const repo = { url: "", html_url: "" } as Repository;
  return byAuthor(code, tip)
    .reverse()
    .map(({ user, commits }) => ({
      author: user,
      total: commits.length,
      weeks: span.map((w) => {
        const mine = commits.filter((c) => weekOf(c.date) === w);
        const files = mine.flatMap((c) => commitFiles(code, repo, c));
        return {
          w,
          a: files.reduce((n, f) => n + f.additions, 0),
          d: files.reduce((n, f) => n + f.deletions, 0),
          c: mine.length,
        };
      }),
    }));
}

export function commitActivity(code: CodeRepo, tip: Commit): Schemas["commit-activity"][] {
  const all = history(code, tip.sha);
  const last = weekOf(new Date(NOW).toISOString());
  return weeks(last - 51 * WEEK, last).map((week) => {
    const days = [0, 0, 0, 0, 0, 0, 0];
    for (const c of all) if (weekOf(c.date) === week) days[new Date(c.date).getUTCDay()]++;
    return { days, total: days.reduce((a, b) => a + b, 0), week };
  });
}

export function codeFrequency(
  code: CodeRepo,
  repo: Repository,
  tip: Commit,
): Schemas["code-frequency-stat"][] {
  const all = history(code, tip.sha);
  return weeks(weekOf(all.at(-1)!.date), weekOf(new Date(NOW).toISOString())).map((w) => {
    const files = all
      .filter((c) => weekOf(c.date) === w)
      .flatMap((c) => commitFiles(code, repo, c));
    return [
      w,
      files.reduce((n, f) => n + f.additions, 0),
      -files.reduce((n, f) => n + f.deletions, 0),
    ];
  });
}

export function participation(
  code: CodeRepo,
  repo: Repository,
  tip: Commit,
): Schemas["participation-stats"] {
  const activity = commitActivity(code, tip);
  const owner = repo.owner.login;
  const all = history(code, tip.sha);
  return {
    all: activity.map((a) => a.total),
    owner: activity.map(
      (a) => all.filter((c) => weekOf(c.date) === a.week && c.author.user?.login === owner).length,
    ),
  };
}

export function punchCard(code: CodeRepo, tip: Commit): Schemas["code-frequency-stat"][] {
  const grid = new Map<string, number>();
  for (const c of history(code, tip.sha)) {
    const d = new Date(c.date);
    const key = `${d.getUTCDay()}:${d.getUTCHours()}`;
    grid.set(key, (grid.get(key) ?? 0) + 1);
  }
  const out: number[][] = [];
  for (let day = 0; day < 7; day++) {
    for (let hour = 0; hour < 24; hour++) out.push([day, hour, grid.get(`${day}:${hour}`) ?? 0]);
  }
  return out;
}

const noise = (seed: string) => parseInt(sha(seed).slice(0, 6), 16) / 0xffffff;

// The last 14 days, or the last 14 weeks folded from them, as GHES's traffic API reports.
export function traffic(repo: Repository, kind: "views" | "clones", per: "day" | "week") {
  const base = (kind === "views" ? 42 : 9) * (repo.stargazers_count / 100);
  const days = Array.from({ length: per === "week" ? 98 : 14 }, (_, i) => {
    const t = NOW - (per === "week" ? 97 - i : 13 - i) * 86400_000;
    const d = new Date(t);
    const weekend = d.getUTCDay() === 0 || d.getUTCDay() === 6;
    const count = Math.round(
      base * (weekend ? 0.45 : 1) * (0.6 + noise(`${repo.full_name}:${kind}:${i}:${per}`) * 0.9),
    );
    const timestamp = `${d.toISOString().slice(0, 10)}T00:00:00Z`;
    return {
      timestamp,
      count,
      uniques: Math.max(1, Math.round(count * (kind === "views" ? 0.31 : 0.55))),
    };
  });
  const points: TrafficPoint[] =
    per === "day"
      ? days
      : Array.from({ length: 14 }, (_, w) => {
          const slice = days.slice(w * 7, w * 7 + 7);
          return {
            timestamp: slice[0].timestamp,
            count: slice.reduce((n, p) => n + p.count, 0),
            uniques: Math.round(slice.reduce((n, p) => n + p.uniques, 0) * 0.7),
          };
        });
  const count = points.reduce((n, p) => n + p.count, 0);
  const uniques = Math.round(points.reduce((n, p) => n + p.uniques, 0) * 0.62);
  return { points, totals: { count, uniques } };
}
