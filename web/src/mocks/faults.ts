// ?fail=<region>[,<region>] makes the mock handlers that gate on those regions answer 503.
// A region is any name a handler passes to gate(), such as "checks", "post-comment", or
// "actions-runs"; there is no fixed list, so each area names its own. The choice persists
// in sessionStorage so it survives client-side navigation; ?fail= (empty) clears it.
// The mock panel in the top bar lists every region gated so far and toggles them.

const KEY = "graftyard.fail";

function read(): Set<string> {
  const url = new URL(window.location.href);
  const param = url.searchParams.get("fail");
  if (param !== null) sessionStorage.setItem(KEY, param);
  const raw = param ?? sessionStorage.getItem(KEY) ?? "";
  return new Set(raw.split(",").filter(Boolean));
}

export function failing(region: string): boolean {
  seen(region);
  return read().has(region);
}

export function failingRegions(): Set<string> {
  return read();
}

// Replaces the failing set, in sessionStorage and in the URL's ?fail= if it has one.
export function setFailing(regions: Iterable<string>) {
  const value = [...regions].sort().join(",");
  sessionStorage.setItem(KEY, value);
  const url = new URL(window.location.href);
  if (url.searchParams.has("fail")) {
    if (value) url.searchParams.set("fail", value);
    else url.searchParams.delete("fail");
    history.replaceState(history.state, "", url);
  }
  notify();
}

const gated = new Set<string>();
const listeners = new Set<() => void>();
const notify = () => listeners.forEach((l) => l());

function seen(region: string) {
  if (gated.has(region)) return;
  gated.add(region);
  notify();
}

// Every region a handler has gated since the page loaded.
export function gatedRegions(): string[] {
  return [...gated].sort();
}

export function onFaultsChange(listener: () => void) {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

export function serviceUnavailable() {
  return {
    message: "Service Unavailable",
    documentation_url: "https://docs.github.com/enterprise-server@3.22/rest",
    status: "503",
  };
}
