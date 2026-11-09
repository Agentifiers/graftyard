// The GHES search query language: free-text terms plus `key:value` qualifiers, where a
// value may be quoted (`label:"good first issue"`) and a leading `-` negates it. The
// mock's search handlers and the search page both read a query through this one parser,
// so a qualifier the page shows as a chip is the qualifier the mock applies.

export type Qualifier = { key: string; value: string; negated: boolean };
export type ParsedQuery = { terms: string[]; qualifiers: Qualifier[] };

const TOKEN = /(-?)([a-z_]+):("([^"]*)"|\S+)|"([^"]*)"|(\S+)/gi;

export function parseQuery(q: string): ParsedQuery {
  const terms: string[] = [];
  const qualifiers: Qualifier[] = [];
  for (const m of q.matchAll(TOKEN)) {
    const [, neg, key, raw, quoted, phrase, word] = m;
    if (key) {
      qualifiers.push({ key: key.toLowerCase(), value: quoted ?? raw, negated: neg === "-" });
    } else {
      const term = phrase ?? word;
      if (term) terms.push(term);
    }
  }
  return { terms, qualifiers };
}

export function formatQualifier({ key, value, negated }: Qualifier) {
  return `${negated ? "-" : ""}${key}:${/\s/.test(value) ? `"${value}"` : value}`;
}

export function withoutQualifier(q: string, drop: Qualifier) {
  const { terms, qualifiers } = parseQuery(q);
  const kept = qualifiers.filter(
    (x) => !(x.key === drop.key && x.value === drop.value && x.negated === drop.negated),
  );
  return [...kept.map(formatQualifier), ...terms.map((t) => (/\s/.test(t) ? `"${t}"` : t))].join(
    " ",
  );
}

// Every value of `key`, positive qualifiers only.
export const values = (p: ParsedQuery, key: string) =>
  p.qualifiers.filter((x) => x.key === key && !x.negated).map((x) => x.value.toLowerCase());

export const negatedValues = (p: ParsedQuery, key: string) =>
  p.qualifiers.filter((x) => x.key === key && x.negated).map((x) => x.value.toLowerCase());

// The result types the search page offers, the endpoint each reads, and its sorts.
// Pull requests are /search/issues with `is:pr`, as GHES's web UI does.
export const SEARCH_TYPES = {
  repositories: {
    label: "Repositories",
    one: "repository",
    many: "repositories",
    endpoint: "repositories",
    sorts: { stars: "Most stars", forks: "Most forks", updated: "Recently updated" },
  },
  code: {
    label: "Code",
    one: "code result",
    many: "code results",
    endpoint: "code",
    sorts: { indexed: "Recently indexed" },
  },
  commits: {
    label: "Commits",
    one: "commit",
    many: "commits",
    endpoint: "commits",
    sorts: { "author-date": "Author date", "committer-date": "Committer date" },
  },
  issues: {
    label: "Issues",
    one: "issue",
    many: "issues",
    endpoint: "issues",
    sorts: { created: "Newest", updated: "Recently updated", comments: "Most commented" },
  },
  pullrequests: {
    label: "Pull requests",
    one: "pull request",
    many: "pull requests",
    endpoint: "issues",
    sorts: { created: "Newest", updated: "Recently updated", comments: "Most commented" },
  },
  users: {
    label: "Users",
    one: "user",
    many: "users",
    endpoint: "users",
    sorts: { followers: "Most followers", repositories: "Most repositories", joined: "Joined" },
  },
  labels: {
    label: "Labels",
    one: "label",
    many: "labels",
    endpoint: "labels",
    sorts: { created: "Created", updated: "Updated" },
  },
} as const;

export type SearchType = keyof typeof SEARCH_TYPES;
export const SEARCH_TYPE_KEYS = Object.keys(SEARCH_TYPES) as SearchType[];
export type SearchEndpoint = (typeof SEARCH_TYPES)[SearchType]["endpoint"];

// The query a type sends: pull requests and issues add their `is:` qualifier unless the
// user wrote one.
export function queryFor(type: SearchType, q: string) {
  const p = parseQuery(q);
  const hasKind =
    values(p, "is").some((v) => v === "pr" || v === "issue") || values(p, "type").length;
  if (type === "pullrequests" && !hasKind) return `${q} is:pr`.trim();
  if (type === "issues" && !hasKind) return `${q} is:issue`.trim();
  return q;
}
