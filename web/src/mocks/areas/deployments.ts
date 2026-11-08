import { createOpenApiHttp } from "openapi-msw";
import type { UiPaths } from "@/areas/releases/api";
import { gate, http, notFound, page, unprocessable } from "../http";
import { deploymentsDb, type EnvSettings } from "./deployments-db";

// Deployments and their statuses, environments with protection rules and branch policies,
// environment secrets and variables, and the private list of deployments awaiting review.

const uiHttp = createOpenApiHttp<UiPaths>({ baseUrl: "/_t/ui" });

const DOCS = "https://docs.github.com/enterprise-server@3.22/rest/deployments";
const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

function envOf(owner: string, repo: string, name: string) {
  const r = deploymentsDb.state(owner, repo);
  const e = r && deploymentsDb.environment(r, decodeURIComponent(name));
  return r && e ? { r, e } : undefined;
}

const touch = (e: EnvSettings) => void (e.updated_at = new Date().toISOString());

export const handlers = [
  // ---- Deployments ----

  http.get("/repos/{owner}/{repo}/deployments", async ({ params, query, response }) => {
    const fault = await gate("deployments-list");
    if (fault) return response.untyped(fault);
    const r = deploymentsDb.state(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const environment = query.get("environment");
    const ref = query.get("ref");
    const sha = query.get("sha");
    const task = query.get("task");
    const items = r.deployments
      .filter(
        (d) =>
          (!environment || d.environment === environment) &&
          (!ref || d.ref === ref) &&
          (!sha || d.sha === sha) &&
          (!task || d.task === task),
      )
      .sort((a, b) => b.created_at.localeCompare(a.created_at));
    return response(200).json(page(items, query), { headers: { Link: "" } });
  }),

  http.post("/repos/{owner}/{repo}/deployments", async ({ params, request, response }) => {
    const fault = await gate("deployments-create");
    if (fault) return response.untyped(fault);
    const r = deploymentsDb.state(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const body = await request.json();
    const ref = body.ref.trim();
    if (!ref) return response.untyped(unprocessable("Deployment", "ref", "missing_field"));
    if (!deploymentsDb.refExists(r, ref)) {
      return response(422).json({
        message: `No ref found for: ${ref}`,
        documentation_url: `${DOCS}/deployments#create-a-deployment`,
        errors: [{ resource: "Deployment", field: "ref", code: "invalid" }],
      });
    }
    const name = body.environment?.trim() || "production";
    const e = deploymentsDb.environment(r, name);
    if (!deploymentsDb.refAllowed(r, e, ref)) {
      return response(422).json({
        message: `Branch "${ref}" is not allowed to deploy to ${name} due to environment protection rules.`,
        documentation_url: `${DOCS}/deployments#create-a-deployment`,
        errors: [{ resource: "Deployment", field: "ref", code: "custom" }],
      });
    }
    return response(201).json(deploymentsDb.create(r, { ...body, ref, environment: name }));
  }),

  http.get("/repos/{owner}/{repo}/deployments/{deployment_id}", async ({ params, response }) => {
    const fault = await gate("deployments-get");
    if (fault) return response.untyped(fault);
    const d = deploymentsDb
      .state(params.owner, params.repo)
      ?.deployments.find((x) => x.id === Number(params.deployment_id));
    return d ? response(200).json(d) : response.untyped(notFound());
  }),

  http.delete("/repos/{owner}/{repo}/deployments/{deployment_id}", async ({ params, response }) => {
    const fault = await gate("deployments-delete");
    if (fault) return response.untyped(fault);
    const r = deploymentsDb.state(params.owner, params.repo);
    const id = Number(params.deployment_id);
    if (!r || !r.deployments.some((d) => d.id === id)) return response.untyped(notFound());
    // GHES deletes an active deployment only when it is the repository's only one.
    const state = deploymentsDb.latestStatus(r, id)?.state;
    if (r.deployments.length > 1 && state !== "inactive") {
      return response(422).json({
        message:
          "We cannot delete an active deployment unless it is the only deployment in a given environment.",
        documentation_url: `${DOCS}/deployments#delete-a-deployment`,
      });
    }
    deploymentsDb.remove(r, id);
    return response(204).empty();
  }),

  http.get(
    "/repos/{owner}/{repo}/deployments/{deployment_id}/statuses",
    async ({ params, query, response }) => {
      const fault = await gate("deployments-statuses");
      if (fault) return response.untyped(fault);
      const r = deploymentsDb.state(params.owner, params.repo);
      const id = Number(params.deployment_id);
      if (!r || !r.deployments.some((d) => d.id === id)) return response.untyped(notFound());
      return response(200).json(page(deploymentsDb.statuses(r, id), query), {
        headers: { Link: "" },
      });
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/deployments/{deployment_id}/statuses/{status_id}",
    async ({ params, response }) => {
      const fault = await gate("deployments-statuses");
      if (fault) return response.untyped(fault);
      const r = deploymentsDb.state(params.owner, params.repo);
      const st = r?.statuses
        .get(Number(params.deployment_id))
        ?.find((s) => s.id === Number(params.status_id));
      return st ? response(200).json(st) : response.untyped(notFound());
    },
  ),

  // A reviewer's verdict is a status: in_progress approves a held deployment and runs it,
  // failure rejects it. Any other state is written as given.
  http.post(
    "/repos/{owner}/{repo}/deployments/{deployment_id}/statuses",
    async ({ params, request, response }) => {
      const fault = await gate("deployments-review");
      if (fault) return response.untyped(fault);
      const r = deploymentsDb.state(params.owner, params.repo);
      const d = r?.deployments.find((x) => x.id === Number(params.deployment_id));
      if (!r || !d) return response.untyped(notFound());
      const body = await request.json();
      const held = deploymentsDb.pendingFor(r, d.id);
      if (held && !held.current_user_can_approve && body.state !== "inactive") {
        return response(422).json({
          message: "You are not allowed to review this deployment.",
          documentation_url: `${DOCS}/statuses#create-a-deployment-status`,
          errors: [{ resource: "DeploymentStatus", field: "state", code: "custom" }],
        });
      }
      if (held && body.state === "in_progress") {
        return response(201).json(deploymentsDb.approve(r, d, body.description ?? ""), {
          headers: { Location: d.statuses_url },
        });
      }
      const st = deploymentsDb.addStatus(r, d, body.state, {
        description: body.description,
        url: body.environment_url,
        autoInactive: body.auto_inactive,
      });
      return response(201).json(st, { headers: { Location: st.url } });
    },
  ),

  uiHttp.get("/repos/{owner}/{repo}/deployments/pending", async ({ params, response }) => {
    const fault = await gate("deployments-pending");
    if (fault) return response.untyped(fault);
    const r = deploymentsDb.state(params.owner, params.repo);
    return r ? response(200).json(r.pending) : response.untyped(notFound());
  }),

  // ---- Environments ----

  http.get("/repos/{owner}/{repo}/environments", async ({ params, query, response }) => {
    const fault = await gate("environments");
    if (fault) return response.untyped(fault);
    const r = deploymentsDb.state(params.owner, params.repo);
    if (!r) return response.untyped(notFound());
    const all = [...r.environments].sort((a, b) => a.name.localeCompare(b.name));
    return response(200).json({
      total_count: all.length,
      environments: page(all, query).map((e) => deploymentsDb.serve(r, e)),
    });
  }),

  http.get(
    "/repos/{owner}/{repo}/environments/{environment_name}",
    async ({ params, response }) => {
      const fault = await gate("environments-get");
      if (fault) return response.untyped(fault);
      const found = envOf(params.owner, params.repo, params.environment_name);
      return found
        ? response(200).json(deploymentsDb.serve(found.r, found.e))
        : response.untyped(notFound());
    },
  ),

  http.put(
    "/repos/{owner}/{repo}/environments/{environment_name}",
    async ({ params, request, response }) => {
      const fault = await gate("environments-update");
      if (fault) return response.untyped(fault);
      const r = deploymentsDb.state(params.owner, params.repo);
      if (!r) return response.untyped(notFound());
      const name = decodeURIComponent(params.environment_name).trim();
      const invalid = (message: string) =>
        response(422).json({ message, documentation_url: `${DOCS}/environments` });
      if (!name || name.length > 255 || /[/:"<>|?*\\,']/.test(name)) {
        return invalid(
          "Name is invalid. Environment names cannot contain /, :, \", <, >, |, ?, *, \\, ' or ,",
        );
      }
      const body = (await request.json()) ?? {};
      const wait = body.wait_timer;
      if (wait !== undefined && (!Number.isInteger(wait) || wait < 0 || wait > 43200)) {
        return invalid("Wait timer must be an integer from 0 to 43200 minutes.");
      }
      const reviewers = [];
      for (const ref of body.reviewers ?? []) {
        const found = ref.id && ref.type ? deploymentsDb.reviewerById(ref.id, ref.type) : undefined;
        if (!found)
          return invalid(`Reviewer ${ref.type ?? ""} ${ref.id ?? ""} could not be found.`);
        reviewers.push(found);
      }
      if (reviewers.length > 6) return invalid("An environment can have at most 6 reviewers.");
      const policy = body.deployment_branch_policy;
      if (policy && policy.protected_branches === policy.custom_branch_policies) {
        return invalid(
          "Exactly one of protected_branches and custom_branch_policies must be true.",
        );
      }
      const e = deploymentsDb.upsertEnvironment(r, name);
      if (wait !== undefined) e.wait_timer = wait;
      if (body.prevent_self_review !== undefined) e.prevent_self_review = body.prevent_self_review;
      if (body.reviewers !== undefined) e.reviewers = reviewers;
      if (policy !== undefined) e.branch_policy = policy;
      touch(e);
      return response(200).json(deploymentsDb.serve(r, e));
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/environments/{environment_name}",
    async ({ params, response }) => {
      const fault = await gate("environments-delete");
      if (fault) return response.untyped(fault);
      const found = envOf(params.owner, params.repo, params.environment_name);
      if (!found) return response.untyped(notFound());
      deploymentsDb.deleteEnvironment(found.r, found.e.name);
      return response(204).empty();
    },
  ),

  // ---- Deployment branch policies ----

  http.get(
    "/repos/{owner}/{repo}/environments/{environment_name}/deployment-branch-policies",
    async ({ params, query, response }) => {
      const fault = await gate("environments-policies");
      if (fault) return response.untyped(fault);
      const found = envOf(params.owner, params.repo, params.environment_name);
      if (!found) return response.untyped(notFound());
      return response(200).json({
        total_count: found.e.policies.length,
        branch_policies: page(found.e.policies, query),
      });
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/environments/{environment_name}/deployment-branch-policies",
    async ({ params, request, response }) => {
      const fault = await gate("environments-policy-create");
      if (fault) return response.untyped(fault);
      const found = envOf(params.owner, params.repo, params.environment_name);
      // GHES answers 404 when the environment does not use custom policies.
      if (!found || !found.e.branch_policy?.custom_branch_policies) {
        return response.untyped(notFound());
      }
      const body = await request.json();
      const name = body.name.trim();
      const type = body.type ?? "branch";
      if (!name)
        return response.untyped(unprocessable("DeploymentBranchPolicy", "name", "missing_field"));
      // GHES answers 303 to the existing policy; the UI shows it as a field error.
      if (found.e.policies.some((p) => p.name === name && p.type === type)) {
        return response.untyped(unprocessable("DeploymentBranchPolicy", "name", "already_exists"));
      }
      const created = deploymentsDb.newPolicy(name, type);
      found.e.policies.push(created);
      touch(found.e);
      return response(200).json(created);
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/environments/{environment_name}/deployment-branch-policies/{branch_policy_id}",
    async ({ params, response }) => {
      const fault = await gate("environments-policies");
      if (fault) return response.untyped(fault);
      const p = envOf(params.owner, params.repo, params.environment_name)?.e.policies.find(
        (x) => x.id === Number(params.branch_policy_id),
      );
      return p ? response(200).json(p) : response.untyped(notFound());
    },
  ),

  http.put(
    "/repos/{owner}/{repo}/environments/{environment_name}/deployment-branch-policies/{branch_policy_id}",
    async ({ params, request, response }) => {
      const fault = await gate("environments-policy-update");
      if (fault) return response.untyped(fault);
      const found = envOf(params.owner, params.repo, params.environment_name);
      const p = found?.e.policies.find((x) => x.id === Number(params.branch_policy_id));
      if (!found || !p) return response.untyped(notFound());
      const name = (await request.json()).name.trim();
      if (!name)
        return response.untyped(unprocessable("DeploymentBranchPolicy", "name", "missing_field"));
      if (found.e.policies.some((x) => x !== p && x.name === name && x.type === p.type)) {
        return response.untyped(unprocessable("DeploymentBranchPolicy", "name", "already_exists"));
      }
      p.name = name;
      touch(found.e);
      return response(200).json(p);
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/environments/{environment_name}/deployment-branch-policies/{branch_policy_id}",
    async ({ params, response }) => {
      const fault = await gate("environments-policy-delete");
      if (fault) return response.untyped(fault);
      const found = envOf(params.owner, params.repo, params.environment_name);
      if (!found) return response.untyped(notFound());
      found.e.policies = found.e.policies.filter((x) => x.id !== Number(params.branch_policy_id));
      touch(found.e);
      return response(204).empty();
    },
  ),

  // ---- Environment secrets and variables ----

  http.get(
    "/repos/{owner}/{repo}/environments/{environment_name}/secrets",
    async ({ params, query, response }) => {
      const fault = await gate("environments-secrets");
      if (fault) return response.untyped(fault);
      const found = envOf(params.owner, params.repo, params.environment_name);
      if (!found) return response.untyped(notFound());
      return response(200).json(
        { total_count: found.e.secrets.length, secrets: page(found.e.secrets, query) },
        { headers: { Link: "" } },
      );
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/environments/{environment_name}/secrets/public-key",
    async ({ params, response }) => {
      const fault = await gate("environments-secrets");
      if (fault) return response.untyped(fault);
      const found = envOf(params.owner, params.repo, params.environment_name);
      return found
        ? response(200).json(deploymentsDb.secretKey(found.r, found.e))
        : response.untyped(notFound());
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/environments/{environment_name}/secrets/{secret_name}",
    async ({ params, response }) => {
      const fault = await gate("environments-secrets");
      if (fault) return response.untyped(fault);
      const s = envOf(params.owner, params.repo, params.environment_name)?.e.secrets.find(
        (x) => x.name === params.secret_name.toUpperCase(),
      );
      return s ? response(200).json(s) : response.untyped(notFound());
    },
  ),

  // The value arrives sealed to the environment's public key, and the mock never opens it.
  http.put(
    "/repos/{owner}/{repo}/environments/{environment_name}/secrets/{secret_name}",
    async ({ params, request, response }) => {
      const fault = await gate("environments-secret-put");
      if (fault) return response.untyped(fault);
      const found = envOf(params.owner, params.repo, params.environment_name);
      if (!found) return response.untyped(notFound());
      const name = params.secret_name.toUpperCase();
      if (!NAME.test(name) || name.startsWith("GITHUB_")) {
        return response.untyped(unprocessable("Secret", "name", "invalid"));
      }
      const body = await request.json();
      if (body.key_id !== deploymentsDb.secretKey(found.r, found.e).key_id) {
        return response.untyped(unprocessable("Secret", "key_id", "invalid"));
      }
      if (!body.encrypted_value) {
        return response.untyped(unprocessable("Secret", "encrypted_value", "missing_field"));
      }
      const now = new Date().toISOString();
      const existing = found.e.secrets.find((x) => x.name === name);
      if (existing) {
        existing.updated_at = now;
        return response(204).empty();
      }
      found.e.secrets.push({ name, created_at: now, updated_at: now });
      return response(201).json({});
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/environments/{environment_name}/secrets/{secret_name}",
    async ({ params, response }) => {
      const fault = await gate("environments-secret-delete");
      if (fault) return response.untyped(fault);
      const found = envOf(params.owner, params.repo, params.environment_name);
      if (!found) return response.untyped(notFound());
      found.e.secrets = found.e.secrets.filter((x) => x.name !== params.secret_name.toUpperCase());
      return response(204).empty();
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/environments/{environment_name}/variables",
    async ({ params, query, response }) => {
      const fault = await gate("environments-variables");
      if (fault) return response.untyped(fault);
      const found = envOf(params.owner, params.repo, params.environment_name);
      if (!found) return response.untyped(notFound());
      return response(200).json(
        { total_count: found.e.variables.length, variables: page(found.e.variables, query) },
        { headers: { Link: "" } },
      );
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/environments/{environment_name}/variables",
    async ({ params, request, response }) => {
      const fault = await gate("environments-variable-create");
      if (fault) return response.untyped(fault);
      const found = envOf(params.owner, params.repo, params.environment_name);
      if (!found) return response.untyped(notFound());
      const body = await request.json();
      const name = body.name.trim().toUpperCase();
      if (!NAME.test(name) || name.startsWith("GITHUB_")) {
        return response.untyped(unprocessable("Variable", "name", "invalid"));
      }
      if (found.e.variables.some((v) => v.name === name)) {
        return response.untyped(unprocessable("Variable", "name", "already_exists"));
      }
      const now = new Date().toISOString();
      found.e.variables.push({ name, value: body.value, created_at: now, updated_at: now });
      return response(201).json({});
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/environments/{environment_name}/variables/{name}",
    async ({ params, response }) => {
      const fault = await gate("environments-variables");
      if (fault) return response.untyped(fault);
      const v = envOf(params.owner, params.repo, params.environment_name)?.e.variables.find(
        (x) => x.name === params.name.toUpperCase(),
      );
      return v ? response(200).json(v) : response.untyped(notFound());
    },
  ),

  http.patch(
    "/repos/{owner}/{repo}/environments/{environment_name}/variables/{name}",
    async ({ params, request, response }) => {
      const fault = await gate("environments-variable-update");
      if (fault) return response.untyped(fault);
      const found = envOf(params.owner, params.repo, params.environment_name);
      const v = found?.e.variables.find((x) => x.name === params.name.toUpperCase());
      if (!found || !v) return response.untyped(notFound());
      const body = await request.json();
      if (body.name !== undefined) {
        const name = body.name.trim().toUpperCase();
        if (!NAME.test(name) || name.startsWith("GITHUB_")) {
          return response.untyped(unprocessable("Variable", "name", "invalid"));
        }
        if (found.e.variables.some((x) => x !== v && x.name === name)) {
          return response.untyped(unprocessable("Variable", "name", "already_exists"));
        }
        v.name = name;
      }
      if (body.value !== undefined) v.value = body.value;
      v.updated_at = new Date().toISOString();
      return response(204).empty();
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/environments/{environment_name}/variables/{name}",
    async ({ params, response }) => {
      const fault = await gate("environments-variable-delete");
      if (fault) return response.untyped(fault);
      const found = envOf(params.owner, params.repo, params.environment_name);
      if (!found) return response.untyped(notFound());
      found.e.variables = found.e.variables.filter((x) => x.name !== params.name.toUpperCase());
      return response(204).empty();
    },
  ),
];
