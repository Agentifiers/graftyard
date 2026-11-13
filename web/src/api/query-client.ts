import { QueryCache, QueryClient, type QueryKey } from "@tanstack/react-query";
import { ApiError } from "./client";

// Reads retry only on a network error (no status) or a 5xx, at most 2 times each, and
// every retry spends a token from one page-wide budget, so an outage cannot multiply
// the load. Each successful query earns a tenth of a token back (design/SHARED.md,
// "How the edge serves it (M4-WP5)").
const MAX_RETRIES = 2;
const BUDGET = 10;
let tokens = BUDGET;

function retryRead(failureCount: number, error: unknown) {
  if (error instanceof ApiError && error.status < 500) return false;
  if (failureCount >= MAX_RETRIES || tokens < 1) return false;
  tokens -= 1;
  return true;
}

// Full jitter (a random delay up to 300 ms doubling, capped at 4 s), but never sooner
// than the reply's Retry-After, and never more than 10 s.
function retryDelay(attempt: number, error: unknown) {
  const jitter = Math.random() * Math.min(4000, 300 * 2 ** attempt);
  const after = error instanceof ApiError && error.retryAfter ? error.retryAfter * 1000 : 0;
  return Math.min(10_000, Math.max(jitter, after));
}

export const queryClient = new QueryClient({
  queryCache: new QueryCache({
    onSuccess: () => {
      tokens = Math.min(BUDGET, tokens + 0.1);
    },
  }),
  defaultOptions: {
    queries: { retry: retryRead, retryDelay, staleTime: 30_000 },
    mutations: { retry: 0 },
  },
});

// The edge inlines the first route's reads as
// <script id="__graft_seed" type="application/json">{"at", "queries": [[key, data]]}</script>,
// under the keys the regions' queryOptions use, so those regions render without a fetch.
// This runs at module init, before the router starts.
type Seed = { at: number; queries: [QueryKey, unknown][] };

if (typeof document !== "undefined") {
  const text = document.getElementById("__graft_seed")?.textContent;
  if (text) {
    try {
      const seed = JSON.parse(text) as Seed;
      for (const [key, data] of seed.queries)
        queryClient.setQueryData(key, data, { updatedAt: seed.at });
    } catch {
      // A broken seed only costs the first fetches.
    }
  }
}
