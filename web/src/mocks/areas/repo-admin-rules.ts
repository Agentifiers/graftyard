import { createOpenApiHttp } from "openapi-msw";
import type { Schemas } from "@/api/client";
import type { UiPaths } from "@/areas/repo-admin/private-api";
import { gate, http, notFound, page } from "../http";
import { users } from "../world";
import { admin, protectedBranch, protectionRecord } from "./repo-admin-db";
import { invalid } from "./repo-admin";

// Branch protection rules and repository rulesets, and the rule insights table.

const ui = createOpenApiHttp<UiPaths>({ baseUrl: "/_t/ui" });

type Ruleset = Schemas["repository-ruleset"];
type RulesetBody = {
  name?: string;
  target?: "branch" | "tag" | "push";
  enforcement?: Ruleset["enforcement"];
  bypass_actors?: Ruleset["bypass_actors"];
  conditions?: Schemas["repository-ruleset-conditions"];
  rules?: Schemas["repository-rule"][];
};

function rulesetProblem(body: RulesetBody, others: Ruleset[]) {
  if (body.name !== undefined && !body.name.trim()) {
    return invalid("Ruleset", "name", "missing_field", "Name can't be blank");
  }
  if (body.name && others.some((r) => r.name.toLowerCase() === body.name!.trim().toLowerCase())) {
    return invalid("Ruleset", "name", "already_exists", "Name must be unique");
  }
  if (body.conditions && !(body.conditions.ref_name?.include ?? []).length) {
    return invalid("Ruleset", "conditions", "invalid", "Add at least one target ref to include");
  }
  for (const rule of body.rules ?? []) {
    if (
      (rule.type === "commit_message_pattern" ||
        rule.type === "branch_name_pattern" ||
        rule.type === "tag_name_pattern") &&
      !rule.parameters?.pattern
    ) {
      return invalid("Ruleset", "rules", "invalid", `The ${rule.type} rule needs a pattern`);
    }
    if (rule.type === "pull_request" && rule.parameters) {
      const n = rule.parameters.required_approving_review_count;
      if (n < 0 || n > 10) {
        return invalid(
          "Ruleset",
          "rules",
          "invalid",
          "Required approvals must be between 0 and 10",
        );
      }
    }
  }
  return undefined;
}

