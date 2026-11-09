import { HttpResponse } from "msw";
import type { Schemas } from "@/api/client";
import { fromBase64, publicKeyOf, sealOpen, toBase64 } from "@/areas/actions/sealed-box";
import { gate, http, notFound, page, unprocessable } from "../http";
import { API, branches, edge, org, raft, repositories, sha, web, type Repository } from "../world";
import { runner, runners, type RunnerRec } from "./actions-db";

// Actions settings for repositories and the organization: permissions, sealed secrets,
// variables, self-hosted runners and runner groups, and caches.

type Secret = Schemas["actions-secret"];
type Variable = Schemas["actions-variable"];
type Visibility = "all" | "private" | "selected";
type OrgSecret = Secret & { visibility: Visibility; selected: number[] };
type OrgVariable = Variable & { visibility: Visibility; selected: number[] };
type Cache = Schemas["actions-cache-list"]["actions_caches"][number];

const now = () => new Date().toISOString();
const full = (p: { owner: string; repo: string }) => `${p.owner}/${p.repo}`.toLowerCase();
const known = (p: { owner: string; repo: string }) => full(p) in branches;
const repoByName = (f: string) => repositories.find((r) => r.full_name === f)!;
const isOrg = (o: string) => o.toLowerCase() === org.login;

// Each scope has its own X25519 key pair. The mock keeps the secret half so it can open
// what the client sealed, which proves the client sealed to the key it was given.
function keyPair(seed: string) {
  const secret = Uint8Array.from(sha(`seal:${seed}`).slice(0, 64).match(/../g)!, (h) =>
    parseInt(h, 16),
  );
  return {
    secret,
    publicKey: {
      key_id: String(5_682_000_000_000_000_000n + BigInt(parseInt(sha(seed).slice(0, 8), 16))),
      key: toBase64(publicKeyOf(secret)),
    },
  };
}
const keys = new Map<string, ReturnType<typeof keyPair>>();
const keyFor = (scope: string) => {
  if (!keys.has(scope)) keys.set(scope, keyPair(scope));
  return keys.get(scope)!;
};

// Opens the box, or answers the 422 GHES would. The value itself is dropped: GHES never
// returns a secret value, so neither does this mock.
function openBox(scope: string, body: { encrypted_value: string; key_id: string }) {
  const pair = keyFor(scope);
  if (body.key_id !== pair.publicKey.key_id) return unprocessable("Secret", "key_id", "invalid");
  let box: Uint8Array;
  try {
    box = fromBase64(body.encrypted_value);
  } catch {
    return unprocessable("Secret", "encrypted_value", "invalid");
  }
  const plain = sealOpen(box, pair.secret);
  if (!plain) return unprocessable("Secret", "encrypted_value", "invalid");
  if (plain.length > 48 * 1024) return unprocessable("Secret", "encrypted_value", "too_large");
  return undefined;
}

// GHES names: letters, digits, and underscores, not starting with a digit or GITHUB_.
function badName(name: string) {
  if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) return "invalid";
  if (/^GITHUB_/i.test(name)) return "reserved";
  return null;
}

// ---- Repository state ----

type RepoActions = {
  permissions: Schemas["actions-repository-permissions"];
  selected: Schemas["selected-actions"];
  workflow: Schemas["actions-get-default-workflow-permissions"];
  fork: Schemas["actions-fork-pr-contributor-approval"];
  retention: number;
  secrets: Secret[];
  variables: Variable[];
  caches: Cache[];
  cacheLimitGb: number;
};

const day = (n: number) =>
  new Date(Date.parse("2026-11-08T10:00:00Z") - n * 86_400_000).toISOString();

function repoActions(r: Repository, secrets: string[], variables: [string, string][]): RepoActions {
  return {
    permissions: {
      enabled: true,
      allowed_actions: r.name === "edge" ? "selected" : "all",
      selected_actions_url: `${r.url}/actions/permissions/selected-actions`,
      sha_pinning_required: false,
    },
    selected: {
      github_owned_allowed: true,
      verified_allowed: false,
      patterns_allowed: ["dtolnay/rust-toolchain@*", "graftyard/*"],
    },
    workflow: { default_workflow_permissions: "read", can_approve_pull_request_reviews: false },
    fork: { approval_policy: "first_time_contributors" },
    retention: 90,
    secrets: secrets.map((name, i) => ({
      name,
      created_at: day(120 - i * 9),
      updated_at: day(30 - i * 4),
    })),
    variables: variables.map(([name, value], i) => ({
      name,
      value,
      created_at: day(80 - i),
      updated_at: day(12 - i),
    })),
    caches: [],
    cacheLimitGb: 10,
  };
}

const repoState = new Map<string, RepoActions>([
  [
    "graftyard/edge",
    repoActions(
      edge,
      ["CARGO_REGISTRY_TOKEN", "SENTRY_DSN", "BYTEDIFF_ORACLE_TOKEN"],
      [
        ["RUST_LOG", "info,edge=debug"],
        ["GHES_VERSION", "3.22"],
        ["BYTEDIFF_RECORDINGS", "recordings/ghes-3.22"],
      ],
    ),
  ],
  ["graftyard/raft", repoActions(raft, ["JEPSEN_SSH_KEY"], [["JEPSEN_TIME_LIMIT", "300"]])],
  ["graftyard/web", repoActions(web, [], [["NODE_VERSION", "24"]])],
]);

