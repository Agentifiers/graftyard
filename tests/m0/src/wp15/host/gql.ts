// M0-WP15: GraphQL queries on the prototype host, executed against GitHub's published GHES 3.22 schema
// (https://docs.github.com/public/ghes-3.22/schema.docs-enterprise.graphql, bundled from .gen/ghes-3.22.graphql). The
// schema validates every query exactly as GHES's would, so a client that sends a field GHES lacks gets GHES's error.
// Resolvers serve viewer, repository (issue, pullRequest, issues, pullRequests, ref, defaultBranchRef), node, and
// rateLimit from the folds; every other field resolves to its type's empty value (null, [], 0, false), which GHES
// would fill with data. Mutations still go through host.ts's `decide` path.

import { buildSchema, parse, validate, execute, specifiedRules, OverlappingFieldsCanBeMergedRule, type GraphQLSchema, GraphQLObjectType, GraphQLNonNull, GraphQLList, GraphQLScalarType, GraphQLEnumType,
  type GraphQLFieldResolver, type GraphQLOutputType, isAbstractType } from "graphql";
// @ts-expect-error -- a text module, bundled by Wrangler's `rules` (wp15/graft-wp15-host.jsonc)
import SDL from "../../../.gen/ghes-3.22.graphql";
import type { Host } from "./host.ts";
import type { Fold } from "./model.ts";
import { caller } from "./ghes.ts";

let schema: GraphQLSchema | null = null;
function getSchema(): GraphQLSchema {
  if (!schema) schema = buildSchema(SDL as string, { assumeValidSDL: true });
  return schema;
}

/** The empty value of a type: what a resolver returns when the prototype has no data for a field. */
function empty(t: GraphQLOutputType): unknown {
  const inner = t instanceof GraphQLNonNull ? t.ofType : t;
  if (inner instanceof GraphQLList) return [];
  if (!(t instanceof GraphQLNonNull)) return null;
  if (inner instanceof GraphQLScalarType) return ({ Int: 0, Float: 0, Boolean: false, String: "", ID: "", DateTime: "1970-01-01T00:00:00Z", URI: "", GitObjectID: "0".repeat(40),
    HTML: "", GitTimestamp: "1970-01-01T00:00:00Z", Date: "1970-01-01", BigInt: "0" } as Record<string, unknown>)[inner.name] ?? "";
  if (inner instanceof GraphQLEnumType) return inner.getValues()[0]?.value ?? null;
  if (inner instanceof GraphQLObjectType) return { __typename: inner.name };
  if (isAbstractType(inner)) return null;
  return null;
}
const fallback: GraphQLFieldResolver<any, any> = (src, _a, _c, info) => {
  if (src && typeof src === "object" && info.fieldName in src) { const v = src[info.fieldName]; return typeof v === "function" ? v() : v; }
  if (info.fieldName === "totalCount") return Array.isArray(src?.nodes) ? src.nodes.length : 0;
  if (info.fieldName === "pageInfo") return { hasNextPage: false, hasPreviousPage: false, endCursor: null, startCursor: null };
  return empty(info.returnType);
};

const iso = (ms: number) => new Date(ms).toISOString().replace(/\.\d{3}Z$/, "Z");

