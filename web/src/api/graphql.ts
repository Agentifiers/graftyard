import { mocksReady } from "@/mocks/browser";
import { ApiError, WEB_HEADER } from "./client";
import persisted from "./persisted.json";

// GraphQL goes out only as persisted documents: the operation name, its variables, and
// the hash of its text in src/api/persisted.json, which the edge embeds and looks up.
// web/scripts/check-persisted.mjs checks each hash against its text.

export type Operation = keyof typeof persisted;

export type PullMergeState = {
  repository: {
    pullRequest: {
      mergeable: "MERGEABLE" | "CONFLICTING" | "UNKNOWN";
      mergeStateStatus:
        | "BEHIND"
        | "BLOCKED"
        | "CLEAN"
        | "DIRTY"
        | "DRAFT"
        | "HAS_HOOKS"
        | "UNKNOWN"
        | "UNSTABLE";
      reviewDecision: "APPROVED" | "CHANGES_REQUESTED" | "REVIEW_REQUIRED" | null;
      headRefOid: string;
    } | null;
  } | null;
};

type Results = { PullMergeState: PullMergeState };

export type PersistedRequest = {
  operationName: Operation;
  variables: Record<string, unknown>;
  extensions: { persistedQuery: { version: 1; sha256Hash: string } };
};

type Reply<T> = { data?: T | null; errors?: { message: string }[] };

export async function graphql<O extends Operation>(
  operationName: O,
  variables: Record<string, unknown>,
  signal?: AbortSignal,
): Promise<Results[O]> {
  await mocksReady();
  const body: PersistedRequest = {
    operationName,
    variables,
    extensions: { persistedQuery: { version: 1, sha256Hash: persisted[operationName].sha256 } },
  };
  const response = await fetch("/api/graphql", {
    method: "POST",
    headers: { "content-type": "application/json", accept: "application/json", ...WEB_HEADER },
    body: JSON.stringify(body),
    signal,
  });
  const reply = (await response.json().catch(() => undefined)) as Reply<Results[O]> | undefined;
  const retryAfter = response.headers.get("retry-after");
  if (!response.ok) throw new ApiError(response.status, reply, retryAfter);
  if (reply?.errors?.length)
    throw new ApiError(response.status, { message: reply.errors[0].message }, retryAfter);
  if (!reply?.data) throw new ApiError(response.status, { message: "No data" }, retryAfter);
  return reply.data;
}