let cacheSeq = 3100;
for (const [f, s] of repoState) {
  const repoBranches = branches[f];
  const kinds =
    f === "graftyard/web"
      ? ["node-modules", "vite"]
      : ["cargo-registry", "cargo-target", "sccache"];
  repoBranches.forEach((b, bi) =>
    kinds.forEach((k, ki) => {
      const hash = sha(`${f}:${b}:${k}`);
      s.caches.push({
        id: cacheSeq++,
        ref: `refs/heads/${b}`,
        key: `${k}-Linux-x64-${hash.slice(0, 16)}`,
        version: sha(`v:${k}`).slice(0, 64),
        last_accessed_at: day(bi * 0.7 + ki * 0.2),
        created_at: day(bi * 1.3 + ki + 2),
        size_in_bytes: Math.round(
          (ki === 1 ? 900_000_000 : 180_000_000) * (0.4 + parseInt(hash.slice(0, 4), 16) / 65536),
        ),
      });
    }),
  );
}

// ---- Organization state ----

const orgSecrets: OrgSecret[] = [
  { name: "NPM_TOKEN", created_at: day(200), updated_at: day(40), visibility: "all", selected: [] },
  {
    name: "SLACK_WEBHOOK_URL",
    created_at: day(150),
    updated_at: day(150),
    visibility: "private",
    selected: [],
  },
  {
    name: "SIGNING_KEY",
    created_at: day(90),
    updated_at: day(20),
    visibility: "selected",
    selected: [edge.id, raft.id],
  },
];
const orgVariables: OrgVariable[] = [
  {
    name: "DEFAULT_REGION",
    value: "weur",
    created_at: day(300),
    updated_at: day(30),
    visibility: "all",
    selected: [],
  },
  {
    name: "RUNNER_IMAGE",
    value: "ghcr.graftyard.test/runner:2.329.0",
    created_at: day(40),
    updated_at: day(3),
    visibility: "selected",
    selected: [edge.id],
  },
];

type Group = Schemas["runner-groups-org"] & { repos: number[] };
let groupSeq = 3;
const group = (id: number, name: string, visibility: string, repos: number[] = []): Group => ({
  id,
  name,
  visibility,
  default: id === 1,
  runners_url: `${API}/orgs/graftyard/actions/runner-groups/${id}/runners`,
  selected_repositories_url: `${API}/orgs/graftyard/actions/runner-groups/${id}/repositories`,
  inherited: false,
  allows_public_repositories: id === 1,
  workflow_restrictions_read_only: false,
  restricted_to_workflows: false,
  selected_workflows: [],
  repos,
});
const groups: Group[] = [group(1, "Default", "all"), group(2, "GPU", "selected", [edge.id])];

const visibleTo = (v: Visibility, selected: number[], r: Repository) =>
  v === "all" ||
  (v === "private" && r.visibility !== "public") ||
  (v === "selected" && selected.includes(r.id));

function minimal(r: Repository): Schemas["minimal-repository"] {
  const { license, ...rest } = r;
  return {
    ...rest,
    license: license && {
      key: license.key,
      name: license.name,
      url: license.url,
      node_id: license.node_id,
      spdx_id: license.spdx_id ?? undefined,
    },
  };
}

const publicRunner = ({ scope, repo, ...r }: RunnerRec): Schemas["runner"] => (
  void scope,
  void repo,
  r
);

const registrationToken = (scope: string): Schemas["authentication-token"] => ({
  token: `A${sha(`reg:${scope}:${Date.now()}`).slice(0, 28).toUpperCase()}`,
  expires_at: new Date(Date.now() + 3_600_000).toISOString(),
});

const tooManyLabels = () =>
  HttpResponse.json(
    {
      message: "Validation Failed",
      documentation_url:
        "https://docs.github.com/enterprise-server@3.22/rest/actions/self-hosted-runners",
      errors: ["A runner may have at most 100 labels"],
    },
    { status: 422 },
  );

function labelsBody(r: RunnerRec) {
  return { total_count: r.labels.length, labels: r.labels };
}

function setLabels(r: RunnerRec, custom: string[]) {
  const readOnly = r.labels.filter((l) => l.type === "read-only");
  r.labels = [
    ...readOnly,
    ...[...new Set(custom.map((l) => l.trim()).filter(Boolean))]
      .filter((l) => !readOnly.some((x) => x.name.toLowerCase() === l.toLowerCase()))
      .map((name, i) => ({ id: 500 + i + name.length, name, type: "custom" as const })),
  ];
}

const DOWNLOADS: Schemas["runner-application"][] = [
  ["linux", "x64"],
  ["linux", "arm64"],
  ["osx", "arm64"],
  ["win", "x64"],
].map(([os, arch]) => ({
  os,
  architecture: arch,
  filename: `actions-runner-${os}-${arch}-2.329.0.${os === "win" ? "zip" : "tar.gz"}`,
  download_url: `https://ghes.graftyard.test/_runner/v2.329.0/actions-runner-${os}-${arch}-2.329.0.${os === "win" ? "zip" : "tar.gz"}`,
  sha256_checksum: sha(`${os}${arch}`) + sha(`${arch}${os}`).slice(0, 24),
}));

function state(p: { owner: string; repo: string }) {
  return repoState.get(full(p));
}

