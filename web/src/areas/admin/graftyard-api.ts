import createClient from "openapi-fetch";
import { waitForMocks } from "@/api/client";

// The two site-admin Graftyard paths this area uses, typed exactly as
// design/AGENT-LAYER.md, "API reference", gives them. The agents area owns the full
// Graftyard client; the merge folds these into it.

export type AgentKind = "graftyard_run" | "third_party" | "mcp";

// {app_id, slug, kind, confine: pattern | null}
export type AgentAppRow = {
  app_id: number;
  slug: string;
  kind: AgentKind;
  confine: string | null;
};

// {index, entry: base64 of the signed bytes}
export type ReleaseEntry = { index: number; entry: string };

// GitHub's error body. A 422 carries errors with GitHub's codes.
export type GraftyardError = {
  message: string;
  documentation_url: string;
  errors?: {
    resource: string;
    field: string;
    code: "missing_field" | "invalid" | "already_exists";
  }[];
};

type Json<T> = { content: { "application/json": T } };
type NoBody = { content?: never };

export type GraftyardAdminPaths = {
  "/agents/apps": {
    get: { parameters: {}; responses: { 200: Json<AgentAppRow[]> } };
  };
  "/agents/apps/{app_id}": {
    put: {
      parameters: { path: { app_id: number } };
      requestBody: Json<{ kind: AgentKind; confine?: string | null }>;
      responses: { 200: Json<AgentAppRow>; 422: Json<GraftyardError> };
    };
    delete: {
      parameters: { path: { app_id: number } };
      responses: { 204: NoBody; 404: Json<GraftyardError> };
    };
  };
  "/release/entries": {
    get: {
      parameters: { query?: { after?: number; per_page?: number } };
      responses: { 200: Json<ReleaseEntry[]> };
    };
  };
};

export const GRAFTYARD_BASE = "/api/graftyard/v1";

export const graftyard = createClient<GraftyardAdminPaths>({ baseUrl: GRAFTYARD_BASE });
graftyard.use(waitForMocks);
