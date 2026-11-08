// ?fail=<region>[,<region>] makes the mock handlers that gate on those regions answer 503.
// A region is any name a handler passes to gate(), such as "checks", "post-comment", or
// "actions-runs"; there is no fixed list, so each area names its own. The choice persists
// in sessionStorage so it survives client-side navigation; ?fail= (empty) clears it.

const KEY = "graftyard.fail";

function read(): Set<string> {
  const url = new URL(window.location.href);
  const param = url.searchParams.get("fail");
  if (param !== null) sessionStorage.setItem(KEY, param);
  const raw = param ?? sessionStorage.getItem(KEY) ?? "";
  return new Set(raw.split(",").filter(Boolean));
}

export function failing(region: string): boolean {
  return read().has(region);
}

export function serviceUnavailable() {
  return {
    message: "Service Unavailable",
    documentation_url: "https://docs.github.com/enterprise-server@3.22/rest",
    status: "503",
  };
}