export const handlers = [
  // ---- Repository permissions ----

  http.get("/repos/{owner}/{repo}/actions/permissions", async ({ params, response }) => {
    const fault = await gate("actions-permissions");
    if (fault) return response.untyped(fault);
    const s = state(params);
    return s ? response(200).json(s.permissions) : response.untyped(notFound());
  }),
  http.put("/repos/{owner}/{repo}/actions/permissions", async ({ params, request, response }) => {
    const fault = await gate("actions-permissions-update");
    if (fault) return response.untyped(fault);
    const s = state(params);
    if (!s) return response.untyped(notFound());
    const body = await request.json();
    s.permissions = { ...s.permissions, ...body };
    return response(204).empty();
  }),
  http.get(
    "/repos/{owner}/{repo}/actions/permissions/selected-actions",
    async ({ params, response }) => {
      const fault = await gate("actions-permissions");
      if (fault) return response.untyped(fault);
      const s = state(params);
      return s ? response(200).json(s.selected) : response.untyped(notFound());
    },
  ),
  http.put(
    "/repos/{owner}/{repo}/actions/permissions/selected-actions",
    async ({ params, request, response }) => {
      const fault = await gate("actions-permissions-update");
      if (fault) return response.untyped(fault);
      const s = state(params);
      if (!s) return response.untyped(notFound());
      const body = (await request.json()) ?? {};
      const bad = (body.patterns_allowed ?? []).find(
        (p) => !/^[\w.-]+(\/[\w.*-]+)*(@[\w.*-]+)?$/.test(p),
      );
      if (bad)
        return response.untyped(unprocessable("SelectedActions", "patterns_allowed", "invalid"));
      if (s.permissions.allowed_actions !== "selected")
        return response.untyped(
          HttpResponse.json(
            { message: "Selected actions are only used when allowed_actions is selected." },
            { status: 409 },
          ),
        );
      s.selected = { ...s.selected, ...body };
      return response(204).empty();
    },
  ),
  http.get("/repos/{owner}/{repo}/actions/permissions/workflow", async ({ params, response }) => {
    const fault = await gate("actions-permissions");
    if (fault) return response.untyped(fault);
    const s = state(params);
    return s ? response(200).json(s.workflow) : response.untyped(notFound());
  }),
  http.put(
    "/repos/{owner}/{repo}/actions/permissions/workflow",
    async ({ params, request, response }) => {
      const fault = await gate("actions-permissions-update");
      if (fault) return response.untyped(fault);
      const s = state(params);
      if (!s) return response.untyped(notFound());
      s.workflow = { ...s.workflow, ...(await request.json()) };
      return response(204).empty();
    },
  ),
  http.get(
    "/repos/{owner}/{repo}/actions/permissions/fork-pr-contributor-approval",
    async ({ params, response }) => {
      const fault = await gate("actions-permissions");
      if (fault) return response.untyped(fault);
      const s = state(params);
      return s ? response(200).json(s.fork) : response.untyped(notFound());
    },
  ),
  http.put(
    "/repos/{owner}/{repo}/actions/permissions/fork-pr-contributor-approval",
    async ({ params, request, response }) => {
      const fault = await gate("actions-permissions-update");
      if (fault) return response.untyped(fault);
      const s = state(params);
      if (!s) return response.untyped(notFound());
      s.fork = await request.json();
      return response(204).empty();
    },
  ),
  http.get(
    "/repos/{owner}/{repo}/actions/permissions/artifact-and-log-retention",
    async ({ params, response }) => {
      const fault = await gate("actions-permissions");
      if (fault) return response.untyped(fault);
      const s = state(params);
      return s
        ? response(200).json({ days: s.retention, maximum_allowed_days: 400 })
        : response.untyped(notFound());
    },
  ),
  http.put(
    "/repos/{owner}/{repo}/actions/permissions/artifact-and-log-retention",
    async ({ params, request, response }) => {
      const fault = await gate("actions-permissions-update");
      if (fault) return response.untyped(fault);
      const s = state(params);
      if (!s) return response.untyped(notFound());
      const { days } = await request.json();
      if (!Number.isInteger(days) || days < 1 || days > 400)
        return response.untyped(unprocessable("ArtifactAndLogRetention", "days", "invalid"));
      s.retention = days;
      return response(204).empty();
    },
  ),

  // ---- Repository secrets ----

  http.get("/repos/{owner}/{repo}/actions/secrets/public-key", async ({ params, response }) => {
    const fault = await gate("actions-secrets");
    if (fault) return response.untyped(fault);
    if (!known(params)) return response.untyped(notFound());
    return response(200).json(keyFor(full(params)).publicKey);
  }),
  http.get("/repos/{owner}/{repo}/actions/secrets", async ({ params, query, response }) => {
    const fault = await gate("actions-secrets");
    if (fault) return response.untyped(fault);
    const s = state(params);
    if (!s) return response.untyped(notFound());
    const list = [...s.secrets].sort((a, b) => a.name.localeCompare(b.name));
    return response(200).json({ total_count: list.length, secrets: page(list, query) });
  }),
  http.get("/repos/{owner}/{repo}/actions/secrets/{secret_name}", async ({ params, response }) => {
    const fault = await gate("actions-secrets");
    if (fault) return response.untyped(fault);
    const secret = state(params)?.secrets.find((x) => x.name === params.secret_name.toUpperCase());
    return secret ? response(200).json(secret) : response.untyped(notFound());
  }),
  http.put(
    "/repos/{owner}/{repo}/actions/secrets/{secret_name}",
    async ({ params, request, response }) => {
      const fault = await gate("actions-secrets-write");
      if (fault) return response.untyped(fault);
      const s = state(params);
      if (!s) return response.untyped(notFound());
      const name = params.secret_name.toUpperCase();
      const bad = badName(name);
      if (bad) return response.untyped(unprocessable("Secret", "name", bad));
      const rejected = openBox(full(params), await request.json());
      if (rejected) return response.untyped(rejected);
      const existing = s.secrets.find((x) => x.name === name);
      if (existing) {
        existing.updated_at = now();
        return response(204).empty();
      }
      s.secrets.push({ name, created_at: now(), updated_at: now() });
      return response(201).json({});
    },
  ),
  http.delete(
    "/repos/{owner}/{repo}/actions/secrets/{secret_name}",
    async ({ params, response }) => {
      const fault = await gate("actions-secrets-write");
      if (fault) return response.untyped(fault);
      const s = state(params);
      const name = params.secret_name.toUpperCase();
      if (!s?.secrets.some((x) => x.name === name)) return response.untyped(notFound());
      s.secrets = s.secrets.filter((x) => x.name !== name);
      return response(204).empty();
    },
  ),
  http.get(
    "/repos/{owner}/{repo}/actions/organization-secrets",
    async ({ params, query, response }) => {
      const fault = await gate("actions-repo-org-secrets");
      if (fault) return response.untyped(fault);
      if (!known(params)) return response.untyped(notFound());
      const r = repoByName(full(params));
      const list = orgSecrets
        .filter((x) => visibleTo(x.visibility, x.selected, r))
        .map(({ name, created_at, updated_at }) => ({ name, created_at, updated_at }));
      return response(200).json({ total_count: list.length, secrets: page(list, query) });
    },
  ),
  // ---- Repository variables ----

  http.get("/repos/{owner}/{repo}/actions/variables", async ({ params, query, response }) => {
    const fault = await gate("actions-variables");
    if (fault) return response.untyped(fault);
    const s = state(params);
    if (!s) return response.untyped(notFound());
    const list = [...s.variables].sort((a, b) => a.name.localeCompare(b.name));
    return response(200).json({ total_count: list.length, variables: page(list, query) });
  }),
  http.post("/repos/{owner}/{repo}/actions/variables", async ({ params, request, response }) => {
    const fault = await gate("actions-variables-write");
    if (fault) return response.untyped(fault);
    const s = state(params);
    if (!s) return response.untyped(notFound());
    const body = await request.json();
    const name = body.name.trim().toUpperCase();
    const bad = badName(name);
    if (bad) return response.untyped(unprocessable("Variable", "name", bad));
    if (s.variables.some((v) => v.name === name))
      return response.untyped(unprocessable("Variable", "name", "already_exists"));
    if (!body.value) return response.untyped(unprocessable("Variable", "value", "missing_field"));
    s.variables.push({ name, value: body.value, created_at: now(), updated_at: now() });
    return response(201).json({});
  }),
  http.get("/repos/{owner}/{repo}/actions/variables/{name}", async ({ params, response }) => {
    const fault = await gate("actions-variables");
    if (fault) return response.untyped(fault);
    const v = state(params)?.variables.find((x) => x.name === params.name.toUpperCase());
    return v ? response(200).json(v) : response.untyped(notFound());
  }),
  http.patch(
    "/repos/{owner}/{repo}/actions/variables/{name}",
    async ({ params, request, response }) => {
      const fault = await gate("actions-variables-write");
      if (fault) return response.untyped(fault);
      const s = state(params);
      const v = s?.variables.find((x) => x.name === params.name.toUpperCase());
      if (!s || !v) return response.untyped(notFound());
      const body = await request.json();
      if (body.name !== undefined) {
        const name = body.name.trim().toUpperCase();
        const bad = badName(name);
        if (bad) return response.untyped(unprocessable("Variable", "name", bad));
        if (name !== v.name && s.variables.some((x) => x.name === name))
          return response.untyped(unprocessable("Variable", "name", "already_exists"));
        v.name = name;
      }
      if (body.value !== undefined) {
        if (!body.value)
          return response.untyped(unprocessable("Variable", "value", "missing_field"));
        v.value = body.value;
      }
      v.updated_at = now();
      return response(204).empty();
    },
  ),
  http.delete("/repos/{owner}/{repo}/actions/variables/{name}", async ({ params, response }) => {
    const fault = await gate("actions-variables-write");
    if (fault) return response.untyped(fault);
    const s = state(params);
    if (!s?.variables.some((x) => x.name === params.name.toUpperCase()))
      return response.untyped(notFound());
    s.variables = s.variables.filter((x) => x.name !== params.name.toUpperCase());
    return response(204).empty();
  }),
  http.get(
    "/repos/{owner}/{repo}/actions/organization-variables",
    async ({ params, query, response }) => {
      const fault = await gate("actions-repo-org-variables");
      if (fault) return response.untyped(fault);
      if (!known(params)) return response.untyped(notFound());
      const r = repoByName(full(params));
      const list = orgVariables
        .filter((x) => visibleTo(x.visibility, x.selected, r))
        .map(({ name, value, created_at, updated_at }) => ({
          name,
          value,
          created_at,
          updated_at,
        }));
      return response(200).json({ total_count: list.length, variables: page(list, query) });
    },
  ),

  // ---- Repository runners ----

  http.get("/repos/{owner}/{repo}/actions/runners", async ({ params, query, response }) => {
    const fault = await gate("actions-runners");
    if (fault) return response.untyped(fault);
    if (!known(params)) return response.untyped(notFound());
    const name = query.get("name");
    const list = runners
      .filter((r) => r.scope === "repo" && r.repo === full(params) && (!name || r.name === name))
      .map(publicRunner);
    return response(200).json({ total_count: list.length, runners: page(list, query) });
  }),
  http.get("/repos/{owner}/{repo}/actions/runners/downloads", async ({ params, response }) => {
    const fault = await gate("actions-runners");
    if (fault) return response.untyped(fault);
    return known(params) ? response(200).json(DOWNLOADS) : response.untyped(notFound());
  }),
  http.post(
    "/repos/{owner}/{repo}/actions/runners/registration-token",
    async ({ params, response }) => {
      const fault = await gate("actions-runner-token");
      if (fault) return response.untyped(fault);
      if (!known(params)) return response.untyped(notFound());
      const token = registrationToken(full(params));
      // A runner configured with this token comes online a little later, as config.sh would.
      setTimeout(() => {
        if (
          !runners.some(
            (r) => r.name === `edge-runner-new-${token.token.slice(1, 5).toLowerCase()}`,
          )
        )
          runners.push(
            runner(
              `edge-runner-new-${token.token.slice(1, 5).toLowerCase()}`,
              "online",
              ["linux", "x64"],
              "repo",
              full(params),
            ),
          );
      }, 15_000);
      return response(201).json(token);
    },
  ),
  http.get("/repos/{owner}/{repo}/actions/runners/{runner_id}", async ({ params, response }) => {
    const fault = await gate("actions-runners");
    if (fault) return response.untyped(fault);
    const r = runners.find((x) => x.id === Number(params.runner_id) && x.repo === full(params));
    return r ? response(200).json(publicRunner(r)) : response.untyped(notFound());
  }),
  http.delete("/repos/{owner}/{repo}/actions/runners/{runner_id}", async ({ params, response }) => {
    const fault = await gate("actions-runner-delete");
    if (fault) return response.untyped(fault);
    const i = runners.findIndex(
      (x) => x.id === Number(params.runner_id) && x.repo === full(params),
    );
    if (i < 0) return response.untyped(notFound());
    if (runners[i].busy)
      return response.untyped(
        HttpResponse.json(
          {
            message: "Validation Failed",
            documentation_url:
              "https://docs.github.com/enterprise-server@3.22/rest/actions/self-hosted-runners",
            errors: ["This runner is running a job. Cancel the job first."],
          },
          { status: 422 },
        ),
      );
    runners.splice(i, 1);
    return response(204).empty();
  }),
  http.put(
    "/repos/{owner}/{repo}/actions/runners/{runner_id}/labels",
    async ({ params, request, response }) => {
      const fault = await gate("actions-runner-labels");
      if (fault) return response.untyped(fault);
      const r = runners.find((x) => x.id === Number(params.runner_id) && x.repo === full(params));
      if (!r) return response.untyped(notFound());
      const { labels } = await request.json();
      if (labels.length > 100) return response.untyped(tooManyLabels());
      setLabels(r, labels);
      return response(200).json(labelsBody(r));
    },
  ),

  // ---- Caches ----

  http.get("/repos/{owner}/{repo}/actions/caches", async ({ params, query, response }) => {
    const fault = await gate("actions-caches");
    if (fault) return response.untyped(fault);
    const s = state(params);
    if (!s) return response.untyped(notFound());
    const ref = query.get("ref");
    const key = query.get("key");
    const sortKey = query.get("sort") ?? "last_accessed_at";
    const dir = query.get("direction") === "asc" ? 1 : -1;
    const list = s.caches
      .filter((c) => (!ref || c.ref === ref) && (!key || c.key?.startsWith(key)))
      .sort((a, b) => {
        const av =
          sortKey === "size_in_bytes"
            ? (a.size_in_bytes ?? 0)
            : String(sortKey === "created_at" ? a.created_at : a.last_accessed_at);
        const bv =
          sortKey === "size_in_bytes"
            ? (b.size_in_bytes ?? 0)
            : String(sortKey === "created_at" ? b.created_at : b.last_accessed_at);
        return (av < bv ? -1 : av > bv ? 1 : 0) * dir;
      });
    return response(200).json({ total_count: list.length, actions_caches: page(list, query) });
  }),
  http.get("/repos/{owner}/{repo}/actions/cache/usage", async ({ params, response }) => {
    const fault = await gate("actions-cache-usage");
    if (fault) return response.untyped(fault);
    const s = state(params);
    if (!s) return response.untyped(notFound());
    return response(200).json({
      full_name: full(params),
      active_caches_count: s.caches.length,
      active_caches_size_in_bytes: s.caches.reduce((n, c) => n + (c.size_in_bytes ?? 0), 0),
    });
  }),
  http.get("/repos/{owner}/{repo}/actions/cache/usage-policy", async ({ params, response }) => {
    const fault = await gate("actions-cache-usage");
    if (fault) return response.untyped(fault);
    const s = state(params);
    return s
      ? response(200).json({ repo_cache_size_limit_in_gb: s.cacheLimitGb })
      : response.untyped(notFound());
  }),
  http.delete("/repos/{owner}/{repo}/actions/caches/{cache_id}", async ({ params, response }) => {
    const fault = await gate("actions-cache-delete");
    if (fault) return response.untyped(fault);
    const s = state(params);
    if (!s?.caches.some((c) => c.id === Number(params.cache_id)))
      return response.untyped(notFound());
    s.caches = s.caches.filter((c) => c.id !== Number(params.cache_id));
    return response(204).empty();
  }),
  http.delete("/repos/{owner}/{repo}/actions/caches", async ({ params, query, response }) => {
    const fault = await gate("actions-cache-delete");
    if (fault) return response.untyped(fault);
    const s = state(params);
    if (!s) return response.untyped(notFound());
    const key = query.get("key");
    const ref = query.get("ref");
    const gone = s.caches.filter((c) => c.key === key && (!ref || c.ref === ref));
    if (gone.length === 0) return response.untyped(notFound());
    s.caches = s.caches.filter((c) => !gone.includes(c));
    return response(200).json({ total_count: gone.length, actions_caches: gone });
  }),

  // ---- Organization secrets ----

  http.get("/orgs/{org}/actions/secrets/public-key", async ({ params, response }) => {
    const fault = await gate("actions-org-secrets");
    if (fault) return response.untyped(fault);
    return isOrg(params.org)
      ? response(200).json(keyFor(`org:${org.login}`).publicKey)
      : response.untyped(notFound());
  }),
  http.get("/orgs/{org}/actions/secrets", async ({ params, query, response }) => {
    const fault = await gate("actions-org-secrets");
    if (fault) return response.untyped(fault);
    if (!isOrg(params.org)) return response.untyped(notFound());
    const list = [...orgSecrets]
      .sort((a, b) => a.name.localeCompare(b.name))
      .map(({ selected, ...s }) => ({
        ...s,
        selected_repositories_url:
          s.visibility === "selected"
            ? `${API}/orgs/graftyard/actions/secrets/${s.name}/repositories`
            : undefined,
        ...(void selected, {}),
      }));
    return response(200).json({ total_count: list.length, secrets: page(list, query) });
  }),
  http.put("/orgs/{org}/actions/secrets/{secret_name}", async ({ params, request, response }) => {
    const fault = await gate("actions-org-secrets-write");
    if (fault) return response.untyped(fault);
    if (!isOrg(params.org)) return response.untyped(notFound());
    const name = params.secret_name.toUpperCase();
    const bad = badName(name);
    if (bad) return response.untyped(unprocessable("Secret", "name", bad));
    const body = await request.json();
    const rejected = openBox(`org:${org.login}`, body);
    if (rejected) return response.untyped(rejected);
    if (body.visibility === "selected" && !(body.selected_repository_ids ?? []).length)
      return response.untyped(unprocessable("Secret", "selected_repository_ids", "missing_field"));
    const existing = orgSecrets.find((s) => s.name === name);
    const selected = body.visibility === "selected" ? (body.selected_repository_ids ?? []) : [];
    if (existing) {
      Object.assign(existing, { visibility: body.visibility, selected, updated_at: now() });
      return response(204).empty();
    }
    orgSecrets.push({
      name,
      visibility: body.visibility,
      selected,
      created_at: now(),
      updated_at: now(),
    });
    return response(201).json({});
  }),
  http.delete("/orgs/{org}/actions/secrets/{secret_name}", async ({ params, response }) => {
    const fault = await gate("actions-org-secrets-write");
    if (fault) return response.untyped(fault);
    const i = orgSecrets.findIndex((s) => s.name === params.secret_name.toUpperCase());
    if (!isOrg(params.org) || i < 0) return response.untyped(notFound());
    orgSecrets.splice(i, 1);
    return response(204).empty();
  }),
  http.get(
    "/orgs/{org}/actions/secrets/{secret_name}/repositories",
    async ({ params, response }) => {
      const fault = await gate("actions-org-secrets");
      if (fault) return response.untyped(fault);
      const s = orgSecrets.find((x) => x.name === params.secret_name.toUpperCase());
      if (!isOrg(params.org) || !s) return response.untyped(notFound());
      const list = repositories.filter((r) => s.selected.includes(r.id)).map(minimal);
      return response(200).json({ total_count: list.length, repositories: list });
    },
  ),
  http.put(
    "/orgs/{org}/actions/secrets/{secret_name}/repositories",
    async ({ params, request, response }) => {
      const fault = await gate("actions-org-secrets-write");
      if (fault) return response.untyped(fault);
      const s = orgSecrets.find((x) => x.name === params.secret_name.toUpperCase());
      if (!isOrg(params.org) || !s) return response.untyped(notFound());
      if (s.visibility !== "selected")
        return response.untyped(
          HttpResponse.json(
            { message: "The secret's visibility is not selected." },
            { status: 409 },
          ),
        );
      s.selected = (await request.json()).selected_repository_ids;
      s.updated_at = now();
      return response(204).empty();
    },
  ),

  // ---- Organization variables ----

  http.get("/orgs/{org}/actions/variables", async ({ params, query, response }) => {
    const fault = await gate("actions-org-variables");
    if (fault) return response.untyped(fault);
    if (!isOrg(params.org)) return response.untyped(notFound());
    const list = [...orgVariables]
      .sort((a, b) => a.name.localeCompare(b.name))
      .map(({ selected, ...v }) => ({
        ...v,
        selected_repositories_url:
          v.visibility === "selected"
            ? `${API}/orgs/graftyard/actions/variables/${v.name}/repositories`
            : undefined,
        ...(void selected, {}),
      }));
    return response(200).json({ total_count: list.length, variables: page(list, query) });
  }),
  http.post("/orgs/{org}/actions/variables", async ({ params, request, response }) => {
    const fault = await gate("actions-org-variables-write");
    if (fault) return response.untyped(fault);
    if (!isOrg(params.org)) return response.untyped(notFound());
    const body = await request.json();
    const name = body.name.trim().toUpperCase();
    const bad = badName(name);
    if (bad) return response.untyped(unprocessable("Variable", "name", bad));
    if (orgVariables.some((v) => v.name === name))
      return response.untyped(unprocessable("Variable", "name", "already_exists"));
    if (!body.value) return response.untyped(unprocessable("Variable", "value", "missing_field"));
    if (body.visibility === "selected" && !(body.selected_repository_ids ?? []).length)
      return response.untyped(
        unprocessable("Variable", "selected_repository_ids", "missing_field"),
      );
    orgVariables.push({
      name,
      value: body.value,
      visibility: body.visibility,
      selected: body.selected_repository_ids ?? [],
      created_at: now(),
      updated_at: now(),
    });
    return response(201).json({});
  }),
  http.patch("/orgs/{org}/actions/variables/{name}", async ({ params, request, response }) => {
    const fault = await gate("actions-org-variables-write");
    if (fault) return response.untyped(fault);
    const v = orgVariables.find((x) => x.name === params.name.toUpperCase());
    if (!isOrg(params.org) || !v) return response.untyped(notFound());
    const body = await request.json();
    if (body.name !== undefined) {
      const name = body.name.trim().toUpperCase();
      const bad = badName(name);
      if (bad) return response.untyped(unprocessable("Variable", "name", bad));
      if (name !== v.name && orgVariables.some((x) => x.name === name))
        return response.untyped(unprocessable("Variable", "name", "already_exists"));
      v.name = name;
    }
    if (body.value !== undefined) {
      if (!body.value) return response.untyped(unprocessable("Variable", "value", "missing_field"));
      v.value = body.value;
    }
    if (body.visibility) v.visibility = body.visibility;
    if (body.selected_repository_ids) v.selected = body.selected_repository_ids;
    if (v.visibility === "selected" && v.selected.length === 0)
      return response.untyped(
        unprocessable("Variable", "selected_repository_ids", "missing_field"),
      );
    v.updated_at = now();
    return response(204).empty();
  }),
  http.delete("/orgs/{org}/actions/variables/{name}", async ({ params, response }) => {
    const fault = await gate("actions-org-variables-write");
    if (fault) return response.untyped(fault);
    const i = orgVariables.findIndex((v) => v.name === params.name.toUpperCase());
    if (!isOrg(params.org) || i < 0) return response.untyped(notFound());
    orgVariables.splice(i, 1);
    return response(204).empty();
  }),
  http.get("/orgs/{org}/actions/variables/{name}/repositories", async ({ params, response }) => {
    const fault = await gate("actions-org-variables");
    if (fault) return response.untyped(fault);
    const v = orgVariables.find((x) => x.name === params.name.toUpperCase());
    if (!isOrg(params.org) || !v) return response.untyped(notFound());
    const list = repositories.filter((r) => v.selected.includes(r.id)).map(minimal);
    return response(200).json({ total_count: list.length, repositories: list });
  }),

  // ---- Organization runners and runner groups ----

  http.get("/orgs/{org}/actions/runners", async ({ params, query, response }) => {
    const fault = await gate("actions-org-runners");
    if (fault) return response.untyped(fault);
    if (!isOrg(params.org)) return response.untyped(notFound());
    const name = query.get("name");
    const list = runners
      .filter((r) => r.scope === "org" && (!name || r.name === name))
      .map(publicRunner);
    return response(200).json({ total_count: list.length, runners: page(list, query) });
  }),
  http.post("/orgs/{org}/actions/runners/registration-token", async ({ params, response }) => {
    const fault = await gate("actions-runner-token");
    if (fault) return response.untyped(fault);
    return isOrg(params.org)
      ? response(201).json(registrationToken(`org:${org.login}`))
      : response.untyped(notFound());
  }),
  http.delete("/orgs/{org}/actions/runners/{runner_id}", async ({ params, response }) => {
    const fault = await gate("actions-runner-delete");
    if (fault) return response.untyped(fault);
    const i = runners.findIndex((x) => x.id === Number(params.runner_id) && x.scope === "org");
    if (!isOrg(params.org) || i < 0) return response.untyped(notFound());
    if (runners[i].busy)
      return response.untyped(
        HttpResponse.json(
          {
            message: "Validation Failed",
            documentation_url:
              "https://docs.github.com/enterprise-server@3.22/rest/actions/self-hosted-runners",
            errors: ["This runner is running a job. Cancel the job first."],
          },
          { status: 422 },
        ),
      );
    runners.splice(i, 1);
    return response(204).empty();
  }),
  http.put(
    "/orgs/{org}/actions/runners/{runner_id}/labels",
    async ({ params, request, response }) => {
      const fault = await gate("actions-runner-labels");
      if (fault) return response.untyped(fault);
      const r = runners.find((x) => x.id === Number(params.runner_id) && x.scope === "org");
      if (!isOrg(params.org) || !r) return response.untyped(notFound());
      const { labels } = await request.json();
      if (labels.length > 100) return response.untyped(tooManyLabels());
      setLabels(r, labels);
      return response(200).json(labelsBody(r));
    },
  ),
  http.get("/orgs/{org}/actions/runner-groups", async ({ params, query, response }) => {
    const fault = await gate("actions-runner-groups");
    if (fault) return response.untyped(fault);
    if (!isOrg(params.org)) return response.untyped(notFound());
    const list = groups.map(({ repos, ...g }) => (void repos, g));
    return response(200).json({ total_count: list.length, runner_groups: page(list, query) });
  }),
  http.post("/orgs/{org}/actions/runner-groups", async ({ params, request, response }) => {
    const fault = await gate("actions-runner-groups-write");
    if (fault) return response.untyped(fault);
    if (!isOrg(params.org)) return response.untyped(notFound());
    const body = await request.json();
    const name = body.name.trim();
    if (!name) return response.untyped(unprocessable("RunnerGroup", "name", "missing_field"));
    if (groups.some((g) => g.name.toLowerCase() === name.toLowerCase()))
      return response.untyped(unprocessable("RunnerGroup", "name", "already_exists"));
    const g = group(groupSeq++, name, body.visibility ?? "all", body.selected_repository_ids ?? []);
    g.allows_public_repositories = body.allows_public_repositories ?? false;
    groups.push(g);
    for (const id of body.runners ?? []) {
      const r = runners.find((x) => x.id === id);
      if (r) r.runner_group_id = g.id;
    }
    const { repos, ...out } = g;
    void repos;
    return response(201).json(out);
  }),
  http.patch(
    "/orgs/{org}/actions/runner-groups/{runner_group_id}",
    async ({ params, request, response }) => {
      const fault = await gate("actions-runner-groups-write");
      if (fault) return response.untyped(fault);
      const g = groups.find((x) => x.id === Number(params.runner_group_id));
      if (!isOrg(params.org) || !g) return response.untyped(notFound());
      const body = await request.json();
      const name = body.name.trim();
      if (!name) return response.untyped(unprocessable("RunnerGroup", "name", "missing_field"));
      if (groups.some((x) => x !== g && x.name.toLowerCase() === name.toLowerCase()))
        return response.untyped(unprocessable("RunnerGroup", "name", "already_exists"));
      if (g.default && body.visibility && body.visibility !== "all")
        return response.untyped(unprocessable("RunnerGroup", "visibility", "invalid"));
      g.name = name;
      if (body.visibility) g.visibility = body.visibility;
      if (body.allows_public_repositories !== undefined)
        g.allows_public_repositories = body.allows_public_repositories;
      const { repos, ...out } = g;
      void repos;
      return response(200).json(out);
    },
  ),
  http.delete(
    "/orgs/{org}/actions/runner-groups/{runner_group_id}",
    async ({ params, response }) => {
      const fault = await gate("actions-runner-groups-write");
      if (fault) return response.untyped(fault);
      const i = groups.findIndex((x) => x.id === Number(params.runner_group_id));
      if (!isOrg(params.org) || i < 0) return response.untyped(notFound());
      if (groups[i].default)
        return response.untyped(unprocessable("RunnerGroup", "id", "default_group"));
      for (const r of runners) if (r.runner_group_id === groups[i].id) r.runner_group_id = 1;
      groups.splice(i, 1);
      return response(204).empty();
    },
  ),
  http.get(
    "/orgs/{org}/actions/runner-groups/{runner_group_id}/repositories",
    async ({ params, response }) => {
      const fault = await gate("actions-runner-groups");
      if (fault) return response.untyped(fault);
      const g = groups.find((x) => x.id === Number(params.runner_group_id));
      if (!isOrg(params.org) || !g) return response.untyped(notFound());
      const list = repositories.filter((r) => g.repos.includes(r.id)).map(minimal);
      return response(200).json({ total_count: list.length, repositories: list });
    },
  ),
  http.put(
    "/orgs/{org}/actions/runner-groups/{runner_group_id}/repositories",
    async ({ params, request, response }) => {
      const fault = await gate("actions-runner-groups-write");
      if (fault) return response.untyped(fault);
      const g = groups.find((x) => x.id === Number(params.runner_group_id));
      if (!isOrg(params.org) || !g) return response.untyped(notFound());
      g.repos = (await request.json()).selected_repository_ids;
      return response(204).empty();
    },
  ),
  http.get(
    "/orgs/{org}/actions/runner-groups/{runner_group_id}/runners",
    async ({ params, query, response }) => {
      const fault = await gate("actions-runner-groups");
      if (fault) return response.untyped(fault);
      const g = groups.find((x) => x.id === Number(params.runner_group_id));
      if (!isOrg(params.org) || !g) return response.untyped(notFound());
      const list = runners
        .filter((r) => r.scope === "org" && r.runner_group_id === g.id)
        .map(publicRunner);
      return response(200).json({ total_count: list.length, runners: page(list, query) });
    },
  ),
  http.put(
    "/orgs/{org}/actions/runner-groups/{runner_group_id}/runners",
    async ({ params, request, response }) => {
      const fault = await gate("actions-runner-groups-write");
      if (fault) return response.untyped(fault);
      const g = groups.find((x) => x.id === Number(params.runner_group_id));
      if (!isOrg(params.org) || !g) return response.untyped(notFound());
      const ids = (await request.json()).runners;
      for (const r of runners) {
        if (r.scope !== "org") continue;
        if (ids.includes(r.id)) r.runner_group_id = g.id;
        else if (r.runner_group_id === g.id) r.runner_group_id = 1;
      }
      return response(204).empty();
    },
  ),
];
