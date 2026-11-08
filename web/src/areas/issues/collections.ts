import { createCollection, localStorageCollectionOptions } from "@tanstack/react-db";
import { queryCollectionOptions } from "@tanstack/query-db-collection";
import { api, type Schemas } from "@/api/client";
import { queryClient } from "@/api/query-client";
import { send } from "./api";
import {
  commentsQuery,
  issueQuery,
  issuesQuery,
  labelsQuery,
  milestonesQuery,
  timelineQuery,
} from "./queries";

// TanStack DB collections for the issues area. Each one shows a write at once, sends it
// once (a repeated POST would post twice), writes the server's answer back, and refetches
// the other reads the write changed. A failed write rolls back and the caller toasts it.

type Repo = { owner: string; repo: string };
type Thread = Repo & { number: number };

function memo<K, V>(make: (key: K) => V, keyOf: (key: K) => string) {
  const cache = new Map<string, V>();
  return (key: K) => {
    const k = keyOf(key);
    let v = cache.get(k);
    if (!v) cache.set(k, (v = make(key)));
    return v;
  };
}

const refetch = (...keys: readonly unknown[][]) =>
  Promise.all(keys.map((queryKey) => queryClient.invalidateQueries({ queryKey })));

// Optimistic records carry a negative id or number until the server answers.
let tempId = -1;
const nextTemp = () => tempId--;

type Viewer = Schemas["private-user"] | Schemas["public-user"];

function simpleUser(viewer: Viewer): Schemas["simple-user"] {
  const { login, id, node_id, avatar_url, gravatar_id, url, html_url, type, site_admin } = viewer;
  return {
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
}

// ---- Issues ----

// The issue number the server gave each optimistic issue, read once the insert settles.
const createdNumbers = new Map<number, number>();
export const createdNumberFor = (temp: number) => createdNumbers.get(temp);

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
        onInsert: async ({ transaction, collection }) => {
          for (const m of transaction.mutations) {
            const draft = m.modified;
            const created = await send(
              api.POST("/repos/{owner}/{repo}/issues", {
                params: { path: r },
                body: {
                  title: draft.title,
                  body: draft.body ?? "",
                  labels: draft.labels.map((l) => (typeof l === "string" ? l : (l.name ?? ""))),
                  assignees: (draft.assignees ?? []).map((u) => u.login),
                  milestone: draft.milestone?.number ?? null,
                },
              }),
            );
            createdNumbers.set(draft.number, created.number);
            await collection.utils.writeInsert(created);
          }
          await refetch(milestonesQuery(r).queryKey);
        },
      }),
    );
  },
  (r) => `${r.owner}/${r.repo}`,
);

export type IssueDraft = {
  title: string;
  body: string;
  labels: Schemas["label"][];
  assignees: Schemas["simple-user"][];
  milestone: Schemas["milestone"] | null;
};

export function draftIssue(viewer: Viewer, d: IssueDraft): Schemas["issue"] {
  const now = new Date().toISOString();
  return {
    id: nextTemp(),
    number: nextTemp(),
    node_id: "",
    url: "",
    repository_url: "",
    labels_url: "",
    comments_url: "",
    events_url: "",
    html_url: "",
    state: "open",
    state_reason: null,
    title: d.title,
    body: d.body,
    user: simpleUser(viewer),
    labels: d.labels,
    assignee: d.assignees[0] ?? null,
    assignees: d.assignees,
    milestone: d.milestone,
    locked: false,
    comments: 0,
    closed_at: null,
    created_at: now,
    updated_at: now,
  };
}

// ---- Comments ----

export type CommentDraft = { body: string };

export const commentsCollection = memo(
  (r: Thread) => {
    const { queryKey, queryFn } = commentsQuery(r);
    const repo = { owner: r.owner, repo: r.repo };
    // The server bumped the issue's `comments` and `updated_at`, and the timeline changed.
    const afterWrite = () =>
      refetch(issueQuery(r).queryKey, issuesQuery(r).queryKey, timelineQuery(r).queryKey);
    return createCollection(
      queryCollectionOptions<Schemas["issue-comment"]>({
        id: `comments:${r.owner}/${r.repo}#${r.number}`,
        queryKey,
        queryFn: (ctx) => queryFn!({ ...ctx, queryKey }),
        queryClient,
        getKey: (c) => c.id,
        onInsert: async ({ transaction, collection }) => {
          for (const m of transaction.mutations) {
            const created = await send(
              api.POST("/repos/{owner}/{repo}/issues/{issue_number}/comments", {
                params: { path: { ...repo, issue_number: r.number } },
                body: { body: m.modified.body ?? "" },
              }),
            );
            await collection.utils.writeInsert(created);
          }
          await afterWrite();
        },
        onUpdate: async ({ transaction, collection }) => {
          for (const m of transaction.mutations) {
            const updated = await send(
              api.PATCH("/repos/{owner}/{repo}/issues/comments/{comment_id}", {
                params: { path: { ...repo, comment_id: m.original.id } },
                body: { body: m.modified.body ?? "" },
              }),
            );
            await collection.utils.writeUpdate(updated);
          }
          await afterWrite();
        },
        onDelete: async ({ transaction, collection }) => {
          for (const m of transaction.mutations) {
            await send(
              api.DELETE("/repos/{owner}/{repo}/issues/comments/{comment_id}", {
                params: { path: { ...repo, comment_id: m.original.id } },
              }),
            );
            await collection.utils.writeDelete(m.original.id);
          }
          await afterWrite();
        },
      }),
    );
  },
  (r) => `${r.owner}/${r.repo}#${r.number}`,
);

