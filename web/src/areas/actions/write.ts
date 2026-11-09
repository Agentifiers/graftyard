import { useMutation, useQueryClient, type QueryKey } from "@tanstack/react-query";
import { toast } from "sonner";
import { ApiError } from "@/api/client";

// The 422 body GHES sends, read into a field to message map for a form.
export type FieldErrors = Record<string, string>;

const CODES: Record<string, string> = {
  missing_field: "is required",
  invalid: "is not valid",
  already_exists: "already exists",
  reserved: "uses a reserved prefix",
  too_large: "is too large",
  unexpected: "is not declared by this workflow",
  disabled: "is disabled",
  missing_workflow_dispatch: "has no workflow_dispatch trigger",
  default_group: "is the default group",
};

export class ValidationError extends ApiError {
  readonly fields: FieldErrors;
  constructor(body: unknown) {
    super(422, body);
    const errors =
      body && typeof body === "object" && "errors" in body && Array.isArray(body.errors)
        ? body.errors
        : [];
    this.fields = Object.fromEntries(
      errors
        .filter(
          (e): e is { field: string; code: string } => typeof e === "object" && e && "field" in e,
        )
        .map((e) => [e.field, `${e.field.replace(/^inputs\./, "")} ${CODES[e.code] ?? e.code}`]),
    );
    const simple = errors.filter((e): e is string => typeof e === "string");
    if (simple.length) this.message = simple.join(" ");
  }
}

type Result<T> = { data?: T; error?: unknown; response: Response };

// unwrap for writes: a 422 becomes a ValidationError that carries its fields.
export async function send<T>(call: Promise<Result<T>>): Promise<T> {
  const { data, error, response } = await call;
  if (response.status === 422) throw new ValidationError(error);
  if (!response.ok) throw new ApiError(response.status, error);
  return data as T;
}

// One write: never retried; on failure it rolls back what `optimistic` changed and toasts;
// on settle it refetches the reads it changed.
export function useWrite<TVars, TResult = unknown>({
  run,
  invalidates,
  optimistic,
  success,
  failure,
}: {
  run: (vars: TVars) => Promise<TResult>;
  invalidates: (vars: TVars) => QueryKey[];
  // Applies the change to the cache at once and returns how to undo it.
  optimistic?: (vars: TVars) => (() => void) | void;
  success?: (vars: TVars, result: TResult) => string | void;
  failure: string | ((vars: TVars) => string);
}) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: run,
    onMutate: async (vars: TVars) => {
      await Promise.all(
        invalidates(vars).map((queryKey) => queryClient.cancelQueries({ queryKey })),
      );
      return { undo: optimistic?.(vars) ?? undefined };
    },
    onError: (error, vars, context) => {
      context?.undo?.();
      if (error instanceof ValidationError && Object.keys(error.fields).length) return;
      toast.error(typeof failure === "string" ? failure : failure(vars), {
        description: error instanceof Error ? error.message : String(error),
      });
    },
    onSuccess: (result, vars) => {
      const message = success?.(vars, result);
      if (message) toast.success(message);
    },
    onSettled: (_r, _e, vars) =>
      Promise.all(invalidates(vars).map((queryKey) => queryClient.invalidateQueries({ queryKey }))),
  });
}

// The message for one form field from the last failed submit.
export const fieldError = (error: unknown, field: string) =>
  error instanceof ValidationError ? error.fields[field] : undefined;
