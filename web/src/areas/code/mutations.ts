import { api, ApiError, type Schemas } from "@/api/client";
import { queryClient } from "@/api/query-client";
import { encodeText } from "./base64";

// The code area's writes. None retries. Each throws a WriteError that carries a 422's
// field errors, so a form can show them beside the right input.

type Repo = { owner: string; repo: string };

export type FieldErrors = Partial<Record<string, string>>;

export class WriteError extends ApiError {
  readonly fields: FieldErrors;
  constructor(status: number, body: unknown) {
    super(status, body);
    const errors =
      body && typeof body === "object" && "errors" in body && Array.isArray(body.errors)
        ? (body.errors as Schemas["validation-error"]["errors"])
        : [];
    this.fields = Object.fromEntries(
      (errors ?? []).filter((e) => e.field).map((e) => [e.field!, describe(e.field!, e.code)]),
    );
  }
}

const CODES: Record<string, string> = {
  missing_field: "is required",
  already_exists: "already exists",
  invalid: "is not valid",
  no_change: "has no changes to commit",
  not_fast_forward: "is not a fast-forward",
};

function describe(field: string, code: string) {
  return `${field.replace(/_/g, " ")} ${CODES[code] ?? code}`;
}

async function call<T>(p: Promise<{ data?: T; error?: unknown; response: Response }>): Promise<T> {
  const { data, error, response } = await p;
  if (!response.ok) throw new WriteError(response.status, error);
  return data as T;
}

// Every read under the repository's git data: contents, trees, commits, refs, stats.
export function invalidateCode({ owner, repo }: Repo) {
  return Promise.all([
    queryClient.invalidateQueries({ queryKey: ["repos", owner, repo] }),
    queryClient.invalidateQueries({ queryKey: ["_t", "repos", owner, repo] }),
  ]);
}

export function putFile(
  r: Repo,
  input: { path: string; text: string; message: string; branch: string; sha?: string },
) {
  return call(
    api.PUT("/repos/{owner}/{repo}/contents/{path}", {
      params: { path: { ...r, path: input.path } },
      body: {
        message: input.message,
        content: encodeText(input.text),
        branch: input.branch,
        ...(input.sha ? { sha: input.sha } : {}),
      },
    }),
  );
}

export function deleteFile(
  r: Repo,
  input: { path: string; sha: string; message: string; branch: string },
) {
  return call(
    api.DELETE("/repos/{owner}/{repo}/contents/{path}", {
      params: { path: { ...r, path: input.path } },
      body: { message: input.message, sha: input.sha, branch: input.branch },
    }),
  );
}

export function createBranch(r: Repo, input: { name: string; sha: string }) {
  return call(
    api.POST("/repos/{owner}/{repo}/git/refs", {
      params: { path: r },
      body: { ref: `refs/heads/${input.name}`, sha: input.sha },
    }),
  );
}

export async function deleteBranch(r: Repo, name: string) {
  const { error, response } = await api.DELETE("/repos/{owner}/{repo}/git/refs/{ref}", {
    params: { path: { ...r, ref: `heads/${name}` } },
  });
  if (!response.ok)
    throw new WriteError(response.status, error ?? { message: `Could not delete ${name}` });
}

export function renameBranch(r: Repo, input: { from: string; to: string }) {
  return call(
    api.POST("/repos/{owner}/{repo}/branches/{branch}/rename", {
      params: { path: { ...r, branch: input.from } },
      body: { new_name: input.to },
    }),
  );
}

export function createStatus(
  r: Repo,
  input: {
    sha: string;
    state: "error" | "failure" | "pending" | "success";
    context: string;
    description: string;
    target_url?: string;
  },
) {
  return call(
    api.POST("/repos/{owner}/{repo}/statuses/{sha}", {
      params: { path: { ...r, sha: input.sha } },
      body: {
        state: input.state,
        context: input.context,
        description: input.description,
        target_url: input.target_url,
      },
    }),
  );
}

export const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));