export function draftComment(viewer: Viewer, body: string): Schemas["issue-comment"] {
  const now = new Date().toISOString();
  return {
    id: nextTemp(),
    node_id: "",
    url: "",
    html_url: "",
    issue_url: "",
    body,
    user: simpleUser(viewer),
    created_at: now,
    updated_at: now,
  };
}

// ---- Repository labels ----

export const labelsCollection = memo(
  (r: Repo) => {
    const { queryKey, queryFn } = labelsQuery(r);
    // A renamed, recoloured, or deleted label changes every issue that has it.
    const afterWrite = () => refetch(["repos", r.owner, r.repo, "issues"]);
    return createCollection(
      queryCollectionOptions<Schemas["label"]>({
        id: `labels:${r.owner}/${r.repo}`,
        queryKey,
        queryFn: (ctx) => queryFn!({ ...ctx, queryKey }),
        queryClient,
        getKey: (l) => l.id,
        onInsert: async ({ transaction, collection }) => {
          for (const m of transaction.mutations) {
            const { name, color, description } = m.modified;
            const created = await send(
              api.POST("/repos/{owner}/{repo}/labels", {
                params: { path: r },
                body: { name, color, description: description ?? "" },
              }),
            );
            await collection.utils.writeInsert(created);
          }
        },
        onUpdate: async ({ transaction, collection }) => {
          for (const m of transaction.mutations) {
            const { name, color, description } = m.modified;
            const updated = await send(
              api.PATCH("/repos/{owner}/{repo}/labels/{name}", {
                params: { path: { ...r, name: m.original.name } },
                body: { new_name: name, color, description: description ?? "" },
              }),
            );
            await collection.utils.writeUpdate(updated);
          }
          await afterWrite();
        },
        onDelete: async ({ transaction, collection }) => {
          for (const m of transaction.mutations) {
            await send(
              api.DELETE("/repos/{owner}/{repo}/labels/{name}", {
                params: { path: { ...r, name: m.original.name } },
              }),
            );
            await collection.utils.writeDelete(m.original.id);
          }
          await afterWrite();
        },
      }),
    );
  },
  (r) => `${r.owner}/${r.repo}`,
);

export function draftLabel(d: {
  name: string;
  color: string;
  description: string;
}): Schemas["label"] {
  return {
    id: nextTemp(),
    node_id: "",
    url: "",
    name: d.name,
    color: d.color,
    description: d.description || null,
    default: false,
    archived_at: null,
    archived_by: null,
  };
}

// ---- Milestones ----

export const milestonesCollection = memo(
  (r: Repo) => {
    const { queryKey, queryFn } = milestonesQuery(r);
    const afterWrite = () =>
      refetch(["repos", r.owner, r.repo, "issues"], ["repos", r.owner, r.repo, "milestones"]);
    return createCollection(
      queryCollectionOptions<Schemas["milestone"]>({
        id: `milestones:${r.owner}/${r.repo}`,
        queryKey,
        queryFn: (ctx) => queryFn!({ ...ctx, queryKey }),
        queryClient,
        getKey: (m) => m.number,
        onInsert: async ({ transaction, collection }) => {
          for (const m of transaction.mutations) {
            const { title, description, due_on, state } = m.modified;
            const created = await send(
              api.POST("/repos/{owner}/{repo}/milestones", {
                params: { path: r },
                body: { title, description: description ?? "", due_on: due_on ?? undefined, state },
              }),
            );
            createdNumbers.set(m.modified.number, created.number);
            await collection.utils.writeInsert(created);
          }
        },
        onUpdate: async ({ transaction, collection }) => {
          for (const m of transaction.mutations) {
            const { title, description, due_on, state } = m.modified;
            const updated = await send(
              api.PATCH("/repos/{owner}/{repo}/milestones/{milestone_number}", {
                params: { path: { ...r, milestone_number: m.original.number } },
                body: { title, description: description ?? "", due_on: due_on ?? "", state },
              }),
            );
            await collection.utils.writeUpdate(updated);
          }
          await afterWrite();
        },
        onDelete: async ({ transaction, collection }) => {
          for (const m of transaction.mutations) {
            await send(
              api.DELETE("/repos/{owner}/{repo}/milestones/{milestone_number}", {
                params: { path: { ...r, milestone_number: m.original.number } },
              }),
            );
            await collection.utils.writeDelete(m.original.number);
          }
          await afterWrite();
        },
      }),
    );
  },
  (r) => `${r.owner}/${r.repo}`,
);

export function draftMilestone(
  viewer: Viewer,
  d: { title: string; description: string; due_on: string | null },
): Schemas["milestone"] {
  const now = new Date().toISOString();
  return {
    url: "",
    html_url: "",
    labels_url: "",
    id: nextTemp(),
    node_id: "",
    number: nextTemp(),
    state: "open",
    title: d.title,
    description: d.description || null,
    creator: simpleUser(viewer),
    open_issues: 0,
    closed_issues: 0,
    created_at: now,
    updated_at: now,
    closed_at: null,
    due_on: d.due_on,
  };
}

// ---- Pinned issues ----

// GHES 3.22's REST API has no route to pin an issue (only GraphQL does), so pins live in
// this browser. Created on first use, because the shell prerender has no localStorage.
export type PinnedIssue = {
  key: string;
  owner: string;
  repo: string;
  number: number;
  pinnedAt: string;
};

let pins: ReturnType<typeof makePins> | undefined;
function makePins() {
  return createCollection(
    localStorageCollectionOptions<PinnedIssue, string>({
      id: "pinned-issues",
      storageKey: "graftyard.pinned-issues",
      getKey: (p) => p.key,
    }),
  );
}
export const pinnedCollection = () => (pins ??= makePins());
export const pinKey = (t: Thread) => `${t.owner}/${t.repo}#${t.number}`.toLowerCase();
