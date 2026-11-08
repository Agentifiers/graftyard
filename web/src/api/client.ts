import createClient from "openapi-fetch";
import { mocksReady } from "@/mocks/browser";
import type { components, paths } from "./ghes";

export type Schemas = components["schemas"];

export const api = createClient<paths>({ baseUrl: "/api/v3" });

// No request leaves before the mock worker (when enabled) is intercepting.
api.use({
  async onRequest() {
    await mocksReady();
  },
});

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
