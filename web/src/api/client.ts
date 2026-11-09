import createClient from "openapi-fetch";
import { mocksReady } from "@/mocks/browser";
import type { components, paths } from "./ghes";

export type Schemas = components["schemas"];

// No request leaves before the mock worker (when enabled) is intercepting. Every
// openapi-fetch client in the app, including the hand-typed Graftyard ones, uses this.
export const waitForMocks = {
  async onRequest() {
    await mocksReady();
  },
};

export const api = createClient<paths>({ baseUrl: "/api/v3" });
api.use(waitForMocks);

// GHES error bodies are basic-error: { message, documentation_url, status }.
export class ApiError extends Error {
  readonly status: number;
  constructor(status: number, body: unknown) {
    const message =
      body && typeof body === "object" && "message" in body
        ? String(body.message)
        : `HTTP ${status}`;
    super(message);
    this.status = status;
  }
}

type Result<T> = { data?: T; error?: unknown; response: Response };

export async function unwrap<T>(call: Promise<Result<T>>): Promise<T> {
  const { data, error, response } = await call;
  if (!response.ok || error !== undefined) throw new ApiError(response.status, error);
  return data as T;
}
