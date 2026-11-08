// ?fail=checks,files makes those regions' handlers answer 503. The choice persists in
// sessionStorage so it survives client-side navigation; ?fail= (empty) clears it.

// "comments" fails listing comments; "post-comment" fails only creating one, which shows
// the optimistic comment rolling back.
export const REGIONS = [
  "user",
  "repos",
  "repo",
  "issues",
  "issue",
  "comments",
  "post-comment",
  "pulls",
  "pull",
  "files",
  "checks",
] as const;
export type FaultRegion = (typeof REGIONS)[number];

const KEY = "graftyard.fail";

function read(): Set<FaultRegion> {
  const url = new URL(window.location.href);
  const param = url.searchParams.get("fail");
  if (param !== null) sessionStorage.setItem(KEY, param);
  const raw = param ?? sessionStorage.getItem(KEY) ?? "";
  return new Set(
    raw.split(",").filter((r): r is FaultRegion => (REGIONS as readonly string[]).includes(r)),
  );
}

export function failing(region: FaultRegion): boolean {
  return read().has(region);
}

export function serviceUnavailable() {
  return {
    message: "Service Unavailable",
    documentation_url: "https://docs.github.com/enterprise-server@3.22/rest",
    status: "503",
  };
}
