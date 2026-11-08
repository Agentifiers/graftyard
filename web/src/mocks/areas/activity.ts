import { HttpResponse } from "msw";
import { createOpenApiHttp } from "openapi-msw";
import { UI_BASE, WATCH_EVENTS, type UiPaths } from "@/areas/activity/ui-api";
import { parseQuery, type SearchType } from "@/areas/activity/search-query";
import { failing, serviceUnavailable } from "../faults";
import { gate, http, notFound, page, unprocessable } from "../http";
import { HOST, findRepo, me, users } from "../world";
import { db as core } from "./core-db";
import { POLL_INTERVAL_S, activity, minimal, type Bucket } from "./activity-db";
import {
  searchCode,
  searchCommits,
  searchIssues,
  searchLabels,
  searchRepositories,
  searchUsers,
  sortBy,
} from "./activity-search";

// Notifications, search, stars, watching, events, rate limit, meta, and feeds.
// Every write has its own fault key; reads share their region's key.

const ui = createOpenApiHttp<UiPaths>({ baseUrl: UI_BASE });

type Q = { get(name: string): string | null };

// GHES headers on every API answer: the rate-limit bucket after this request.
function rateHeaders(b: Bucket = "core") {
  const r = activity.spend(b);
  return {
    "X-RateLimit-Limit": String(r.limit),
    "X-RateLimit-Remaining": String(r.remaining),
    "X-RateLimit-Reset": String(r.reset),
    "X-RateLimit-Used": String(r.used),
    "X-RateLimit-Resource": b,
  };
}

function pollHeaders() {
  return { ...rateHeaders(), "X-Poll-Interval": String(POLL_INTERVAL_S) };
}

// The /notifications filters: all, participating, since, before.
const PARTICIPATING = new Set([
  "assign",
  "author",
  "comment",
  "mention",
  "review_requested",
  "team_mention",
  "state_change",
]);

function inbox(query: Q, repo?: string) {
  const all = query.get("all") === "true";
  const participating = query.get("participating") === "true";
  const since = query.get("since");
  const before = query.get("before");
  return activity
    .threads("inbox")
    .map((t) => t.thread)
    .filter(
      (t) =>
        (all || t.unread) &&
        (!participating || PARTICIPATING.has(t.reason)) &&
        (!since || t.updated_at >= since) &&
        (!before || t.updated_at < before) &&
        (!repo || t.repository.full_name.toLowerCase() === repo),
    )
    .sort((a, b) => b.updated_at.localeCompare(a.updated_at));
}

const order = (q: Q) => (q.get("order") === "asc" ? "asc" : "desc");

// Search answers 503 when the whole search region fails or only this result type does,
// so the page can show one type's error while the other counts still load.
async function searchGate(type: SearchType) {
  const fault = await gate("search");
  if (fault) return fault;
  return failing(`search-${type}`)
    ? HttpResponse.json(serviceUnavailable(), { status: 503 })
    : undefined;
}

// Search: the shared envelope, the 422 GHES sends for an empty q, and its own bucket.
function searchGuard(q: string | null) {
  if (!q || !q.trim()) return unprocessable("Search", "q", "missing");
  if (q.length > 256) return unprocessable("Search", "q", "invalid");
  return undefined;
}

