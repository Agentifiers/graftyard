import { HttpResponse, delay } from "msw";
import { createOpenApiHttp } from "openapi-msw";
import type { paths } from "@/api/ghes";
import { failing, serviceUnavailable } from "./faults";

// The kit every area's handlers use. `http` is typed by the full GHES 3.22 description,
// so a handler's path, params, request body, and response body must match it.
export const http = createOpenApiHttp<paths>({ baseUrl: "/api/v3" });

const DOCS = "https://docs.github.com/enterprise-server@3.22/rest";

export const notFound = () =>
  HttpResponse.json(
    { message: "Not Found", documentation_url: DOCS, status: "404" },
    { status: 404 },
  );

// GHES's validation error shape.
export const unprocessable = (resource: string, field: string, code = "invalid") =>
  HttpResponse.json(
    {
      message: "Validation Failed",
      documentation_url: DOCS,
      status: "422",
      errors: [{ resource, field, code }],
    },
    { status: 422 },
  );

// A little latency so skeletons show, then the region's fault if ?fail= names it.
export async function gate(region: string) {
  await delay(150 + Math.random() * 250);
  return failing(region) ? HttpResponse.json(serviceUnavailable(), { status: 503 }) : undefined;
}

type Query = { get(name: "per_page" | "page"): string | null };

// GHES pagination: per_page (default 30, at most 100) and 1-based page.
export function page<T>(items: T[], query: Query) {
  const perPage = Math.min(100, Number(query.get("per_page")) || 30);
  const pageNo = Math.max(1, Number(query.get("page")) || 1);
  return items.slice((pageNo - 1) * perPage, pageNo * perPage);
}

export const byUpdatedDesc = (a: { updated_at: string }, b: { updated_at: string }) =>
  b.updated_at.localeCompare(a.updated_at);
