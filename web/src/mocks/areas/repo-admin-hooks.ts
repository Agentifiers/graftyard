import type { Schemas } from "@/api/client";
import { gate, http, notFound, page, unprocessable } from "../http";
import { id } from "../world";
import { admin, delivery, deployKey } from "./repo-admin-db";
import { invalid } from "./repo-admin";

// Autolinks, webhooks and their deliveries, deploy keys, and pre-receive hook enforcement.

type Hook = Schemas["hook"];

const URL_RE = /^https?:\/\/[^\s/$.?#].[^\s]*$/;
const KEY_RE =
  /^(ssh-(ed25519|rsa|dss)|ecdsa-sha2-nistp(256|384|521)|sk-(ssh-ed25519|ecdsa-sha2-nistp256)@openssh\.com) [A-Za-z0-9+/=]{20,}( .*)?$/;

// Each delivery to a receiver whose URL names "fail" answers 500; others answer 200. The
// delivery appears in_flight for a moment, so the list shows the new row arrive.
function deliver(
  repo: Parameters<typeof delivery>[0],
  h: Hook,
  rec: ReturnType<typeof admin.state>,
  event: string,
  action: string | null,
  redelivery = false,
) {
  const url = h.config.url ?? "";
  const status = /fail|down/.test(url) ? 500 : 200;
  const d = delivery(repo, h, event, action, new Date().toISOString(), status, redelivery);
  const list = rec!.admin.deliveries.get(h.id) ?? [];
  rec!.admin.deliveries.set(h.id, [d, ...list]);
  h.last_response =
    status === 200
      ? { code: 200, status: "active", message: "OK" }
      : { code: 500, status: "failed", message: "Invalid HTTP Response: 500" };
  return d;
}

function hookProblem(
  config: Schemas["webhook-config"] | undefined | null,
  events: string[] | undefined,
) {
  if (config?.url !== undefined && !URL_RE.test(config.url)) {
    return invalid("Hook", "url", "invalid", "Payload URL is not a valid URL");
  }
  if (config?.content_type && !["json", "form"].includes(config.content_type)) {
    return invalid("Hook", "content_type", "invalid", "Content type must be json or form");
  }
  if (events && events.length === 0) {
    return invalid("Hook", "events", "invalid", "Choose at least one event");
  }
  return undefined;
}

export const handlers = [
  // ---- Autolinks ----

  http.get("/repos/{owner}/{repo}/autolinks", async ({ params, response }) => {
    const fault = await gate("admin-repo-autolinks");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    return rec ? response(200).json(rec.autolinks) : response.untyped(notFound());
  }),

  http.post("/repos/{owner}/{repo}/autolinks", async ({ params, request, response }) => {
    const fault = await gate("admin-repo-autolink-save");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    if (!rec) return response.untyped(notFound());
    const body = await request.json();
    const prefix = body.key_prefix.trim();
    if (!prefix) return response.untyped(unprocessable("KeyLink", "key_prefix", "missing_field"));
    if (rec.autolinks.some((a) => a.key_prefix.toLowerCase() === prefix.toLowerCase())) {
      return response.untyped(
        invalid("KeyLink", "key_prefix", "already_exists", "Key prefix has already been taken"),
      );
    }
    if (
      !body.url_template.includes("<num>") ||
      !URL_RE.test(body.url_template.replace("<num>", "1"))
    ) {
      return response.untyped(
        invalid(
          "KeyLink",
          "url_template",
          "invalid",
          "URL template must be a URL that contains <num>",
        ),
      );
    }
    const created = {
      id: id(),
      key_prefix: prefix,
      url_template: body.url_template,
      is_alphanumeric: body.is_alphanumeric ?? true,
      updated_at: new Date().toISOString(),
    };
    rec.autolinks.push(created);
    return response(201).json(created);
  }),

  http.get("/repos/{owner}/{repo}/autolinks/{autolink_id}", async ({ params, response }) => {
    const fault = await gate("admin-repo-autolinks");
    if (fault) return response.untyped(fault);
    const a = admin
      .state(params.owner, params.repo)
      ?.admin.autolinks.find((x) => x.id === Number(params.autolink_id));
    return a ? response(200).json(a) : response.untyped(notFound());
  }),

  http.delete("/repos/{owner}/{repo}/autolinks/{autolink_id}", async ({ params, response }) => {
    const fault = await gate("admin-repo-autolink-delete");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    if (!rec?.autolinks.some((a) => a.id === Number(params.autolink_id))) {
      return response.untyped(notFound());
    }
    rec.autolinks = rec.autolinks.filter((a) => a.id !== Number(params.autolink_id));
    return response(204).empty();
  }),

  // ---- Webhooks ----

  http.get("/repos/{owner}/{repo}/hooks", async ({ params, query, response }) => {
    const fault = await gate("admin-repo-hooks");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    return rec ? response(200).json(page(rec.hooks, query)) : response.untyped(notFound());
  }),

  http.post("/repos/{owner}/{repo}/hooks", async ({ params, request, response }) => {
    const fault = await gate("admin-repo-hook-save");
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    if (!found) return response.untyped(notFound());
    const body = (await request.json()) ?? {};
    const config = body.config ?? {};
    if (!config.url) return response.untyped(unprocessable("Hook", "url", "missing_field"));
    const problem = hookProblem(config, body.events ?? ["push"]);
    if (problem) return response.untyped(problem);
    if (found.admin.hooks.some((h) => h.config.url === config.url)) {
      return response.untyped(
        invalid("Hook", "url", "already_exists", "Hook already exists on this repository"),
      );
    }
    const n = id();
    const base = `${found.repo.url}/hooks/${n}`;
    const now = new Date().toISOString();
    const created: Hook = {
      type: "Repository",
      id: n,
      name: "web",
      active: body.active ?? true,
      events: body.events ?? ["push"],
      config: {
        url: config.url,
        content_type: config.content_type ?? "form",
        insecure_ssl: String(config.insecure_ssl ?? "0"),
        ...(config.secret ? { secret: "********" } : {}),
      },
      created_at: now,
      updated_at: now,
      url: base,
      test_url: `${base}/test`,
      ping_url: `${base}/pings`,
      deliveries_url: `${base}/deliveries`,
      last_response: { code: null, status: "unused", message: null },
    };
    found.admin.hooks.push(created);
    // GHES pings a new hook at once.
    deliver(found.repo, created, found, "ping", null);
    return response(201).json(created);
  }),

  http.get("/repos/{owner}/{repo}/hooks/{hook_id}", async ({ params, response }) => {
    const fault = await gate("admin-repo-hook");
    if (fault) return response.untyped(fault);
    const h = admin
      .state(params.owner, params.repo)
      ?.admin.hooks.find((x) => x.id === Number(params.hook_id));
    return h ? response(200).json(h) : response.untyped(notFound());
  }),

  http.patch("/repos/{owner}/{repo}/hooks/{hook_id}", async ({ params, request, response }) => {
    const fault = await gate("admin-repo-hook-save");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    const h = rec?.hooks.find((x) => x.id === Number(params.hook_id));
    if (!rec || !h) return response.untyped(notFound());
    const body = await request.json();
    const problem = hookProblem(body.config, body.events);
    if (problem) return response.untyped(problem);
    if (body.config) {
      const { secret, ...config } = body.config;
      h.config = {
        ...h.config,
        ...config,
        insecure_ssl: String(config.insecure_ssl ?? h.config.insecure_ssl ?? "0"),
        ...(secret ? { secret: "********" } : secret === "" ? { secret: undefined } : {}),
      };
    }
    if (body.events) h.events = body.events;
    if (body.add_events) h.events = [...new Set([...h.events, ...body.add_events])];
    if (body.remove_events) h.events = h.events.filter((e) => !body.remove_events!.includes(e));
    if (body.active !== undefined) h.active = body.active;
    h.updated_at = new Date().toISOString();
    return response(200).json(h);
  }),

  http.delete("/repos/{owner}/{repo}/hooks/{hook_id}", async ({ params, response }) => {
    const fault = await gate("admin-repo-hook-delete");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    if (!rec?.hooks.some((h) => h.id === Number(params.hook_id)))
      return response.untyped(notFound());
    rec.hooks = rec.hooks.filter((h) => h.id !== Number(params.hook_id));
    rec.deliveries.delete(Number(params.hook_id));
    return response(204).empty();
  }),

  http.get("/repos/{owner}/{repo}/hooks/{hook_id}/config", async ({ params, response }) => {
    const fault = await gate("admin-repo-hook");
    if (fault) return response.untyped(fault);
    const h = admin
      .state(params.owner, params.repo)
      ?.admin.hooks.find((x) => x.id === Number(params.hook_id));
    return h ? response(200).json(h.config) : response.untyped(notFound());
  }),

  http.get(
    "/repos/{owner}/{repo}/hooks/{hook_id}/deliveries",
    async ({ params, query, response }) => {
      const fault = await gate("admin-repo-deliveries");
      if (fault) return response.untyped(fault);
      const list = admin
        .state(params.owner, params.repo)
        ?.admin.deliveries.get(Number(params.hook_id));
      if (!list) return response.untyped(notFound());
      const status = query.get("status");
      const items = list
        .filter(
          (d) =>
            !status ||
            (status === "success"
              ? d.status_code < 400 && d.status_code > 0
              : d.status_code >= 400 || d.status_code === 0),
        )
        .map(
          ({ request, response: res, url, ...item }) => (void request, void res, void url, item),
        );
      const perPage = Math.min(100, Number(query.get("per_page")) || 30);
      return response(200).json(items.slice(0, perPage));
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/hooks/{hook_id}/deliveries/{delivery_id}",
    async ({ params, response }) => {
      const fault = await gate("admin-repo-delivery");
      if (fault) return response.untyped(fault);
      const d = admin
        .state(params.owner, params.repo)
        ?.admin.deliveries.get(Number(params.hook_id))
        ?.find((x) => x.id === Number(params.delivery_id));
      return d ? response(200).json(d) : response.untyped(notFound());
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/hooks/{hook_id}/deliveries/{delivery_id}/attempts",
    async ({ params, response }) => {
      const fault = await gate("admin-repo-redeliver");
      if (fault) return response.untyped(fault);
      const found = admin.state(params.owner, params.repo);
      const h = found?.admin.hooks.find((x) => x.id === Number(params.hook_id));
      const d = found?.admin.deliveries
        .get(Number(params.hook_id))
        ?.find((x) => x.id === Number(params.delivery_id));
      if (!found || !h || !d) return response.untyped(notFound());
      if (!h.active) {
        return response.untyped(
          invalid("Hook", "active", "invalid", "Hook is inactive, so it cannot redeliver"),
        );
      }
      deliver(found.repo, h, found, d.event, d.action, true);
      return response(202).json({});
    },
  ),

  http.post("/repos/{owner}/{repo}/hooks/{hook_id}/pings", async ({ params, response }) => {
    const fault = await gate("admin-repo-hook-ping");
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    const h = found?.admin.hooks.find((x) => x.id === Number(params.hook_id));
    if (!found || !h) return response.untyped(notFound());
    deliver(found.repo, h, found, "ping", null);
    return response(204).empty();
  }),

  // GHES sends the latest push to the hook if it subscribes to push; otherwise nothing.
  http.post("/repos/{owner}/{repo}/hooks/{hook_id}/tests", async ({ params, response }) => {
    const fault = await gate("admin-repo-hook-test");
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    const h = found?.admin.hooks.find((x) => x.id === Number(params.hook_id));
    if (!found || !h) return response.untyped(notFound());
    if (h.events.includes("push") || h.events.includes("*"))
      deliver(found.repo, h, found, "push", null);
    return response(204).empty();
  }),

  // ---- Deploy keys ----

  http.get("/repos/{owner}/{repo}/keys", async ({ params, query, response }) => {
    const fault = await gate("admin-repo-keys");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    return rec ? response(200).json(page(rec.keys, query)) : response.untyped(notFound());
  }),

  http.post("/repos/{owner}/{repo}/keys", async ({ params, request, response }) => {
    const fault = await gate("admin-repo-key-add");
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    if (!found) return response.untyped(notFound());
    const body = await request.json();
    const key = body.key.trim();
    if (!key) return response.untyped(unprocessable("PublicKey", "key", "missing_field"));
    if (!KEY_RE.test(key)) {
      return response.untyped(
        invalid(
          "PublicKey",
          "key",
          "custom",
          "key is invalid. You must supply a key in OpenSSH public key format",
        ),
      );
    }
    // A key is unique across the instance (design/SHARED.md, "Surfaces, tiers, and oracles", git-ssh).
    if (admin.keyInUse(key)) {
      return response.untyped(invalid("PublicKey", "key", "custom", "key is already in use"));
    }
    const created = deployKey(
      found.repo,
      body.title?.trim() || key.split(" ")[2] || "deploy key",
      key,
      body.read_only ?? true,
      new Date().toISOString(),
      null,
    );
    found.admin.keys.push(created);
    return response(201).json(created);
  }),

  http.get("/repos/{owner}/{repo}/keys/{key_id}", async ({ params, response }) => {
    const fault = await gate("admin-repo-keys");
    if (fault) return response.untyped(fault);
    const k = admin
      .state(params.owner, params.repo)
      ?.admin.keys.find((x) => x.id === Number(params.key_id));
    return k ? response(200).json(k) : response.untyped(notFound());
  }),

  http.delete("/repos/{owner}/{repo}/keys/{key_id}", async ({ params, response }) => {
    const fault = await gate("admin-repo-key-delete");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    if (rec) rec.keys = rec.keys.filter((k) => k.id !== Number(params.key_id));
    return response(204).empty();
  }),

  // ---- Pre-receive hooks ----

  http.get("/repos/{owner}/{repo}/pre-receive-hooks", async ({ params, response }) => {
    const fault = await gate("admin-repo-pre-receive");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    if (!rec) return response.untyped(notFound());
    return response(200).json(
      rec.preReceive.map(
        ({ default_enforcement, configurable, ...h }) => (
          void default_enforcement,
          void configurable,
          h
        ),
      ),
    );
  }),

  http.get(
    "/repos/{owner}/{repo}/pre-receive-hooks/{pre_receive_hook_id}",
    async ({ params, response }) => {
      const fault = await gate("admin-repo-pre-receive");
      if (fault) return response.untyped(fault);
      const h = admin
        .state(params.owner, params.repo)
        ?.admin.preReceive.find((x) => x.id === Number(params.pre_receive_hook_id));
      if (!h) return response.untyped(notFound());
      const { default_enforcement, configurable, ...rest } = h;
      void default_enforcement;
      void configurable;
      return response(200).json(rest);
    },
  ),

  http.patch(
    "/repos/{owner}/{repo}/pre-receive-hooks/{pre_receive_hook_id}",
    async ({ params, request, response }) => {
      const fault = await gate("admin-repo-pre-receive-save");
      if (fault) return response.untyped(fault);
      const h = admin
        .state(params.owner, params.repo)
        ?.admin.preReceive.find((x) => x.id === Number(params.pre_receive_hook_id));
      if (!h) return response.untyped(notFound());
      if (!h.configurable) {
        return response.untyped(
          invalid(
            "PreReceiveHook",
            "enforcement",
            "invalid",
            `${h.name} is enforced for the whole instance and cannot be changed here`,
          ),
        );
      }
      const { enforcement } = (await request.json()) ?? {};
      if (enforcement) h.enforcement = enforcement;
      const { default_enforcement, configurable, ...rest } = h;
      void default_enforcement;
      void configurable;
      return response(200).json(rest);
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/pre-receive-hooks/{pre_receive_hook_id}",
    async ({ params, response }) => {
      const fault = await gate("admin-repo-pre-receive-save");
      if (fault) return response.untyped(fault);
      const h = admin
        .state(params.owner, params.repo)
        ?.admin.preReceive.find((x) => x.id === Number(params.pre_receive_hook_id));
      if (!h) return response.untyped(notFound());
      h.enforcement = h.default_enforcement;
      const { default_enforcement, configurable, ...rest } = h;
      void default_enforcement;
      void configurable;
      return response(200).json(rest);
    },
  ),
];
