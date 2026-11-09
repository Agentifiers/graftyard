import createClient from "openapi-fetch";
import { ApiError, waitForMocks, type Schemas } from "@/api/client";

// A failed write with GHES's validation errors kept by field, so a form can put each one
// under its input. `fields` is empty for a 503 or a 404.
export class WriteError extends ApiError {
  readonly fields: Record<string, string>;
  constructor(status: number, body: unknown) {
    super(status, body);
    this.fields = {};
    const errors =
      body && typeof body === "object" && "errors" in body && Array.isArray(body.errors)
        ? (body.errors as { field?: string; code?: string; message?: string }[])
        : [];
    for (const e of errors) {
      if (!e.field) continue;
      this.fields[e.field] = e.message ?? CODES[e.code ?? ""] ?? `is ${e.code}`;
    }
  }
}

const CODES: Record<string, string> = {
  already_exists: "already exists",
  invalid: "is invalid",
  missing_field: "is required",
  missing: "does not exist",
};

type Result<T> = { data?: T; error?: unknown; response: Response };

// unwrap for writes: the same contract, but a failure keeps its field errors.
export async function send<T>(call: Promise<Result<T>>): Promise<T> {
  const { data, error, response } = await call;
  if (!response.ok || error !== undefined) throw new WriteError(response.status, error);
  return data as T;
}

export const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

// A TanStack Form field-meta updater that shows a server error under the field. It sits in
// the onSubmit slot, which the form clears the next time the user edits that field.
export const serverError =
  (message: string) =>
  <M extends { errorMap: object }>(meta: M): M => ({
    ...meta,
    errorMap: { ...meta.errorMap, onSubmit: message },
  });

// Two APIs outside the generated GHES `paths`, typed by hand in openapi-typescript's shape.
//
// Uploads: GHES serves release asset uploads on its uploads host, which the description
// lists under a separate server, so the generated `paths` lack it. The app reaches it at
// /api/uploads, the path GHES serves it on for a same-origin client.
//
// The private UI API: the deployments a protected environment holds for review. GHES
// exposes those only per workflow run (Actions owns that endpoint), so the deployments
// page reads them here, and approves or rejects through deployment statuses.

export type UploadPaths = {
  "/repos/{owner}/{repo}/releases/{release_id}/assets": {
    post: {
      parameters: {
        query: { name: string; label?: string };
        path: { owner: string; repo: string; release_id: number };
      };
      requestBody: { content: { "application/octet-stream": Blob } };
      responses: {
        201: { content: { "application/json": Schemas["release-asset"] } };
        422: { content: { "application/json": Schemas["validation-error"] } };
      };
    };
  };
};

export type PendingReview = Schemas["pending-deployment"] & {
  deployment: Schemas["deployment"];
};

export type UiPaths = {
  "/repos/{owner}/{repo}/deployments/pending": {
    get: {
      parameters: { path: { owner: string; repo: string } };
      responses: { 200: { content: { "application/json": PendingReview[] } } };
    };
  };
};

export const uploads = createClient<UploadPaths>({ baseUrl: "/api/uploads" });
uploads.use(waitForMocks);

export const ui = createClient<UiPaths>({ baseUrl: "/_t/ui" });
ui.use(waitForMocks);

// Upload progress: the body goes out as a stream of 64 KB chunks, and each chunk the
// network side pulls advances the bar. XMLHttpRequest's upload events never fire while a
// service worker intercepts the request, so they cannot show progress against the mock.
// openapi-fetch takes this as the request's `fetch`.
export function fetchWithProgress(onProgress: (fraction: number) => void) {
  return async (request: Request): Promise<Response> => {
    const data = new Uint8Array(await request.arrayBuffer());
    const CHUNK = 64 * 1024;
    let sent = 0;
    const body = new ReadableStream<Uint8Array>({
      pull(controller) {
        if (sent >= data.byteLength) {
          controller.close();
          return;
        }
        controller.enqueue(data.subarray(sent, sent + CHUNK));
        sent = Math.min(data.byteLength, sent + CHUNK);
        onProgress(data.byteLength ? sent / data.byteLength : 1);
      },
    });
    if (data.byteLength === 0) onProgress(1);
    return fetch(request.url, {
      method: request.method,
      headers: request.headers,
      body,
      signal: request.signal,
      duplex: "half",
    } as RequestInit);
  };
}

// Saves a downloaded body as a file, for asset and source archive downloads.
export function saveBlob(blob: Blob, response: Response, fallbackName: string) {
  const disposition = response.headers.get("content-disposition") ?? "";
  const name = /filename="?([^";]+)"?/.exec(disposition)?.[1] ?? fallbackName;
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
  return name;
}
