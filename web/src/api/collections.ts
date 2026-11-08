import { createCollection } from "@tanstack/react-db";
import { queryCollectionOptions } from "@tanstack/query-db-collection";
import { api, unwrap, type Schemas } from "./client";
import { commentsQuery, issueQuery, issuesQuery } from "@/areas/core/queries";
import { queryClient } from "./query-client";

type Repo = { owner: string; repo: string };

function memo<K, V>(make: (key: K) => V, keyOf: (key: K) => string) {
  const cache = new Map<string, V>();
  return (key: K) => {
    const k = keyOf(key);
    let v = cache.get(k);
    if (!v) cache.set(k, (v = make(key)));
    return v;
  };
}

// Every issue and pull request in a repository, as GHES lists them. The UI filters PRs out.
export const issuesCollection = memo(
  (r: Repo) => {
    const { queryKey, queryFn } = issuesQuery(r);
    return createCollection(
      queryCollectionOptions<Schemas["issue"]>({
        id: `issues:${r.owner}/${r.repo}`,
        queryKey,
        queryFn: (ctx) => queryFn!({ ...ctx, queryKey }),
        queryClient,
        getKey: (i) => i.number,
      }),
    );
  },
  (r) => `${r.owner}/${r.repo}`,
);

export type CommentDraft = { body: string };

// An optimistic comment carries a negative id until the server answers with the real one.
let tempId = -1;

export const commentsCollection = memo(
  (r: Repo & { number: number }) => {
    const { queryKey, queryFn } = commentsQuery(r);
    return createCollection(
      queryCollectionOptions<Schemas["issue-comment"]>({
        id: `comments:${r.owner}/${r.repo}#${r.number}`,
        queryKey,
        queryFn: (ctx) => queryFn!({ ...ctx, queryKey }),
        queryClient,
        getKey: (c) => c.id,
        // One POST per comment and no retry: a repeated POST would post twice.
        onInsert: async ({ transaction, collection }) => {
          for (const m of transaction.mutations) {
            const created = await unwrap(
              api.POST("/repos/{owner}/{repo}/issues/{issue_number}/comments", {
                params: { path: { owner: r.owner, repo: r.repo, issue_number: r.number } },
                body: { body: m.modified.body ?? "" },
              }),
            );
            await collection.utils.writeInsert(created);
          }
          // The server bumped the issue's `comments` and `updated_at`; refetch the reads that show them.
          await Promise.all(
            [issueQuery(r), issuesQuery(r)].map(({ queryKey }) =>
              queryClient.invalidateQueries({ queryKey, exact: true }),
            ),
          );
        },
      }),
    );
  },
  (r) => `${r.owner}/${r.repo}#${r.number}`,
);

type Viewer = Schemas["private-user"] | Schemas["public-user"];

export function draftComment(viewer: Viewer, body: string): Schemas["issue-comment"] {
  const now = new Date().toISOString();
  const { login, id, node_id, avatar_url, gravatar_id, url, html_url, type, site_admin } = viewer;
  const user: Schemas["simple-user"] = {
    login,
    id,
    node_id,
    avatar_url,
    gravatar_id,
    url,
    html_url,
    type,
    site_admin,
    followers_url: viewer.followers_url,
    following_url: viewer.following_url,
    gists_url: viewer.gists_url,
    starred_url: viewer.starred_url,
    subscriptions_url: viewer.subscriptions_url,
    organizations_url: viewer.organizations_url,
    repos_url: viewer.repos_url,
    events_url: viewer.events_url,
    received_events_url: viewer.received_events_url,
  };
  return {
    id: tempId--,
    node_id: "",
    url: "",
    html_url: "",
    issue_url: "",
    body,
    user,
    created_at: now,
    updated_at: now,
  };
}
