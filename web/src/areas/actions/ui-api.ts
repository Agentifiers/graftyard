import createClient from "openapi-fetch";
import { waitForMocks } from "@/api/client";

// What GHES 3.22's REST description does not carry but the Actions pages need: a
// workflow's parsed `on:` block (dispatch inputs), a run's `needs` graph, and the job
// log relay. They are Graftyard's private UI API (`/_t/ui/*`, `design/SHARED.md`,
// "Frontend and native app") and the `LogRelay` WebSocket (`design/SHARED.md`,
// "Actions", Logs), hand-typed in openapi-typescript's shape.

// One `on.workflow_dispatch.inputs` entry, by its `type:`.
export type DispatchInput =
  | { type: "string"; description: string; required: boolean; default?: string }
  | { type: "choice"; description: string; required: boolean; default?: string; options: string[] }
  | { type: "boolean"; description: string; required: boolean; default?: boolean }
  | { type: "environment"; description: string; required: boolean; default?: string };

export type WorkflowSource = {
  path: string;
  // The events in the file's `on:` key.
  events: string[];
  // `on.workflow_dispatch.inputs` in file order, or null when the workflow has no
  // workflow_dispatch trigger.
  dispatch: { name: string; input: DispatchInput }[] | null;
  // The YAML as committed on the default branch.
  source: string;
};

export type GraphJob = {
  // The job's `name:` (matrix legs expanded), which is also the check run's name.
  name: string;
  // Names of the jobs this one `needs:`.
  needs: string[];
  // The job in the run's latest attempt.
  job_id: number;
  environment: string | null;
};

export type RunGraph = { jobs: GraphJob[] };

type Path<P> = { owner: string; repo: string } & P;
type Json<T> = { 200: { content: { "application/json": T } } };

export type UiPaths = {
  "/repos/{owner}/{repo}/actions/workflows/{workflow_id}/source": {
    get: {
      parameters: { path: Path<{ workflow_id: string }> };
      responses: Json<WorkflowSource>;
    };
  };
  "/repos/{owner}/{repo}/actions/runs/{run_id}/graph": {
    get: {
      parameters: { path: Path<{ run_id: number }> };
      responses: Json<RunGraph>;
    };
  };
};

export const UI_BASE = "/_t/ui";
export const ui = createClient<UiPaths>({ baseUrl: UI_BASE });
ui.use(waitForMocks);

// Where GHES's 302s for job logs and artifact archives point: signed blob URLs.
export type BlobPaths = {
  "/actions/jobs/{job_id}/logs": {
    get: {
      parameters: { path: { job_id: string }; query: { sig: string } };
      responses: { 200: { content: { "text/plain": string } } };
    };
  };
};

export const BLOB_BASE = "/_t/blobs";

// ---- The LogRelay WebSocket ----

// One log line: its 0-based position in the job's log, the step it belongs to (1-based,
// as in the job's `steps`), its runner timestamp, and its text with ANSI colour codes and
// `##[...]` commands intact. The saved log blob has one `<at> <text>` line per seq.
export type RelayLine = { seq: number; step: number; at: string; text: string };

// Server to viewer. A relay with a runner attached says `hello` and replays its tail;
// one without says `no-stream` and closes, and the viewer reads the step log blobs.
export type RelayMessage =
  | { type: "hello"; job_id: number; tail: RelayLine[] }
  | { type: "line"; line: RelayLine }
  | { type: "step"; number: number; status: string; conclusion: string | null }
  | { type: "end"; conclusion: string }
  | { type: "no-stream" };

export const relayPath = (jobId: number | ":jobId") => `/_t/logrelay/jobs/${jobId}`;

export function relayUrl(jobId: number) {
  const scheme = window.location.protocol === "https:" ? "wss" : "ws";
  return `${scheme}://${window.location.host}${relayPath(jobId)}`;
}