export async function graphqlQuery(h: Host, req: Request, query: string, variables: Record<string, unknown>, opName?: string): Promise<Response> {
  const cred = await caller(h, req);
  if (!cred) return Response.json({ message: "Bad credentials", documentation_url: "https://docs.github.com/enterprise-server@3.22/graphql" }, { status: 401 });
  const userId = cred.t === "Pat" || cred.t === "UserToServer" ? cred.user : null;
  const login = (id: number) => h.authz.users[id]?.login ?? `user-${id}`;
  const actor = (key: string) => { const a = /^a(\d+)i/.exec(key); return a ? { __typename: "Bot", login: `${h.authz.apps[a[1]]?.slug ?? "app"}[bot]`, id: `BOT_${a[1]}` }
    : { __typename: "User", login: login(Number(key.slice(1))), id: `U_${key.slice(1)}`, name: login(Number(key.slice(1))), databaseId: Number(key.slice(1)) }; };
  const canSee = (f: Fold) => userId == null ? true : f.collaborators[`u${userId}`] != null;
  const repoObj = (f: Fold) => {
    const [owner, name] = f.repo.split("/");
    const issue = (n: number) => { const i = f.issues?.[String(n)]; if (!i) return null; const r = i.revisions.at(-1)!;
      return { __typename: "Issue", id: `I_${f.repo}#${n}`, number: n, title: r.title, body: r.body, state: "OPEN", url: `https://${h.authz.host}/${f.repo}/issues/${n}`,
        author: actor(i.author), createdAt: iso(i.createdAt), updatedAt: iso(i.createdAt), closed: false, locked: false, comments: { nodes: (f.comments ?? []).filter((c) => c.issue === n)
          .map((c) => ({ __typename: "IssueComment", id: `IC_${c.id}`, body: c.body, author: actor(c.author), createdAt: iso(c.createdAt) })) },
        assignees: { nodes: (i.assigned.at(-1)?.assignees ?? []).map((l) => ({ __typename: "User", login: l })) }, labels: { nodes: [] } }; };
    const pr = (n: number) => { const p = f.prs[String(n)]; if (!p) return null;
      return { __typename: "PullRequest", id: `PR_${n}`, number: n, title: p.title ?? "", body: p.body ?? "", state: p.merged ? "MERGED" : p.open ? "OPEN" : "CLOSED", isDraft: p.draft,
        headRefName: p.headRef.replace("refs/heads/", ""), headRefOid: p.headOid, baseRefName: p.base.replace("refs/heads/", ""), author: actor(p.author),
        url: `https://${h.authz.host}/${f.repo}/pull/${n}`, mergeable: "UNKNOWN", closed: !p.open, merged: p.merged }; };
    return { __typename: "Repository", id: `R_${f.repo}`, name, nameWithOwner: f.repo, owner: { __typename: "Organization", login: owner, id: `O_${owner}` },
      url: `https://${h.authz.host}/${f.repo}`, isPrivate: true, visibility: "PRIVATE", viewerPermission: userId != null ? (f.collaborators[`u${userId}`] ?? "READ").toUpperCase() : "READ",
      hasIssuesEnabled: true, defaultBranchRef: f.refs[f.defaultBranch] ? { __typename: "Ref", name: f.defaultBranch.replace("refs/heads/", ""), target: { __typename: "Commit", oid: f.refs[f.defaultBranch] } } : null,
      issue: ({ number }: { number: number }) => issue(number), pullRequest: ({ number }: { number: number }) => pr(number),
      issueOrPullRequest: ({ number }: { number: number }) => issue(number) ?? pr(number),
      issues: () => ({ nodes: Object.keys(f.issues ?? {}).map(Number).sort((a, b) => b - a).map(issue) }),
      pullRequests: () => ({ nodes: Object.keys(f.prs).map(Number).sort((a, b) => b - a).map(pr) }),
      ref: ({ qualifiedName }: { qualifiedName: string }) => { const n = qualifiedName.startsWith("refs/") ? qualifiedName : `refs/heads/${qualifiedName}`;
        return f.refs[n] ? { __typename: "Ref", name: n.replace(/^refs\/(heads|tags)\//, ""), prefix: n.replace(/[^/]+$/, ""), target: { __typename: "Commit", oid: f.refs[n] } } : null; } };
  };
  const root = {
    viewer: userId != null ? { __typename: "User", login: login(userId), id: `U_${userId}`, databaseId: userId, name: login(userId) } : { __typename: "Bot", login: "bot" },
    repository: ({ owner, name }: { owner: string; name: string }) => { const f = h.folds[`${owner}/${name}`]; return f && canSee(f) ? repoObj(f) : null; },
    rateLimit: { __typename: "RateLimit", limit: 5000, cost: 1, remaining: 4999, used: 1, resetAt: iso(Date.now() + 3_600_000), nodeCount: 1 },
  };
  // Field arguments arrive as the resolver's first argument when the source value is a function (graphql-js default).
  // graphql-ruby (GitHub) accepts same-named fields of different types under disjoint type conditions, which `gh`
  // sends (`...on Issue{state} ...on PullRequest{state}`); graphql-js's OverlappingFieldsCanBeMerged rejects it.
  let doc;
  try { doc = parse(query); } catch (e) { return Response.json({ errors: [{ message: (e as Error).message }] }); }
  const verrs = validate(getSchema(), doc, specifiedRules.filter((r) => r !== OverlappingFieldsCanBeMergedRule));
  if (verrs.length) return Response.json({ errors: verrs.map((e) => ({ message: e.message, locations: e.locations, extensions: { code: "undefinedField" } })) });
  const out = await execute({ schema: getSchema(), document: doc, variableValues: variables, operationName: opName, rootValue: root,
    fieldResolver: (src, args, ctx, info) => {
      if (src && typeof src === "object" && typeof src[info.fieldName] === "function") return src[info.fieldName](args);
      return fallback(src, args, ctx, info);
    },
    typeResolver: (v) => (v as { __typename?: string })?.__typename });
  const errors = out.errors?.map((e) => ({ message: e.message, locations: e.locations, path: e.path, ...(e.path ? {} : { extensions: { code: "undefinedField" } }) }));
  if (!out.data && out.errors) return Response.json({ errors }, { status: 200 });
  const missing = (out.data && Object.entries(out.data).some(([k, v]) => k === "repository" && v === null)) ?
    [{ type: "NOT_FOUND", path: ["repository"], locations: [{ line: 1, column: 1 }], message: `Could not resolve to a Repository with the name '${String(variables.owner ?? "")}/${String(variables.name ?? variables.repo ?? "")}'.` }] : [];
  return Response.json({ data: out.data, ...(errors?.length || missing.length ? { errors: [...(errors ?? []), ...missing] } : {}) });
}
