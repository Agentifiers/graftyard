import { QueryClient } from "@tanstack/react-query";
import { ApiError } from "./client";

const MAX_RETRIES = 2;

// Every query is a GET, so every query may retry. A 4xx will not change on retry.
function retryRead(failureCount: number, error: unknown) {
  if (error instanceof ApiError && error.status < 500) return false;
  return failureCount < MAX_RETRIES;
}

// Full jitter: a random delay up to an exponentially growing cap.
function jitteredDelay(attempt: number) {
  return Math.random() * Math.min(4000, 300 * 2 ** attempt);
}

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: { retry: retryRead, retryDelay: jitteredDelay, staleTime: 30_000 },
    mutations: { retry: 0 },
  },
});
