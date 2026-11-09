import { HttpResponse, ws } from "msw";
import { createOpenApiHttp } from "openapi-msw";
import type { Schemas } from "@/api/client";
import {
  BLOB_BASE,
  UI_BASE,
  relayPath,
  type BlobPaths,
  type RelayMessage,
  type UiPaths,
} from "@/areas/actions/ui-api";
import { failing } from "../faults";
import { gate, http, notFound, page, unprocessable } from "../http";
import { branches, sha } from "../world";
import { actions, annotationsFor, environments, type JobRec, type RunRec } from "./actions-db";

// Workflows, runs, jobs, logs, artifacts, and deployment reviews; the private UI reads
// the run page needs; the job log relay; and the signed blob URLs GHES redirects to.

const ui = createOpenApiHttp<UiPaths>({ baseUrl: UI_BASE });
const blobs = createOpenApiHttp<BlobPaths>({ baseUrl: BLOB_BASE });

const full = (p: { owner: string; repo: string }) => `${p.owner}/${p.repo}`.toLowerCase();
const known = (p: { owner: string; repo: string }) => full(p) in branches;

const forbidden = (message: string) =>
  HttpResponse.json(
    {
      message,
      documentation_url: "https://docs.github.com/enterprise-server@3.22/rest/actions",
      status: "403",
    },
    { status: 403 },
  );

const conflict = (message: string) =>
  HttpResponse.json(
    {
      message,
      documentation_url: "https://docs.github.com/enterprise-server@3.22/rest/actions",
      status: "409",
    },
    { status: 409 },
  );

type Query = { get(name: string): string | null };

// GHES's run filters. `status` takes a status or a conclusion.
function filterRuns(list: RunRec[], q: Query, workflowId?: number) {
  const actor = q.get("actor");
  const branch = q.get("branch");
  const event = q.get("event");
  const status = q.get("status");
  const headSha = q.get("head_sha");
  return list.filter(({ run }) => {
    if (workflowId !== undefined && run.workflow_id !== workflowId) return false;
    if (actor && run.actor?.login !== actor) return false;
    if (branch && run.head_branch !== branch) return false;
    if (event && run.event !== event) return false;
    if (headSha && run.head_sha !== headSha) return false;
    if (status && run.status !== status && run.conclusion !== status) return false;
    return true;
  });
}

const signed = (path: string) =>
  `${BLOB_BASE}${path}?sig=${sha(`${path}:${Date.now()}`).slice(0, 24)}`;

// A stored (uncompressed) zip with one file, so an artifact download is a real archive.
function zip(name: string, text: string) {
  const data = new TextEncoder().encode(text);
  const file = new TextEncoder().encode(name);
  let crc = 0xffffffff;
  for (const b of data) {
    crc ^= b;
    for (let k = 0; k < 8; k++) crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
  }
  crc = (crc ^ 0xffffffff) >>> 0;
  const local = new DataView(new ArrayBuffer(30));
  [
    [0, 0x04034b50, 4],
    [4, 20, 2],
    [14, crc, 4],
    [18, data.length, 4],
    [22, data.length, 4],
    [26, file.length, 2],
  ].forEach(([at, v, size]) =>
    size === 4 ? local.setUint32(at, v, true) : local.setUint16(at, v, true),
  );
  const central = new DataView(new ArrayBuffer(46));
  [
    [0, 0x02014b50, 4],
    [4, 20, 2],
    [6, 20, 2],
    [16, crc, 4],
    [20, data.length, 4],
    [24, data.length, 4],
    [28, file.length, 2],
  ].forEach(([at, v, size]) =>
    size === 4 ? central.setUint32(at, v, true) : central.setUint16(at, v, true),
  );
  const end = new DataView(new ArrayBuffer(22));
  const centralAt = 30 + file.length + data.length;
  [
    [0, 0x06054b50, 4],
    [8, 1, 2],
    [10, 1, 2],
    [12, 46 + file.length, 4],
    [16, centralAt, 4],
  ].forEach(([at, v, size]) =>
    size === 4 ? end.setUint32(at, v, true) : end.setUint16(at, v, true),
  );
  return new Blob([local, file, data, central, file, end], { type: "application/zip" });
}

function graph(rec: RunRec) {
  return {
    jobs: actions.jobs(rec).map((j) => ({
      name: j.spec.name,
      needs: (j.spec.needs ?? []).map((k) => rec.spec.jobs.find((s) => s.key === k)!.name),
      job_id: j.job.id,
      environment: j.spec.environment ?? null,
    })),
  };
}

