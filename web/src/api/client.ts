import createClient from "openapi-fetch";
import { mocksReady } from "@/mocks/browser";
import type { components, paths } from "./ghes";

export type Schemas = components["schemas"];

// The edge accepts the session cookie only on requests that carry this header, which a
// cross-site form cannot send without a preflight the edge never grants.
export const WEB_HEADER = { "x-graftyard-web": "1" } as const;

// No request leaves before the mock worker (when enabled) is intercepting, and every
// request carries WEB_HEADER. Every openapi-fetch client in the app, including the
// hand-typed Graftyard ones, uses this.
export const waitForMocks = {
  async onRequest({ request }: { request: Request }) {
    await mocksReady();
    for (const [name, value] of Object.entries(WEB_HEADER)) request.headers.set(name, value);
    return request;
  },
};

export const api = createClient<paths>({ baseUrl: "/api/v3" });
api.use(waitForMocks);

// GHES error bodies are basic-error: { message, documentation_url, status }.
export class ApiError extends Error {
  readonly status: number;
  // The reply's Retry-After in seconds, when it sent one.
  readonly retryAfter: number | undefined;
  constructor(status: number, body: unknown, retryAfter?: string | null) {
    const message =
      body && typeof body === "object" && "message" in body
        ? String(body.message)
        : `HTTP ${status}`;
    super(message);
    this.status = status;
    const seconds = Number(retryAfter);
    this.retryAfter = retryAfter && Number.isFinite(seconds) ? seconds : undefined;
  }
}

type Result<T> = { data?: T; error?: unknown; response: Response };

export async function unwrap<T>(call: Promise<Result<T>>): Promise<T> {
  const { data, error, response } = await call;
  if (!response.ok || error !== undefined)
    throw new ApiError(response.status, error, response.headers.get("retry-after"));
  return data as T;
}
