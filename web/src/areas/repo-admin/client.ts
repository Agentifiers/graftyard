import type { QueryKey } from "@tanstack/react-query";
import { toast } from "sonner";
import { ApiError } from "@/api/client";
import { queryClient } from "@/api/query-client";

// The shared ApiError keeps only the message, and repository admin forms need a 422's
// field errors, so this area's calls throw a subclass that carries them. It is still an
// ApiError, so the shared retry policy treats it the same.

export type FieldError = { field?: string; message: string };

export class AdminError extends ApiError {
  readonly errors: FieldError[];
  constructor(status: number, body: unknown) {
    super(status, body);
    this.errors = parseErrors(body);
    if (status === 422 && this.errors.length)
      this.message = this.errors.map((e) => e.message).join("; ");
  }
  fieldError(field: string) {
    return this.errors.find((e) => e.field === field)?.message;
  }
}

function parseErrors(body: unknown): FieldError[] {
  if (!body || typeof body !== "object" || !("errors" in body) || !Array.isArray(body.errors)) {
    return [];
  }
  return body.errors.map((e: unknown) => {
    if (typeof e === "string") return { message: e };
    const o = (e ?? {}) as { field?: string; code?: string; message?: string };
    return {
      field: o.field,
      message: o.message ?? `${o.field ?? "value"} ${o.code ?? "is invalid"}`,
    };
  });
}

type Result<T> = { data?: T; error?: unknown; response: Response };

export async function call<T>(promise: Promise<Result<T>>): Promise<T> {
  const { data, error, response } = await promise;
  if (!response.ok || error !== undefined) throw new AdminError(response.status, error);
  return data as T;
}

const messageOf = (e: unknown) => (e instanceof Error ? e.message : String(e));

// The toast every failed write shows. A write never retries, so the user decides.
export function failed(what: string) {
  return (e: unknown) => toast.error(`${what}: ${messageOf(e)}`);
}

// An optimistic cache edit for one query, with the rollback a mutation's onError calls.
export async function optimistic<T>(key: QueryKey, edit: (old: T) => T) {
  await queryClient.cancelQueries({ queryKey: key });
  const before = queryClient.getQueryData<T>(key);
  if (before !== undefined) queryClient.setQueryData<T>(key, edit(before));
  return () => queryClient.setQueryData(key, before);
}

export const refetch = (...keys: QueryKey[]) =>
  Promise.all(keys.map((queryKey) => queryClient.invalidateQueries({ queryKey })));