export const handlers = [
  ui.get("/repos/{owner}/{repo}/protection-rules", async ({ params, response }) => {
    const fault = await gate("admin-repo-protection");
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    if (!found) return response.untyped(notFound());
    return response(200).json(admin.rules(found.admin));
  }),

  http.get("/repos/{owner}/{repo}/branches/{branch}/protection", async ({ params, response }) => {
    const fault = await gate("admin-repo-protection");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    const rule = rec && admin.ruleFor(rec, params.branch);
    if (!rule) {
      return response.untyped(
        Response.json(
          {
            message: "Branch not protected",
            documentation_url:
              "https://docs.github.com/enterprise-server@3.22/rest/branches/branch-protection#get-branch-protection",
            status: "404",
          },
          { status: 404 },
        ),
      );
    }
    return response(200).json(rule.protection);
  }),

  // GHES's {branch} is a branch name; GHES's own UI also passes a wildcard pattern here,
  // and the mock accepts it the same way, so a rule like release/* is one PUT.
  http.put(
    "/repos/{owner}/{repo}/branches/{branch}/protection",
    async ({ params, request, response }) => {
      const fault = await gate("admin-repo-protection-save");
      if (fault) return response.untyped(fault);
      const found = admin.state(params.owner, params.repo);
      if (!found) return response.untyped(notFound());
      const { repo, admin: rec } = found;
      const pattern = params.branch.trim();
      const body = await request.json();
      const reviews = body.required_pull_request_reviews;
      const count = reviews?.required_approving_review_count ?? 1;
      if (reviews && (count < 0 || count > 6)) {
        return response(422).json({
          message: "Validation Failed",
          documentation_url:
            "https://docs.github.com/enterprise-server@3.22/rest/branches/branch-protection#update-branch-protection",
          errors: ["required_approving_review_count must be between 0 and 6"],
        });
      }
      const contexts = body.required_status_checks?.contexts ?? [];
      if (body.required_status_checks && contexts.length === 0) {
        return response(422).json({
          message: "Validation Failed",
          documentation_url:
            "https://docs.github.com/enterprise-server@3.22/rest/branches/branch-protection#update-branch-protection",
          errors: ["required_status_checks needs at least one check"],
        });
      }
      const unknownUser = body.restrictions?.users.find(
        (l) => !Object.values(users).some((u) => u.login === l),
      );
      if (unknownUser) {
        return response(422).json({
          message: "Validation Failed",
          documentation_url:
            "https://docs.github.com/enterprise-server@3.22/rest/branches/branch-protection#update-branch-protection",
          errors: [`${unknownUser} is not a user who can push to this repository`],
        });
      }
      const protection = protectionRecord(repo, pattern, {
        reviews: reviews
          ? {
              count,
              dismissStale: reviews.dismiss_stale_reviews ?? false,
              codeOwners: reviews.require_code_owner_reviews ?? false,
              lastPush: reviews.require_last_push_approval ?? false,
            }
          : null,
        checks: body.required_status_checks
          ? { strict: body.required_status_checks.strict, contexts }
          : null,
        enforceAdmins: body.enforce_admins ?? false,
        signatures: rec.protections.get(pattern)?.protection.required_signatures?.enabled ?? false,
        linear: body.required_linear_history ?? false,
        forcePushes: body.allow_force_pushes ?? false,
        deletions: body.allow_deletions ?? false,
        conversations: body.required_conversation_resolution ?? false,
        restrict: body.restrictions,
        lock: body.lock_branch,
      });
      const created_at = rec.protections.get(pattern)?.created_at ?? new Date().toISOString();
      rec.protections.set(pattern, { protection, created_at });
      return response(200).json(protectedBranch(protection));
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/branches/{branch}/protection",
    async ({ params, response }) => {
      const fault = await gate("admin-repo-protection-delete");
      if (fault) return response.untyped(fault);
      const rec = admin.state(params.owner, params.repo)?.admin;
      rec?.protections.delete(params.branch);
      return response(204).empty();
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/branches/{branch}/protection/required_signatures",
    async ({ params, response }) => {
      const fault = await gate("admin-repo-protection-save");
      if (fault) return response.untyped(fault);
      const rule = admin.state(params.owner, params.repo)?.admin.protections.get(params.branch);
      if (!rule?.protection.required_signatures) return response.untyped(notFound());
      rule.protection.required_signatures.enabled = true;
      return response(200).json(rule.protection.required_signatures);
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/branches/{branch}/protection/required_signatures",
    async ({ params, response }) => {
      const fault = await gate("admin-repo-protection-save");
      if (fault) return response.untyped(fault);
      const rule = admin.state(params.owner, params.repo)?.admin.protections.get(params.branch);
      if (!rule?.protection.required_signatures) return response.untyped(notFound());
      rule.protection.required_signatures.enabled = false;
      return response(204).empty();
    },
  ),

  ui.get("/repos/{owner}/{repo}/check-names", async ({ params, response }) => {
    const fault = await gate("admin-repo-checks");
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    return found ? response(200).json(admin.checkNames(found.repo)) : response.untyped(notFound());
  }),

  // ---- Rulesets ----

  http.get("/repos/{owner}/{repo}/rulesets", async ({ params, query, response }) => {
    const fault = await gate("admin-repo-rulesets");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    if (!rec) return response.untyped(notFound());
    // The list carries no rules or conditions, as GHES's does.
    const list = rec.rulesets.map(
      ({ rules, conditions, bypass_actors, ...r }) => (
        void rules,
        void conditions,
        void bypass_actors,
        r
      ),
    );
    return response(200).json(page(list, query));
  }),

  http.post("/repos/{owner}/{repo}/rulesets", async ({ params, request, response }) => {
    const fault = await gate("admin-repo-ruleset-save");
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    if (!found) return response.untyped(notFound());
    const { repo, admin: rec } = found;
    const body = await request.json();
    const problem = rulesetProblem(body, rec.rulesets);
    if (problem) return response.untyped(problem);
    const now = new Date().toISOString();
    const n = 7000 + rec.rulesets.length + Math.floor(Math.random() * 100_000);
    const created: Ruleset = {
      id: n,
      name: body.name.trim(),
      target: body.target ?? "branch",
      source_type: "Repository",
      source: repo.full_name,
      enforcement: body.enforcement,
      bypass_actors: body.bypass_actors ?? [],
      current_user_can_bypass: body.bypass_actors?.length ? "always" : "never",
      node_id: `RRS_${n}`,
      _links: {
        self: { href: `${repo.url}/rulesets/${n}` },
        html: { href: `${repo.html_url}/rules/${n}` },
      },
      conditions: body.conditions ?? { ref_name: { include: [], exclude: [] } },
      rules: body.rules ?? [],
      created_at: now,
      updated_at: now,
    };
    rec.rulesets.push(created);
    rec.ruleHistory.set(n, [
      { version_id: 1, actor: { id: users.mira.id, type: "User" }, updated_at: now },
    ]);
    return response(201).json(created);
  }),

  // Before /rulesets/{ruleset_id}, which would otherwise match "rule-suites" as an id.
  http.get("/repos/{owner}/{repo}/rulesets/rule-suites", async ({ params, query, response }) => {
    const fault = await gate("admin-repo-rule-suites");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    if (!rec) return response.untyped(notFound());
    const hours =
      { hour: 1, day: 24, week: 168, month: 720 }[query.get("time_period") ?? "day"] ?? 24;
    const since = Date.now() - hours * 3_600_000;
    const result = query.get("rule_suite_result") ?? "all";
    const actor = query.get("actor_name");
    const ref = query.get("ref");
    const list = rec.suites
      .filter((s) => Date.parse(s.pushed_at ?? "") >= since)
      .filter((s) => result === "all" || s.result === result)
      .filter((s) => !actor || s.actor_name === actor)
      .filter((s) => !ref || s.ref === ref || s.ref?.endsWith(`/${ref}`))
      .map(({ evaluations, ...s }) => (void evaluations, s));
    return response(200).json(page(list, query));
  }),

  http.get(
    "/repos/{owner}/{repo}/rulesets/rule-suites/{rule_suite_id}",
    async ({ params, response }) => {
      const fault = await gate("admin-repo-rule-suites");
      if (fault) return response.untyped(fault);
      const s = admin
        .state(params.owner, params.repo)
        ?.admin.suites.find((x) => x.id === Number(params.rule_suite_id));
      if (!s) return response.untyped(notFound());
      const { evaluations, ...rest } = s;
      return response(200).json({ ...rest, rule_evaluations: evaluations });
    },
  ),
  http.get("/repos/{owner}/{repo}/rulesets/{ruleset_id}", async ({ params, response }) => {
    const fault = await gate("admin-repo-ruleset");
    if (fault) return response.untyped(fault);
    const r = admin
      .state(params.owner, params.repo)
      ?.admin.rulesets.find((x) => x.id === Number(params.ruleset_id));
    return r ? response(200).json(r) : response.untyped(notFound());
  }),

  http.put("/repos/{owner}/{repo}/rulesets/{ruleset_id}", async ({ params, request, response }) => {
    const fault = await gate("admin-repo-ruleset-save");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    const r = rec?.rulesets.find((x) => x.id === Number(params.ruleset_id));
    if (!rec || !r) return response.untyped(notFound());
    const body = (await request.json()) ?? {};
    const problem = rulesetProblem(
      body,
      rec.rulesets.filter((x) => x.id !== r.id),
    );
    if (problem) return response.untyped(problem);
    const now = new Date().toISOString();
    Object.assign(r, body, { name: body.name?.trim() ?? r.name, updated_at: now });
    const history = rec.ruleHistory.get(r.id) ?? [];
    rec.ruleHistory.set(r.id, [
      {
        version_id: (history[0]?.version_id ?? 0) + 1,
        actor: { id: users.mira.id, type: "User" },
        updated_at: now,
      },
      ...history,
    ]);
    return response(200).json(r);
  }),

  http.delete("/repos/{owner}/{repo}/rulesets/{ruleset_id}", async ({ params, response }) => {
    const fault = await gate("admin-repo-ruleset-delete");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    if (!rec) return response.untyped(notFound());
    rec.rulesets = rec.rulesets.filter((x) => x.id !== Number(params.ruleset_id));
    return response(204).empty();
  }),

  http.get(
    "/repos/{owner}/{repo}/rulesets/{ruleset_id}/history",
    async ({ params, query, response }) => {
      const fault = await gate("admin-repo-ruleset");
      if (fault) return response.untyped(fault);
      const h = admin
        .state(params.owner, params.repo)
        ?.admin.ruleHistory.get(Number(params.ruleset_id));
      return h ? response(200).json(page(h, query)) : response.untyped(notFound());
    },
  ),
];
