import { HttpResponse } from "msw";
import type { Schemas } from "@/api/client";
import { gate, http, notFound, page, unprocessable } from "../http";
import { me, users } from "../world";
import { invalid } from "./orgs";
import {
  deliver,
  findOrg,
  log,
  makeHook,
  makeRuleset,
  orgDb,
  recordRulesetVersion,
  teamById,
} from "./orgs-db";

// The organization's webhooks and their deliveries, its rulesets and their history, and
// the pre-receive hooks the site admin lets it enforce.

type Hook = Schemas["org-hook"];
type Ruleset = Schemas["repository-ruleset"];

const URL_RE = /^https?:\/\/[^\s/$.?#].[^\s]*$/;
const EVENTS = new Set([
  "*",
  "push",
  "pull_request",
  "pull_request_review",
  "issues",
  "issue_comment",
  "create",
  "delete",
  "release",
  "check_run",
  "check_suite",
  "workflow_run",
  "deployment",
  "deployment_status",
  "fork",
  "member",
  "membership",
  "organization",
  "repository",
  "team",
  "team_add",
  "ping",
]);
const viewer = me.login;

function findHook(org: string, id: string) {
  return findOrg(org) ? orgDb.hooks.find((h) => h.id === Number(id)) : undefined;
}

function hookProblem(
  config: { url?: string; content_type?: string } | undefined,
  events?: string[],
) {
  if (config?.url !== undefined && !URL_RE.test(config.url)) {
    return invalid("Hook", "url", "invalid", "Payload URL is not a valid URL.");
  }
  if (config?.content_type && !["json", "form"].includes(config.content_type)) {
    return invalid("Hook", "content_type", "invalid", "Content type must be json or form.");
  }
  if (events && events.length === 0) {
    return invalid("Hook", "events", "invalid", "Choose at least one event.");
  }
  const unknown = events?.find((e) => !EVENTS.has(e));
  if (unknown) return invalid("Hook", "events", "invalid", `${unknown} is not a webhook event.`);
  return undefined;
}

type ConfigIn = {
  url?: string;
  content_type?: string;
  secret?: string;
  insecure_ssl?: string | number;
};

function applyConfig(h: Hook, config: ConfigIn) {
  const { secret, ...rest } = config;
  h.config = {
    ...h.config,
    ...rest,
    insecure_ssl: String(rest.insecure_ssl ?? h.config.insecure_ssl ?? "0"),
    ...(secret ? { secret: "********" } : {}),
  };
  if (secret === "") delete h.config.secret;
  h.updated_at = new Date().toISOString();
}

// A delivery list row drops the request, response, and URL that the detail carries.
const item = ({ request, response, url, ...rest }: Schemas["hook-delivery"]) => (
  void request,
  void response,
  void url,
  rest
);

// A ruleset's name must be set, and a bypass team must exist, as in GHES.
function rulesetProblem(body: {
  name?: string;
  enforcement?: string;
  bypass_actors?: Schemas["repository-ruleset-bypass-actor"][];
  conditions?: unknown;
}) {
  if (body.name !== undefined && !body.name.trim()) {
    return unprocessable("Ruleset", "name", "missing_field");
  }
  for (const a of body.bypass_actors ?? []) {
    if (a.actor_type === "Team" && !teamById(a.actor_id ?? -1)) {
      return invalid("Ruleset", "bypass_actors", "invalid", "A bypass team does not exist.");
    }
  }
  return undefined;
}

function nameTaken(name: string, except?: number) {
  return orgDb.rulesets.some((r) => r.id !== except && r.name.toLowerCase() === name.toLowerCase());
}

export const handlers = [
  // ---- Webhooks ----

  http.get("/orgs/{org}/hooks", async ({ params, query, response }) => {
    const fault = await gate("orgs-hooks");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    return response(200).json(page(orgDb.hooks, query));
  }),

  http.post("/orgs/{org}/hooks", async ({ params, request, response }) => {
    const fault = await gate("orgs-hook-save");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const body = await request.json();
    if (body.name !== "web") {
      return response.untyped(
        invalid("Hook", "name", "invalid", "Organization hooks must be named web."),
      );
    }
    const problem = hookProblem(body.config, body.events);
    if (problem) return response.untyped(problem);
    if (orgDb.hooks.some((h) => h.config.url === body.config.url)) {
      return response.untyped(
        invalid("Hook", "url", "already_exists", "A hook with this payload URL already exists."),
      );
    }
    const h = makeHook({
      url: body.config.url,
      content_type: body.config.content_type ?? "form",
      secret: body.config.secret,
      insecure_ssl: String(body.config.insecure_ssl ?? "0"),
      events: body.events ?? ["push"],
      active: body.active ?? true,
    });
    orgDb.hooks.push(h);
    // GHES pings a new hook at once; the receiver answers a moment later.
    deliver(h, "ping", null, new Date(), { pending: true });
    log("hook.create", viewer, { hook_id: h.id });
    return response(201).json(h);
  }),

  http.get("/orgs/{org}/hooks/{hook_id}", async ({ params, response }) => {
    const fault = await gate("orgs-hook");
    if (fault) return response.untyped(fault);
    const h = findHook(params.org, params.hook_id);
    return h ? response(200).json(h) : response.untyped(notFound());
  }),

  http.patch("/orgs/{org}/hooks/{hook_id}", async ({ params, request, response }) => {
    const fault = await gate("orgs-hook-save");
    if (fault) return response.untyped(fault);
    const h = findHook(params.org, params.hook_id);
    if (!h) return response.untyped(notFound());
    const body = (await request.json()) ?? {};
    const problem = hookProblem(body.config, body.events);
    if (problem) return response.untyped(problem);
    const url = body.config?.url;
    if (url && orgDb.hooks.some((o) => o.id !== h.id && o.config.url === url)) {
      return response.untyped(
        invalid("Hook", "url", "already_exists", "A hook with this payload URL already exists."),
      );
    }
    if (body.config) applyConfig(h, body.config);
    if (body.events) h.events = body.events;
    if (body.active !== undefined) h.active = body.active;
    h.updated_at = new Date().toISOString();
    log("hook.config_changed", viewer, { hook_id: h.id });
    return response(200).json(h);
  }),

  http.delete("/orgs/{org}/hooks/{hook_id}", async ({ params, response }) => {
    const fault = await gate("orgs-hook-delete");
    if (fault) return response.untyped(fault);
    const h = findHook(params.org, params.hook_id);
    if (!h) return response.untyped(notFound());
    orgDb.hooks.splice(orgDb.hooks.indexOf(h), 1);
    orgDb.deliveries.delete(h.id);
    log("hook.destroy", viewer, { hook_id: h.id });
    return response(204).empty();
  }),

  http.get("/orgs/{org}/hooks/{hook_id}/config", async ({ params, response }) => {
    const fault = await gate("orgs-hook");
    if (fault) return response.untyped(fault);
    const h = findHook(params.org, params.hook_id);
    return h ? response(200).json(h.config) : response.untyped(notFound());
  }),

  http.patch("/orgs/{org}/hooks/{hook_id}/config", async ({ params, request, response }) => {
    const fault = await gate("orgs-hook-save");
    if (fault) return response.untyped(fault);
    const h = findHook(params.org, params.hook_id);
    if (!h) return response.untyped(notFound());
    const body = (await request.json()) ?? {};
    const problem = hookProblem(body);
    if (problem) return response.untyped(problem);
    applyConfig(h, body);
    return response(200).json(h.config);
  }),

  http.get("/orgs/{org}/hooks/{hook_id}/deliveries", async ({ params, query, response }) => {
    const fault = await gate("orgs-hook-deliveries");
    if (fault) return response.untyped(fault);
    const h = findHook(params.org, params.hook_id);
    if (!h) return response.untyped(notFound());
    const status = query.get("status");
    const list = (orgDb.deliveries.get(h.id) ?? []).filter((d) =>
      status === "success"
        ? d.status_code >= 200 && d.status_code < 400
        : status === "failure"
          ? d.status_code >= 400
          : true,
    );
    const perPage = Math.min(100, Number(query.get("per_page")) || 30);
    return response(200).json(list.slice(0, perPage).map(item));
  }),

  http.get("/orgs/{org}/hooks/{hook_id}/deliveries/{delivery_id}", async ({ params, response }) => {
    const fault = await gate("orgs-hook-delivery");
    if (fault) return response.untyped(fault);
    const h = findHook(params.org, params.hook_id);
    const d = h && orgDb.deliveries.get(h.id)?.find((x) => x.id === Number(params.delivery_id));
    return d ? response(200).json(d) : response.untyped(notFound());
  }),

  http.post(
    "/orgs/{org}/hooks/{hook_id}/deliveries/{delivery_id}/attempts",
    async ({ params, response }) => {
      const fault = await gate("orgs-hook-redeliver");
      if (fault) return response.untyped(fault);
      const h = findHook(params.org, params.hook_id);
      const d = h && orgDb.deliveries.get(h.id)?.find((x) => x.id === Number(params.delivery_id));
      if (!h || !d) return response.untyped(notFound());
      if (!h.active) {
        return response.untyped(
          invalid("Hook", "active", "invalid", "This hook is inactive, so it cannot redeliver."),
        );
      }
      deliver(h, d.event, d.action, new Date(), { redelivery: true, pending: true });
      return response(202).json({});
    },
  ),

  http.post("/orgs/{org}/hooks/{hook_id}/pings", async ({ params, response }) => {
    const fault = await gate("orgs-hook-ping");
    if (fault) return response.untyped(fault);
    const h = findHook(params.org, params.hook_id);
    if (!h) return response.untyped(notFound());
    deliver(h, "ping", null, new Date(), { pending: true });
    return response(204).empty();
  }),

  // ---- Rulesets ----

  http.get("/orgs/{org}/rulesets", async ({ params, query, response }) => {
    const fault = await gate("orgs-rulesets");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const targets = query.get("targets")?.split(",").filter(Boolean);
    const list = orgDb.rulesets.filter((r) => !targets || targets.includes(r.target ?? "branch"));
    return response(200).json(page(list, query));
  }),

  http.post("/orgs/{org}/rulesets", async ({ params, request, response }) => {
    const fault = await gate("orgs-ruleset-save");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const body = await request.json();
    const name = body.name.trim();
    if (!name) return response.untyped(unprocessable("Ruleset", "name", "missing_field"));
    if (nameTaken(name))
      return response.untyped(unprocessable("Ruleset", "name", "already_exists"));
    const problem = rulesetProblem(body);
    if (problem) return response.untyped(problem);
    const r = makeRuleset({
      name,
      target: body.target ?? "branch",
      enforcement: body.enforcement,
      bypass_actors: body.bypass_actors ?? [],
      conditions: body.conditions ?? null,
      rules: body.rules ?? [],
    });
    orgDb.rulesets.push(r);
    recordRulesetVersion(r, users.mira.id);
    log("repository_ruleset.create", viewer, { name });
    return response(201).json(r);
  }),

  http.get("/orgs/{org}/rulesets/{ruleset_id}", async ({ params, response }) => {
    const fault = await gate("orgs-ruleset");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const r = orgDb.rulesets.find((x) => x.id === Number(params.ruleset_id));
    return r ? response(200).json(r) : response.untyped(notFound());
  }),

  http.put("/orgs/{org}/rulesets/{ruleset_id}", async ({ params, request, response }) => {
    const fault = await gate("orgs-ruleset-save");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const r = orgDb.rulesets.find((x) => x.id === Number(params.ruleset_id));
    if (!r) return response.untyped(notFound());
    const body = (await request.json()) ?? {};
    if (body.name !== undefined && nameTaken(body.name.trim(), r.id)) {
      return response.untyped(unprocessable("Ruleset", "name", "already_exists"));
    }
    const problem = rulesetProblem(body);
    if (problem) return response.untyped(problem);
    const next: Ruleset = {
      ...r,
      ...(body.name !== undefined && { name: body.name.trim() }),
      ...(body.target && { target: body.target }),
      ...(body.enforcement && { enforcement: body.enforcement }),
      ...(body.bypass_actors && { bypass_actors: body.bypass_actors }),
      ...(body.conditions && { conditions: body.conditions }),
      ...(body.rules && { rules: body.rules }),
      updated_at: new Date().toISOString(),
    };
    Object.assign(r, next);
    recordRulesetVersion(r, users.mira.id);
    log("repository_ruleset.update", viewer, { name: r.name });
    return response(200).json(r);
  }),

  http.delete("/orgs/{org}/rulesets/{ruleset_id}", async ({ params, response }) => {
    const fault = await gate("orgs-ruleset-delete");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const i = orgDb.rulesets.findIndex((x) => x.id === Number(params.ruleset_id));
    if (i < 0) return response.untyped(notFound());
    const [gone] = orgDb.rulesets.splice(i, 1);
    log("repository_ruleset.destroy", viewer, { name: gone.name });
    return response(204).empty();
  }),

  http.get("/orgs/{org}/rulesets/{ruleset_id}/history", async ({ params, query, response }) => {
    const fault = await gate("orgs-ruleset-history");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const h = orgDb.rulesetHistory.get(Number(params.ruleset_id));
    if (!h) return response.untyped(notFound());
    return response(200).json(
      page(
        h.map(({ state, ...v }) => (void state, v)),
        query,
      ),
    );
  }),

  // GHES 3.22's description types `state` as an empty object, but the server answers the
  // ruleset as it was at that version, which is what a history diff needs. Served untyped
  // for that one field; the version fields around it match ruleset-version.
  http.get(
    "/orgs/{org}/rulesets/{ruleset_id}/history/{version_id}",
    async ({ params, response }) => {
      const fault = await gate("orgs-ruleset-history");
      if (fault) return response.untyped(fault);
      if (!findOrg(params.org)) return response.untyped(notFound());
      const v = orgDb.rulesetHistory
        .get(Number(params.ruleset_id))
        ?.find((x) => x.version_id === Number(params.version_id));
      return v ? response.untyped(HttpResponse.json(v)) : response.untyped(notFound());
    },
  ),

  // ---- Pre-receive hooks ----

  http.get("/orgs/{org}/pre-receive-hooks", async ({ params, query, response }) => {
    const fault = await gate("orgs-pre-receive");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    return response(200).json(page(orgDb.preReceive, query));
  }),

  http.get("/orgs/{org}/pre-receive-hooks/{pre_receive_hook_id}", async ({ params, response }) => {
    const fault = await gate("orgs-pre-receive");
    if (fault) return response.untyped(fault);
    const h =
      findOrg(params.org) &&
      orgDb.preReceive.find((x) => x.id === Number(params.pre_receive_hook_id));
    return h ? response(200).json(h) : response.untyped(notFound());
  }),

  http.patch(
    "/orgs/{org}/pre-receive-hooks/{pre_receive_hook_id}",
    async ({ params, request, response }) => {
      const fault = await gate("orgs-pre-receive-save");
      if (fault) return response.untyped(fault);
      const h =
        findOrg(params.org) &&
        orgDb.preReceive.find((x) => x.id === Number(params.pre_receive_hook_id));
      if (!h) return response.untyped(notFound());
      const body = (await request.json()) ?? {};
      if (body.enforcement && !["enabled", "disabled", "testing"].includes(body.enforcement)) {
        return response.untyped(unprocessable("PreReceiveHook", "enforcement", "invalid"));
      }
      const locked = !orgDb.preReceiveDefaults.has(h.id!) || !siteAllows(h.id!);
      if (locked) {
        return response.untyped(unprocessable("PreReceiveHook", "enforcement", "not_allowed"));
      }
      if (body.enforcement) h.enforcement = body.enforcement;
      if (body.allow_downstream_configuration !== undefined) {
        h.allow_downstream_configuration = body.allow_downstream_configuration;
      }
      h.configuration_url = `${h.configuration_url!.replace(/\/admin\//, `/orgs/${orgDb.org.login}/`)}`;
      log("pre_receive_hook.enforcement", viewer, {
        name: h.name,
        data: { enforcement: h.enforcement },
      });
      return response(200).json(h);
    },
  ),

  http.delete(
    "/orgs/{org}/pre-receive-hooks/{pre_receive_hook_id}",
    async ({ params, response }) => {
      const fault = await gate("orgs-pre-receive-save");
      if (fault) return response.untyped(fault);
      const h =
        findOrg(params.org) &&
        orgDb.preReceive.find((x) => x.id === Number(params.pre_receive_hook_id));
      if (!h) return response.untyped(notFound());
      h.enforcement = orgDb.preReceiveDefaults.get(h.id!) ?? "disabled";
      h.allow_downstream_configuration = siteAllows(h.id!);
      h.configuration_url = h.configuration_url!.replace(`/orgs/${orgDb.org.login}/`, "/admin/");
      log("pre_receive_hook.enforcement", viewer, { name: h.name, data: { enforcement: "reset" } });
      return response(200).json(h);
    },
  ),
];

// Whether the site admin lets organizations change this hook's enforcement. block-secrets
// is enforced instance-wide.
function siteAllows(id: number) {
  return id !== 13;
}