const jobsBody = (jobs: JobRec[], q: Query) => ({
  total_count: jobs.length,
  jobs: page(
    jobs.map((j) => j.job),
    q as { get(name: "per_page" | "page"): string | null },
  ),
});

export const handlers = [
  // ---- Workflows ----

  http.get("/repos/{owner}/{repo}/actions/workflows", async ({ params, query, response }) => {
    const fault = await gate("actions-workflows");
    if (fault) return response.untyped(fault);
    if (!known(params)) return response.untyped(notFound());
    const list = actions.workflows(full(params)).map((w) => w.workflow);
    return response(200).json({ total_count: list.length, workflows: page(list, query) });
  }),

  http.get(
    "/repos/{owner}/{repo}/actions/workflows/{workflow_id}",
    async ({ params, response }) => {
      const fault = await gate("actions-workflow");
      if (fault) return response.untyped(fault);
      const w = actions.workflow(full(params), params.workflow_id);
      return w ? response(200).json(w.workflow) : response.untyped(notFound());
    },
  ),

  http.put(
    "/repos/{owner}/{repo}/actions/workflows/{workflow_id}/enable",
    async ({ params, response }) => {
      const fault = await gate("actions-workflow-state");
      if (fault) return response.untyped(fault);
      return actions.setWorkflowState(full(params), params.workflow_id, "active")
        ? response(204).empty()
        : response.untyped(notFound());
    },
  ),

  http.put(
    "/repos/{owner}/{repo}/actions/workflows/{workflow_id}/disable",
    async ({ params, response }) => {
      const fault = await gate("actions-workflow-state");
      if (fault) return response.untyped(fault);
      return actions.setWorkflowState(full(params), params.workflow_id, "disabled_manually")
        ? response(204).empty()
        : response.untyped(notFound());
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/actions/workflows/{workflow_id}/dispatches",
    async ({ params, request, response }) => {
      const fault = await gate("actions-dispatch");
      if (fault) return response.untyped(fault);
      const repo = full(params);
      const w = actions.workflow(repo, params.workflow_id);
      if (!w) return response.untyped(notFound());
      const body = await request.json();
      if (w.workflow.state !== "active")
        return response.untyped(unprocessable("Workflow", "state", "disabled"));
      if (!w.spec.dispatch)
        return response.untyped(unprocessable("Workflow", "on", "missing_workflow_dispatch"));
      const ref = body.ref.replace(/^refs\/heads\//, "");
      if (!branches[repo].includes(ref))
        return response.untyped(unprocessable("Workflow", "ref", "invalid"));
      const given = body.inputs ?? {};
      const declared = new Map(w.spec.dispatch.map((d) => [d.name, d.input]));
      for (const name of Object.keys(given))
        if (!declared.has(name))
          return response.untyped(unprocessable("Workflow", `inputs.${name}`, "unexpected"));
      const inputs: Record<string, string | boolean> = {};
      for (const [name, input] of declared) {
        const raw = given[name];
        const value = raw === undefined || raw === "" ? input.default : raw;
        if (value === undefined || value === "") {
          if (input.required)
            return response.untyped(unprocessable("Workflow", `inputs.${name}`, "missing_field"));
          continue;
        }
        if (input.type === "boolean") {
          if (value !== true && value !== false && value !== "true" && value !== "false")
            return response.untyped(unprocessable("Workflow", `inputs.${name}`, "invalid"));
          inputs[name] = value === true || value === "true";
          continue;
        }
        const text = String(value);
        if (input.type === "choice" && !input.options.includes(text))
          return response.untyped(unprocessable("Workflow", `inputs.${name}`, "invalid"));
        if (input.type === "environment" && !environments(repo).some((e) => e.name === text))
          return response.untyped(unprocessable("Workflow", `inputs.${name}`, "invalid"));
        inputs[name] = text;
      }
      const rejected = w.spec.checkInputs?.(repo, inputs);
      if (rejected)
        return response.untyped(unprocessable("Workflow", rejected.field, rejected.code));
      const rec = actions.dispatch(repo, w.spec.file, ref, inputs);
      if (!body.return_run_details) return response(204).empty();
      return response(200).json({
        workflow_run_id: rec.run.id,
        run_url: rec.run.url,
        html_url: rec.run.html_url,
      });
    },
  ),

  // ---- Runs ----

  http.get("/repos/{owner}/{repo}/actions/runs", async ({ params, query, response }) => {
    const fault = await gate("actions-runs");
    if (fault) return response.untyped(fault);
    if (!known(params)) return response.untyped(notFound());
    const list = filterRuns(actions.runs(full(params)), query);
    return response(200).json({
      total_count: list.length,
      workflow_runs: page(
        list.map((r) => r.run),
        query,
      ),
    });
  }),

  http.get(
    "/repos/{owner}/{repo}/actions/workflows/{workflow_id}/runs",
    async ({ params, query, response }) => {
      const fault = await gate("actions-runs");
      if (fault) return response.untyped(fault);
      const w = actions.workflow(full(params), params.workflow_id);
      if (!w) return response.untyped(notFound());
      const list = filterRuns(actions.runs(full(params)), query, w.workflow.id);
      return response(200).json({
        total_count: list.length,
        workflow_runs: page(
          list.map((r) => r.run),
          query,
        ),
      });
    },
  ),

  http.get("/repos/{owner}/{repo}/actions/runs/{run_id}", async ({ params, response }) => {
    const fault = await gate("actions-run");
    if (fault) return response.untyped(fault);
    const rec = actions.run(full(params), Number(params.run_id));
    return rec ? response(200).json(rec.run) : response.untyped(notFound());
  }),

  http.get(
    "/repos/{owner}/{repo}/actions/runs/{run_id}/attempts/{attempt_number}",
    async ({ params, response }) => {
      const fault = await gate("actions-run");
      if (fault) return response.untyped(fault);
      const rec = actions.run(full(params), Number(params.run_id));
      const run = rec && actions.attemptRun(rec, Number(params.attempt_number));
      return run ? response(200).json(run) : response.untyped(notFound());
    },
  ),

  http.delete("/repos/{owner}/{repo}/actions/runs/{run_id}", async ({ params, response }) => {
    const fault = await gate("actions-delete-run");
    if (fault) return response.untyped(fault);
    const rec = actions.run(full(params), Number(params.run_id));
    if (!rec) return response.untyped(notFound());
    if (rec.run.status !== "completed")
      return response.untyped(forbidden("Cannot delete a workflow run that is not completed."));
    actions.deleteRun(rec);
    return response(204).empty();
  }),

  http.post("/repos/{owner}/{repo}/actions/runs/{run_id}/cancel", async ({ params, response }) => {
    const fault = await gate("actions-cancel");
    if (fault) return response.untyped(fault);
    const rec = actions.run(full(params), Number(params.run_id));
    if (!rec) return response.untyped(notFound());
    if (rec.run.status === "completed")
      return response.untyped(conflict("Cannot cancel a workflow run that is completed."));
    actions.cancel(rec);
    return response(202).json({});
  }),

  http.post("/repos/{owner}/{repo}/actions/runs/{run_id}/rerun", async ({ params, response }) => {
    const fault = await gate("actions-rerun");
    if (fault) return response.untyped(fault);
    const rec = actions.run(full(params), Number(params.run_id));
    if (!rec) return response.untyped(notFound());
    if (rec.run.status !== "completed")
      return response.untyped(forbidden("This workflow is already running."));
    actions.rerun(rec);
    return response(201).json({});
  }),

  http.post(
    "/repos/{owner}/{repo}/actions/runs/{run_id}/rerun-failed-jobs",
    async ({ params, response }) => {
      const fault = await gate("actions-rerun");
      if (fault) return response.untyped(fault);
      const rec = actions.run(full(params), Number(params.run_id));
      if (!rec) return response.untyped(notFound());
      if (rec.run.status !== "completed")
        return response.untyped(forbidden("This workflow is already running."));
      const failed = actions.failedKeys(rec);
      if (failed.size === 0)
        return response.untyped(forbidden("This workflow run has no failed jobs."));
      actions.rerun(rec, failed);
      return response(201).json({});
    },
  ),

  http.post("/repos/{owner}/{repo}/actions/jobs/{job_id}/rerun", async ({ params, response }) => {
    const fault = await gate("actions-rerun");
    if (fault) return response.untyped(fault);
    const j = actions.job(Number(params.job_id));
    if (!j || j.run.full !== full(params)) return response.untyped(notFound());
    if (j.run.run.status !== "completed")
      return response.untyped(forbidden("This workflow is already running."));
    actions.rerun(j.run, new Set([j.spec.key]));
    return response(201).json({});
  }),

  // ---- Jobs and logs ----

  http.get(
    "/repos/{owner}/{repo}/actions/runs/{run_id}/jobs",
    async ({ params, query, response }) => {
      const fault = await gate("actions-jobs");
      if (fault) return response.untyped(fault);
      const rec = actions.run(full(params), Number(params.run_id));
      if (!rec) return response.untyped(notFound());
      const jobs = query.get("filter") === "all" ? rec.attempts.flat() : actions.jobs(rec);
      return response(200).json(jobsBody(jobs, query));
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/actions/runs/{run_id}/attempts/{attempt_number}/jobs",
    async ({ params, query, response }) => {
      const fault = await gate("actions-jobs");
      if (fault) return response.untyped(fault);
      const rec = actions.run(full(params), Number(params.run_id));
      const attempt = Number(params.attempt_number);
      if (!rec || attempt < 1 || attempt > rec.attempts.length) return response.untyped(notFound());
      return response(200).json(jobsBody(actions.jobs(rec, attempt), query));
    },
  ),

  http.get("/repos/{owner}/{repo}/actions/jobs/{job_id}", async ({ params, response }) => {
    const fault = await gate("actions-jobs");
    if (fault) return response.untyped(fault);
    const j = actions.job(Number(params.job_id));
    return j && j.run.full === full(params)
      ? response(200).json(j.job)
      : response.untyped(notFound());
  }),

  // GHES answers 302 to a short-lived blob URL holding the job's whole log so far.
  http.get("/repos/{owner}/{repo}/actions/jobs/{job_id}/logs", async ({ params, response }) => {
    const fault = await gate("actions-logs");
    if (fault) return response.untyped(fault);
    const j = actions.job(Number(params.job_id));
    if (!j || j.run.full !== full(params)) return response.untyped(notFound());
    return response(302).empty({ headers: { Location: signed(`/actions/jobs/${j.job.id}/logs`) } });
  }),

  blobs.get("/actions/jobs/{job_id}/logs", ({ params, response }) => {
    const j = actions.job(Number(params.job_id));
    if (!j) return response.untyped(notFound());
    return response(200).text(actions.logText(j));
  }),

  // ---- Annotations ----

  // GHES serves a job's annotations as its check run's. The check-run routes belong to the
  // pulls area; this answers only for check runs Actions created and lets every other id
  // fall through to the next handler.
  http.get(
    "/repos/{owner}/{repo}/check-runs/{check_run_id}/annotations",
    async ({ params, query, response }) => {
      const list = annotationsFor(Number(params.check_run_id));
      if (!list) return;
      const fault = await gate("actions-annotations");
      if (fault) return response.untyped(fault);
      return response(200).json(page(list, query));
    },
  ),

  // ---- Artifacts ----

  http.get("/repos/{owner}/{repo}/actions/artifacts", async ({ params, query, response }) => {
    const fault = await gate("actions-artifacts");
    if (fault) return response.untyped(fault);
    if (!known(params)) return response.untyped(notFound());
    const name = query.get("name");
    const list = actions.artifacts(full(params)).filter((a) => !name || a.name === name);
    return response(200).json({ total_count: list.length, artifacts: page(list, query) });
  }),

  http.get(
    "/repos/{owner}/{repo}/actions/runs/{run_id}/artifacts",
    async ({ params, query, response }) => {
      const fault = await gate("actions-artifacts");
      if (fault) return response.untyped(fault);
      const rec = actions.run(full(params), Number(params.run_id));
      if (!rec) return response.untyped(notFound());
      const name = query.get("name");
      const list = rec.artifacts.filter((a) => !name || a.name === name);
      return response(200).json({ total_count: list.length, artifacts: page(list, query) });
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/actions/artifacts/{artifact_id}",
    async ({ params, response }) => {
      const fault = await gate("actions-artifacts");
      if (fault) return response.untyped(fault);
      const a = actions.artifact(Number(params.artifact_id));
      return a ? response(200).json(a) : response.untyped(notFound());
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/actions/artifacts/{artifact_id}",
    async ({ params, response }) => {
      const fault = await gate("actions-artifact-delete");
      if (fault) return response.untyped(fault);
      if (!actions.artifact(Number(params.artifact_id))) return response.untyped(notFound());
      actions.deleteArtifact(full(params), Number(params.artifact_id));
      return response(204).empty();
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/actions/artifacts/{artifact_id}/{archive_format}",
    async ({ params, response }) => {
      const fault = await gate("actions-artifacts");
      if (fault) return response.untyped(fault);
      const a = actions.artifact(Number(params.artifact_id));
      if (!a || params.archive_format !== "zip") return response.untyped(notFound());
      if (a.expired)
        return response.untyped(
          HttpResponse.json({ message: "Artifact has expired" }, { status: 410 }),
        );
      return response(302).empty({
        headers: { Location: signed(`/actions/artifacts/${a.id}/zip`) },
      });
    },
  ),

  blobs.untyped.get(`${BLOB_BASE}/actions/artifacts/:artifact_id/zip`, ({ params }) => {
    const a = actions.artifact(Number(params.artifact_id));
    if (!a) return notFound();
    const body = zip(
      `${a.name}.txt`,
      `${a.name}\nRun ${a.workflow_run?.id} on ${a.workflow_run?.head_branch} at ${a.workflow_run?.head_sha}\n${a.size_in_bytes} bytes before compression.\n`,
    );
    return new HttpResponse(body, {
      headers: {
        "content-type": "application/zip",
        "content-disposition": `attachment; filename="${a.name}.zip"`,
      },
    });
  }),

  // ---- Deployment reviews ----

  http.get(
    "/repos/{owner}/{repo}/actions/runs/{run_id}/pending_deployments",
    async ({ params, response }) => {
      const fault = await gate("actions-pending");
      if (fault) return response.untyped(fault);
      const rec = actions.run(full(params), Number(params.run_id));
      return rec ? response(200).json(actions.pendingFor(rec)) : response.untyped(notFound());
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/actions/runs/{run_id}/pending_deployments",
    async ({ params, request, response }) => {
      const fault = await gate("actions-review");
      if (fault) return response.untyped(fault);
      const rec = actions.run(full(params), Number(params.run_id));
      if (!rec) return response.untyped(notFound());
      const body = await request.json();
      if (body.environment_ids.length === 0)
        return response.untyped(
          unprocessable("PendingDeployment", "environment_ids", "missing_field"),
        );
      const unknown = body.environment_ids.filter(
        (id) => !rec.pending.some((p) => p.env.id === id),
      );
      if (unknown.length)
        return response.untyped(unprocessable("PendingDeployment", "environment_ids", "invalid"));
      if (body.state === "rejected" && !body.comment.trim())
        return response.untyped(unprocessable("PendingDeployment", "comment", "missing_field"));
      const pending = rec.pending.filter((p) => body.environment_ids.includes(p.env.id));
      const deployments: Schemas["deployment"][] = pending.map((p) => actions.deployment(rec, p));
      actions.review(rec, body.environment_ids, body.state, body.comment);
      return response(200).json(deployments);
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/actions/runs/{run_id}/approvals",
    async ({ params, response }) => {
      const fault = await gate("actions-pending");
      if (fault) return response.untyped(fault);
      const rec = actions.run(full(params), Number(params.run_id));
      return rec ? response(200).json(rec.approvals) : response.untyped(notFound());
    },
  ),

  // ---- Graftyard's UI API ----

  ui.get(
    "/repos/{owner}/{repo}/actions/workflows/{workflow_id}/source",
    async ({ params, response }) => {
      const fault = await gate("actions-workflow-source");
      if (fault) return response.untyped(fault);
      const w = actions.workflow(full(params), params.workflow_id);
      if (!w) return response.untyped(notFound());
      return response(200).json({
        path: w.workflow.path,
        events: w.spec.events,
        dispatch: w.spec.dispatch,
        source: w.spec.source,
      });
    },
  ),

  ui.get("/repos/{owner}/{repo}/actions/runs/{run_id}/graph", async ({ params, response }) => {
    const fault = await gate("actions-graph");
    if (fault) return response.untyped(fault);
    const rec = actions.run(full(params), Number(params.run_id));
    return rec ? response(200).json(graph(rec)) : response.untyped(notFound());
  }),

  // ---- The job's LogRelay ----

  // A viewer joins the job's relay. With a runner attached, it replays a short tail and
  // then every line as the runner sends it. Without one (the job is not running here,
  // or the relay is failed from the Mock panel), it says so and closes, and the viewer
  // reads the step log blobs instead.
  ws
    .link(`${location.origin.replace(/^http/, "ws")}${relayPath(":jobId")}`)
    .addEventListener("connection", ({ client, params }) => {
      const send = (m: RelayMessage) => client.send(JSON.stringify(m));
      const j = actions.job(Number(params.jobId));
      if (!j || !j.streaming || failing("actions-log-relay")) {
        send({ type: "no-stream" });
        client.close(1000, "no runner stream");
        return;
      }
      send({ type: "hello", job_id: j.job.id, tail: j.log.slice(-200) });
      const listener = (m: RelayMessage) => {
        send(m);
        if (m.type === "end" || m.type === "no-stream") {
          j.listeners.delete(listener);
          client.close(1000, m.type);
        }
      };
      j.listeners.add(listener);
      client.addEventListener("close", () => j.listeners.delete(listener));
    }),
];
