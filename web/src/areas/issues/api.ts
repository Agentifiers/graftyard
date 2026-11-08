import { toast } from "sonner";
import { ApiError } from "@/api/client";

// A write's result. `unwrap` keeps only the message; a form also needs the 422's field
// errors, so writes in this area go through `send` instead.

type FieldError = { resource?: string; field?: string; code: string; message?: string };

export class ValidationError extends ApiError {
  readonly fields: Record<string, string>;
  constructor(status: number, body: unknown, errors: FieldError[]) {
    super(status, body);
    this.fields = Object.fromEntries(
      errors.filter((e) => e.field).map((e) => [e.field!, e.message ?? describe(e.code)]),
    );
  }
}

function describe(code: string) {
  switch (code) {
    case "missing_field":
      return "Can't be blank.";
    case "already_exists":
      return "Has already been taken.";
    case "locked":
      return "The conversation is locked.";
    default:
      return "Is invalid.";
  }
}

type Result<T> = { data?: T; error?: unknown; response: Response };

export async function send<T>(call: Promise<Result<T>>): Promise<T> {
  const { data, error, response } = await call;
  if (response.ok && error === undefined) return data as T;
  const errors =
    error && typeof error === "object" && "errors" in error && Array.isArray(error.errors)
      ? (error.errors as FieldError[])
      : [];
  if (response.status === 422 && errors.length) {
    throw new ValidationError(response.status, error, errors);
  }
  throw new ApiError(response.status, error);
}

export const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

// Every failed write says what failed, and the UI has already rolled back.
export function toastFailure(what: string, e: unknown) {
  const fields = e instanceof ValidationError ? Object.entries(e.fields) : [];
  toast.error(`${what} failed`, {
    description: fields.length
      ? fields.map(([f, m]) => `${f}: ${m}`).join(" ")
      : `${errorText(e)}. Nothing was changed.`,
  });
}
