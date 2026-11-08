import type { Schemas } from "@/api/client";

// The issue list's filters as the URL carries them: a list of typed rules, a sort, a page,
// and a title search. One pure predicate applies them, so the list, the counts, and the
// filter bar agree on what each rule means.

export const FIELDS = ["state", "author", "label", "assignee", "milestone"] as const;
export type Field = (typeof FIELDS)[number];

// The operators each field offers, as the ReUI filter bar names them.
export const OPERATORS = {
  state: ["is_any_of"],
  author: ["is_any_of", "is_none_of"],
  label: ["has_any_of", "has_all_of", "has_none_of"],
  assignee: ["is_any_of", "is_none_of"],
  milestone: ["is_any_of", "is_none_of"],
} as const satisfies Record<Field, readonly string[]>;

export type Rule = {
  [F in Field]: { f: F; op: (typeof OPERATORS)[F][number]; v: string[] };
}[Field];

// The value that means "nothing": no assignee, no milestone.
export const NONE = "none";

export const SORTS = {
  newest: "Newest",
  oldest: "Oldest",
  comments: "Most commented",
  updated: "Recently updated",
} as const;
export type Sort = keyof typeof SORTS;

export type IssueSearch = {
  filters?: Rule[];
  sort?: Sort;
  page?: number;
  q?: string;
};

export const PER_PAGE = 10;

function parseRule(raw: unknown): Rule | undefined {
  if (!raw || typeof raw !== "object") return undefined;
  const { f, op, v } = raw as Record<string, unknown>;
  if (!FIELDS.includes(f as Field)) return undefined;
  const ops: readonly string[] = OPERATORS[f as Field];
  if (typeof op !== "string" || !ops.includes(op)) return undefined;
  const values = Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  if (values.length === 0) return undefined;
  return { f, op, v: values } as Rule;
}

export function validateIssueSearch(search: Record<string, unknown>): IssueSearch {
  const filters = Array.isArray(search.filters)
    ? search.filters.map(parseRule).filter((r): r is Rule => Boolean(r))
    : undefined;
  const page = Number(search.page);
  return {
    filters: filters?.length ? filters : undefined,
    sort:
      typeof search.sort === "string" && search.sort in SORTS ? (search.sort as Sort) : undefined,
    page: Number.isInteger(page) && page > 1 ? page : undefined,
    q: typeof search.q === "string" && search.q ? search.q : undefined,
  };
}

// Open issues unless a state rule says otherwise, as GHES's list defaults to is:open.
export function stateOf(filters: Rule[] = []): "open" | "closed" | "all" {
  const rule = filters.find((r) => r.f === "state");
  if (!rule) return "open";
  return rule.v.includes("open") && rule.v.includes("closed")
    ? "all"
    : rule.v[0] === "closed"
      ? "closed"
      : "open";
}

export function withState(
  filters: Rule[] = [],
  state: "open" | "closed" | "all",
): Rule[] | undefined {
  const rest = filters.filter((r) => r.f !== "state");
  const next: Rule[] =
    state === "open"
      ? rest
      : [
          ...rest,
          { f: "state", op: "is_any_of", v: state === "all" ? ["open", "closed"] : [state] },
        ];
  return next.length ? next : undefined;
}

type Issue = Schemas["issue"];

const labelNames = (i: Issue) => i.labels.map((l) => (typeof l === "string" ? l : (l.name ?? "")));

function matchesRule(i: Issue, r: Rule): boolean {
  switch (r.f) {
    case "state":
      return r.v.includes(i.state);
    case "author": {
      const hit = r.v.includes(i.user?.login ?? "");
      return r.op === "is_any_of" ? hit : !hit;
    }
    case "label": {
      const names = labelNames(i);
      if (r.op === "has_all_of") return r.v.every((v) => names.includes(v));
      const any = r.v.some((v) => names.includes(v));
      return r.op === "has_any_of" ? any : !any;
    }
    case "assignee": {
      const logins = (i.assignees ?? []).map((u) => u.login);
      const hit = r.v.some((v) => (v === NONE ? logins.length === 0 : logins.includes(v)));
      return r.op === "is_any_of" ? hit : !hit;
    }
    case "milestone": {
      const n = i.milestone ? String(i.milestone.number) : NONE;
      const hit = r.v.includes(n);
      return r.op === "is_any_of" ? hit : !hit;
    }
  }
}

// Every rule but state, and the title or #number search. State is split off so the
// list can count open and closed issues under the same filters.
export function matchesExceptState(i: Issue, filters: Rule[] = [], q = ""): boolean {
  if (i.pull_request) return false;
  const needle = q.trim().toLowerCase();
  if (needle) {
    const byNumber = /^#?\d+$/.test(needle) && String(i.number) === needle.replace("#", "");
    if (!byNumber && !i.title.toLowerCase().includes(needle)) return false;
  }
  return filters.every((r) => r.f === "state" || matchesRule(i, r));
}

export function sortKey(sort: Sort = "newest"): {
  by: "created_at" | "updated_at" | "comments";
  dir: "asc" | "desc";
} {
  switch (sort) {
    case "oldest":
      return { by: "created_at", dir: "asc" };
    case "comments":
      return { by: "comments", dir: "desc" };
    case "updated":
      return { by: "updated_at", dir: "desc" };
    default:
      return { by: "created_at", dir: "desc" };
  }
}