export const handlers = [
  // ---- Notifications ----

  http.get("/notifications", async ({ query, response }) => {
    const fault = await gate("notifications");
    if (fault) return response.untyped(fault);
    return response(200).json(page(inbox(query), query), { headers: pollHeaders() });
  }),

  http.put("/notifications", async ({ request, response }) => {
    const fault = await gate("notifications-update");
    if (fault) return response.untyped(fault);
    const body = (await request.json().catch(() => undefined)) ?? {};
    const before = ("last_read_at" in body && body.last_read_at) || new Date().toISOString();
    if (Number.isNaN(Date.parse(before)))
      return response.untyped(unprocessable("Notification", "last_read_at"));
    activity.markAllRead(before);
    return response(205).empty();
  }),

  http.get("/repos/{owner}/{repo}/notifications", async ({ params, query, response }) => {
    const fault = await gate("notifications");
    if (fault) return response.untyped(fault);
    if (!findRepo(params.owner, params.repo)) return response.untyped(notFound());
    const full = `${params.owner}/${params.repo}`.toLowerCase();
    return response(200).json(page(inbox(query, full), query), { headers: pollHeaders() });
  }),

  http.put("/repos/{owner}/{repo}/notifications", async ({ params, request, response }) => {
    const fault = await gate("notifications-update");
    if (fault) return response.untyped(fault);
    if (!findRepo(params.owner, params.repo)) return response.untyped(notFound());
    const body = (await request.json().catch(() => undefined)) ?? {};
    activity.markAllRead(
      ("last_read_at" in body && body.last_read_at) || new Date().toISOString(),
      `${params.owner}/${params.repo}`.toLowerCase(),
    );
    return response(205).empty();
  }),

  http.get("/notifications/threads/{thread_id}", async ({ params, response }) => {
    const fault = await gate("notifications");
    if (fault) return response.untyped(fault);
    const t = activity.thread(params.thread_id);
    return t ? response(200).json(t.thread) : response.untyped(notFound());
  }),

  http.patch("/notifications/threads/{thread_id}", async ({ params, response }) => {
    const fault = await gate("notifications-update");
    if (fault) return response.untyped(fault);
    return activity.markRead(params.thread_id)
      ? response(205).empty()
      : response.untyped(notFound());
  }),

  http.delete("/notifications/threads/{thread_id}", async ({ params, response }) => {
    const fault = await gate("notifications-update");
    if (fault) return response.untyped(fault);
    activity.markDone(params.thread_id);
    return response(204).empty();
  }),

  http.get("/notifications/threads/{thread_id}/subscription", async ({ params, response }) => {
    const fault = await gate("notifications");
    if (fault) return response.untyped(fault);
    const t = activity.thread(params.thread_id);
    return t ? response(200).json(activity.threadSubscription(t)) : response.untyped(notFound());
  }),

  http.put(
    "/notifications/threads/{thread_id}/subscription",
    async ({ params, request, response }) => {
      const fault = await gate("notifications-update");
      if (fault) return response.untyped(fault);
      const body = (await request.json().catch(() => undefined)) ?? {};
      const ignored = "ignored" in body && Boolean(body.ignored);
      const t = activity.setThreadSubscribed(params.thread_id, !ignored);
      return t
        ? response(200).json(activity.threadSubscription(t, ignored))
        : response.untyped(notFound());
    },
  ),

  http.delete("/notifications/threads/{thread_id}/subscription", async ({ params, response }) => {
    const fault = await gate("notifications-update");
    if (fault) return response.untyped(fault);
    return activity.setThreadSubscribed(params.thread_id, false)
      ? response(204).empty()
      : response.untyped(notFound());
  }),

  ui.get("/notifications/saved", async ({ response }) => {
    const fault = await gate("notifications");
    if (fault) return response.untyped(fault);
    const list = activity.saved().map((t) => t.thread);
    return response(200).json(list.sort((a, b) => b.updated_at.localeCompare(a.updated_at)));
  }),

  ui.put("/notifications/saved/{thread_id}", async ({ params, response }) => {
    const fault = await gate("notifications-update");
    if (fault) return response.untyped(fault);
    activity.setSaved(params.thread_id, true);
    return response(204).empty();
  }),

  ui.delete("/notifications/saved/{thread_id}", async ({ params, response }) => {
    const fault = await gate("notifications-update");
    if (fault) return response.untyped(fault);
    activity.setSaved(params.thread_id, false);
    return response(204).empty();
  }),

  ui.get("/notifications/done", async ({ response }) => {
    const fault = await gate("notifications");
    if (fault) return response.untyped(fault);
    const list = activity.threads("done").map((t) => t.thread);
    return response(200).json(list.sort((a, b) => b.updated_at.localeCompare(a.updated_at)));
  }),

  ui.delete("/notifications/done/{thread_id}", async ({ params, response }) => {
    const fault = await gate("notifications-update");
    if (fault) return response.untyped(fault);
    return activity.moveToInbox(params.thread_id)
      ? response(204).empty()
      : response.untyped(notFound());
  }),

  ui.put("/notifications/threads/{thread_id}/unread", async ({ params, response }) => {
    const fault = await gate("notifications-update");
    if (fault) return response.untyped(fault);
    return activity.markUnread(params.thread_id)
      ? response(204).empty()
      : response.untyped(notFound());
  }),

  // ---- Search ----

  http.get("/search/repositories", async ({ query, response }) => {
    const fault = await searchGate("repositories");
    if (fault) return response.untyped(fault);
    const bad = searchGuard(query.get("q"));
    if (bad) return response.untyped(bad);
    const sort = query.get("sort");
    const items = sortBy(
      searchRepositories(parseQuery(query.get("q")!)),
      sort === "stars"
        ? (r) => r.stargazers_count
        : sort === "forks"
          ? (r) => r.forks_count
          : sort === "updated"
            ? (r) => r.updated_at
            : (r) => r.score,
      order(query),
    );
    return response(200).json(
      { total_count: items.length, incomplete_results: false, items: page(items, query) },
      { headers: rateHeaders("search") },
    );
  }),

  http.get("/search/issues", async ({ query, response }) => {
    const q = query.get("q");
    const type = q && /\bis:pr\b|\btype:pr\b/.test(q) ? "pullrequests" : "issues";
    const fault = await searchGate(type);
    if (fault) return response.untyped(fault);
    const bad = searchGuard(q);
    if (bad) return response.untyped(bad);
    const sort = query.get("sort");
    const items = sortBy(
      searchIssues(parseQuery(q!)),
      sort === "created"
        ? (i) => i.created_at
        : sort === "updated"
          ? (i) => i.updated_at
          : sort === "comments"
            ? (i) => i.comments
            : (i) => i.score,
      order(query),
    );
    return response(200).json(
      {
        total_count: items.length,
        incomplete_results: false,
        items: page(items, query),
        search_type: "lexical",
      },
      { headers: rateHeaders("search") },
    );
  }),

  http.get("/search/users", async ({ query, response }) => {
    const fault = await searchGate("users");
    if (fault) return response.untyped(fault);
    const bad = searchGuard(query.get("q"));
    if (bad) return response.untyped(bad);
    const sort = query.get("sort");
    const items = sortBy(
      searchUsers(parseQuery(query.get("q")!)),
      sort === "followers"
        ? (u) => u.followers ?? 0
        : sort === "repositories"
          ? (u) => u.public_repos ?? 0
          : sort === "joined"
            ? (u) => u.created_at ?? ""
            : (u) => u.score,
      order(query),
    );
    return response(200).json(
      { total_count: items.length, incomplete_results: false, items: page(items, query) },
      { headers: rateHeaders("search") },
    );
  }),

  http.get("/search/code", async ({ query, response }) => {
    const fault = await searchGate("code");
    if (fault) return response.untyped(fault);
    const bad = searchGuard(query.get("q"));
    if (bad) return response.untyped(bad);
    const p = parseQuery(query.get("q")!);
    // GHES needs at least one search term for code, qualifiers alone are not enough.
    if (!p.terms.length) return response.untyped(unprocessable("Search", "q", "missing"));
    const items = sortBy(
      searchCode(p),
      query.get("sort") === "indexed" ? (c) => c.last_modified_at ?? "" : (c) => c.score,
      order(query),
    );
    return response(200).json(
      { total_count: items.length, incomplete_results: false, items: page(items, query) },
      { headers: rateHeaders("code_search") },
    );
  }),

  http.get("/search/commits", async ({ query, response }) => {
    const fault = await searchGate("commits");
    if (fault) return response.untyped(fault);
    const bad = searchGuard(query.get("q"));
    if (bad) return response.untyped(bad);
    const sort = query.get("sort");
    const items = sortBy(
      searchCommits(parseQuery(query.get("q")!)),
      sort === "author-date"
        ? (c) => c.commit.author.date
        : sort === "committer-date"
          ? (c) => c.commit.committer?.date ?? ""
          : (c) => c.score,
      order(query),
    );
    return response(200).json(
      { total_count: items.length, incomplete_results: false, items: page(items, query) },
      { headers: rateHeaders("search") },
    );
  }),

  http.get("/search/labels", async ({ query, response }) => {
    const fault = await searchGate("labels");
    if (fault) return response.untyped(fault);
    const bad = searchGuard(query.get("q"));
    if (bad) return response.untyped(bad);
    const repositoryId = Number(query.get("repository_id"));
    if (!repositoryId) return response.untyped(unprocessable("Search", "repository_id", "missing"));
    const found = searchLabels(repositoryId, parseQuery(query.get("q")!));
    if (!found) return response.untyped(notFound());
    const items = sortBy(found, (l) => l.score, order(query));
    return response(200).json(
      { total_count: items.length, incomplete_results: false, items: page(items, query) },
      { headers: rateHeaders("search") },
    );
  }),

  // ---- Stars ----

  http.get("/user/starred", async ({ query, response }) => {
    const fault = await gate("stars");
    if (fault) return response.untyped(fault);
    const list = sortBy(
      activity.starredBy(me.login),
      query.get("sort") === "updated" ? (s) => s.repo.updated_at ?? "" : (s) => s.at,
      query.get("direction") === "asc" ? "asc" : "desc",
    );
    return response(200).json(
      page(
        list.map((s) => s.repo),
        query,
      ),
    );
  }),

  http.get("/users/{username}/starred", async ({ params, query, response }) => {
    const fault = await gate("stars");
    if (fault) return response.untyped(fault);
    const list = sortBy(
      activity.starredBy(params.username),
      query.get("sort") === "updated" ? (s) => s.repo.updated_at ?? "" : (s) => s.at,
      query.get("direction") === "asc" ? "asc" : "desc",
    );
    return response(200).json(
      page(
        list.map((s) => s.repo),
        query,
      ),
    );
  }),

  http.get("/user/starred/{owner}/{repo}", async ({ params, response }) => {
    const fault = await gate("stars");
    if (fault) return response.untyped(fault);
    if (!findRepo(params.owner, params.repo)) return response.untyped(notFound());
    return activity.isStarred(params.owner, params.repo)
      ? response(204).empty()
      : response(404).json({
          message: "Not Found",
          documentation_url: `${HOST}/docs`,
          status: "404",
        });
  }),

  http.put("/user/starred/{owner}/{repo}", async ({ params, response }) => {
    const fault = await gate("star-toggle");
    if (fault) return response.untyped(fault);
    return activity.star(params.owner, params.repo)
      ? response(204).empty()
      : response.untyped(notFound());
  }),

  http.delete("/user/starred/{owner}/{repo}", async ({ params, response }) => {
    const fault = await gate("star-toggle");
    if (fault) return response.untyped(fault);
    return activity.unstar(params.owner, params.repo)
      ? response(204).empty()
      : response.untyped(notFound());
  }),

  http.get("/repos/{owner}/{repo}/stargazers", async ({ params, query, response }) => {
    const fault = await gate("stars");
    if (fault) return response.untyped(fault);
    if (!findRepo(params.owner, params.repo)) return response.untyped(notFound());
    const list = activity
      .stargazers(params.owner, params.repo)
      .map((s) => ({ ...s.user, starred_at: s.at }));
    return response(200).json(page(list, query));
  }),

  // ---- Watching ----

  http.get("/repos/{owner}/{repo}/subscribers", async ({ params, query, response }) => {
    const fault = await gate("watch");
    if (fault) return response.untyped(fault);
    if (!findRepo(params.owner, params.repo)) return response.untyped(notFound());
    return response(200).json(page(activity.watchers(params.owner, params.repo), query));
  }),

  http.get("/repos/{owner}/{repo}/subscription", async ({ params, response }) => {
    const fault = await gate("watch");
    if (fault) return response.untyped(fault);
    if (!findRepo(params.owner, params.repo)) return response.untyped(notFound());
    const w = activity.watch(params.owner, params.repo);
    // GHES answers 404 when the viewer has no subscription: "participating" in the UI.
    return w
      ? response(200).json(activity.subscription(params.owner, params.repo, w))
      : response(404).empty();
  }),

  http.put("/repos/{owner}/{repo}/subscription", async ({ params, request, response }) => {
    const fault = await gate("watch-update");
    if (fault) return response.untyped(fault);
    if (!findRepo(params.owner, params.repo)) return response.untyped(notFound());
    const body = (await request.json().catch(() => undefined)) ?? {};
    const w = activity.setWatch(params.owner, params.repo, body);
    return response(200).json(activity.subscription(params.owner, params.repo, w));
  }),

  http.delete("/repos/{owner}/{repo}/subscription", async ({ params, response }) => {
    const fault = await gate("watch-update");
    if (fault) return response.untyped(fault);
    activity.unwatch(params.owner, params.repo);
    return response(204).empty();
  }),

  ui.get("/repos/{owner}/{repo}/subscription/events", async ({ params, response }) => {
    const fault = await gate("watch");
    if (fault) return response.untyped(fault);
    return response(200).json({
      events: activity.watch(params.owner, params.repo)?.events ?? null,
    });
  }),

  ui.put("/repos/{owner}/{repo}/subscription/events", async ({ params, request, response }) => {
    const fault = await gate("watch-update");
    if (fault) return response.untyped(fault);
    if (!findRepo(params.owner, params.repo)) return response.untyped(notFound());
    const { events } = await request.json();
    if (!events.length)
      return response.untyped(unprocessable("Subscription", "events", "missing_field"));
    if (events.some((e) => !WATCH_EVENTS.includes(e))) {
      return response.untyped(unprocessable("Subscription", "events", "invalid"));
    }
    const repo = findRepo(params.owner, params.repo)!;
    if (events.includes("discussions") && !repo.has_discussions) {
      return response(422).json({
        message: "Validation Failed",
        documentation_url: `${HOST}/docs`,
        errors: [
          {
            resource: "Subscription",
            field: "events",
            code: "invalid",
            message: `Discussions are turned off for ${repo.full_name}.`,
          },
        ],
      });
    }
    return response(200).json({
      events: activity.setCustom(params.owner, params.repo, events).events,
    });
  }),

  ui.delete("/repos/{owner}/{repo}/subscription/events", async ({ params, response }) => {
    const fault = await gate("watch-update");
    if (fault) return response.untyped(fault);
    activity.clearCustom(params.owner, params.repo);
    return response(204).empty();
  }),

  http.get("/user/subscriptions", async ({ query, response }) => {
    const fault = await gate("watch");
    if (fault) return response.untyped(fault);
    return response(200).json(page(activity.watched().map(minimal), query));
  }),

  // ---- Events ----

  http.get("/users/{username}/received_events", async ({ params, query, response }) => {
    const fault = await gate("feed");
    if (fault) return response.untyped(fault);
    // The viewer receives what happens in repositories they watch or star, by anyone else.
    const own = params.username.toLowerCase() === me.login;
    const list = activity
      .events()
      .filter((e) => !own || e.actor.login !== me.login || e.type === "PushEvent");
    return response(200).json(page(list, query), { headers: pollHeaders() });
  }),

  http.get("/users/{username}/received_events/public", async ({ query, response }) => {
    const fault = await gate("feed");
    if (fault) return response.untyped(fault);
    return response(200).json(
      page(
        activity.events().filter((e) => e.public),
        query,
      ),
    );
  }),

  http.get("/users/{username}/events", async ({ params, query, response }) => {
    const fault = await gate("feed");
    if (fault) return response.untyped(fault);
    const list = activity.events().filter((e) => e.actor.login === params.username.toLowerCase());
    return response(200).json(page(list, query));
  }),

  http.get("/users/{username}/events/public", async ({ params, query, response }) => {
    const fault = await gate("feed");
    if (fault) return response.untyped(fault);
    const list = activity
      .events()
      .filter((e) => e.public && e.actor.login === params.username.toLowerCase());
    return response(200).json(page(list, query));
  }),

  http.get("/users/{username}/events/orgs/{org}", async ({ params, query, response }) => {
    const fault = await gate("feed");
    if (fault) return response.untyped(fault);
    const list = activity.events().filter((e) => e.org?.login === params.org.toLowerCase());
    return response(200).json(page(list, query));
  }),

  http.get("/events", async ({ query, response }) => {
    const fault = await gate("feed");
    if (fault) return response.untyped(fault);
    return response(200).json(
      page(
        activity.events().filter((e) => e.public),
        query,
      ),
      { headers: pollHeaders() },
    );
  }),

  http.get("/repos/{owner}/{repo}/events", async ({ params, query, response }) => {
    const fault = await gate("feed");
    if (fault) return response.untyped(fault);
    if (!core.state(params.owner, params.repo)) return response.untyped(notFound());
    const full = `${params.owner}/${params.repo}`.toLowerCase();
    return response(200).json(
      page(
        activity.events().filter((e) => e.repo.name.toLowerCase() === full),
        query,
      ),
    );
  }),

  http.get("/feeds", async ({ response }) => {
    const fault = await gate("feed");
    if (fault) return response.untyped(fault);
    const atom = "application/atom+xml";
    const token = "token=AMOCKFEEDTOKEN";
    return response(200).json({
      timeline_url: `${HOST}/timeline`,
      user_url: `${HOST}/{user}`,
      current_user_public_url: `${HOST}/${me.login}`,
      current_user_url: `${HOST}/${me.login}.private?${token}`,
      current_user_actor_url: `${HOST}/${me.login}.private.actor?${token}`,
      current_user_organization_url: "",
      current_user_organization_urls: [
        `${HOST}/organizations/${users.org.login}/${me.login}.private.atom?${token}`,
      ],
      security_advisories_url: `${HOST}/security-advisories`,
      _links: {
        timeline: { href: `${HOST}/timeline`, type: atom },
        user: { href: `${HOST}/{user}`, type: atom },
        current_user_public: { href: `${HOST}/${me.login}`, type: atom },
        current_user: { href: `${HOST}/${me.login}.private?${token}`, type: atom },
        current_user_actor: { href: `${HOST}/${me.login}.private.actor?${token}`, type: atom },
        current_user_organizations: [
          {
            href: `${HOST}/organizations/${users.org.login}/${me.login}.private.atom?${token}`,
            type: atom,
          },
        ],
        security_advisories: { href: `${HOST}/security-advisories`, type: atom },
      },
    });
  }),

  // ---- Rate limit and meta ----

  http.get("/rate_limit", async ({ response }) => {
    const fault = await gate("rate-limit");
    if (fault) return response.untyped(fault);
    // GET /rate_limit does not count against core, as on GHES.
    const resources = {
      core: activity.bucket("core"),
      search: activity.bucket("search"),
      code_search: activity.bucket("code_search"),
      graphql: activity.bucket("graphql"),
      integration_manifest: activity.bucket("integration_manifest"),
      source_import: activity.bucket("source_import"),
    };
    const core = resources.core;
    return response(200).json(
      { resources, rate: core },
      {
        headers: {
          "X-RateLimit-Limit": String(core.limit),
          "X-RateLimit-Remaining": String(core.remaining),
          "X-RateLimit-Reset": String(core.reset),
        },
      },
    );
  }),

  http.get("/meta", async ({ response }) => {
    const fault = await gate("meta");
    if (fault) return response.untyped(fault);
    return response(200).json({
      verifiable_password_authentication: true,
      installed_version: "3.22.0",
      packages: [],
      domains: { website: [HOST], packages: [], actions: [`${HOST}/_services/pipelines`] },
    });
  }),
];
