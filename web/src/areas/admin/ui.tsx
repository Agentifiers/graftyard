import { useState, type ReactNode } from "react";
import {
  createColumnHelper,
  filterFn_includesString,
  useTable,
  type ColumnDef,
} from "@tanstack/react-table";
import { useMutation, useQueryClient, type QueryKey } from "@tanstack/react-query";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { Search01Icon } from "@hugeicons/core-free-icons";
import { ApiError } from "@/api/client";
import {
  DataGrid,
  DataGridContainer,
  dataGridFeatures,
  type DataGridFeatures,
} from "@/components/reui/data-grid/data-grid";
import { DataGridPagination } from "@/components/reui/data-grid/data-grid-pagination";
import { DataGridTable } from "@/components/reui/data-grid/data-grid-table";
import { Badge, type BadgeProps } from "@/components/reui/badge";
import type { AnyFieldApi, AnyFormApi } from "@tanstack/react-form";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";

// Pieces every admin page shares: the searchable grid, the write helper, and the
// mapping from a GHES 422 to the form field it names.

export const columns = <T extends object>() => createColumnHelper<DataGridFeatures, T>();

// A searchable, sortable, paginated grid over rows already in memory. `search` is the
// text filter, owned by the page so it can live in the URL.
export function Grid<T extends object>({
  rows,
  cols,
  rowId,
  search,
  onSearch,
  placeholder,
  empty,
  toolbar,
  pageSize = 10,
  onRowClick,
}: {
  rows: T[];
  cols: ColumnDef<DataGridFeatures, T, any>[];
  rowId: (row: T) => string;
  search: string;
  onSearch: (q: string) => void;
  placeholder: string;
  empty: ReactNode;
  toolbar?: ReactNode;
  pageSize?: number;
  onRowClick?: (row: T) => void;
}) {
  const table = useTable({
    features: dataGridFeatures,
    columns: cols,
    data: rows,
    getRowId: rowId,
    state: { globalFilter: search },
    onGlobalFilterChange: (v) => onSearch(typeof v === "function" ? v(search) : String(v ?? "")),
    globalFilterFn: filterFn_includesString,
    initialState: { pagination: { pageIndex: 0, pageSize } },
  });
  const count = table.getFilteredRowModel().rows.length;
  return (
    <DataGrid
      table={table}
      recordCount={count}
      emptyMessage={empty}
      onRowClick={onRowClick}
      tableLayout={{ dense: true, headerBackground: true, width: "auto" }}
    >
      <div className="flex flex-col gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <label className="relative w-full max-w-xs">
            <HugeiconsIcon
              icon={Search01Icon}
              strokeWidth={2}
              className="text-muted-foreground absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2"
            />
            <Input
              type="search"
              aria-label={placeholder}
              placeholder={placeholder}
              className="rounded-sm pl-8"
              value={search}
              onChange={(e) => onSearch(e.target.value)}
            />
          </label>
          <span className="text-muted-foreground text-xs tabular-nums" data-testid="grid-count">
            {count} of {rows.length}
          </span>
          {toolbar && <div className="ml-auto flex flex-wrap items-center gap-2">{toolbar}</div>}
        </div>
        <DataGridContainer className="rounded-sm border">
          <DataGridTable />
        </DataGridContainer>
        {count > pageSize && <DataGridPagination sizes={[10, 25, 50]} />}
      </div>
    </DataGrid>
  );
}

// A write: no retry, a toast on failure, the reads it changed invalidated after.
// `onError` lets the caller roll back or map a 422 onto a form.
export function useWrite<TVars, TData>({
  run,
  invalidate,
  success,
  onSuccess,
  onError,
}: {
  run: (vars: TVars) => Promise<TData>;
  invalidate: QueryKey[];
  success?: string | ((data: TData, vars: TVars) => string);
  onSuccess?: (data: TData, vars: TVars) => void;
  onError?: (error: unknown, vars: TVars) => boolean | void;
}) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: run,
    retry: 0,
    onSuccess: (data, vars) => {
      if (success) toast.success(typeof success === "function" ? success(data, vars) : success);
      onSuccess?.(data, vars);
    },
    onError: (error, vars) => {
      // A handled error (a 422 shown on its field) needs no toast.
      if (onError?.(error, vars) === true) return;
      toast.error(message(error));
    },
    onSettled: () =>
      Promise.all(invalidate.map((queryKey) => queryClient.invalidateQueries({ queryKey }))),
  });
}

// A 422's own message is only "Validation Failed"; the reason is in its errors.
export const message = (e: unknown) =>
  e instanceof FieldsError && e.fields.length
    ? e.fields.map((f) => f.message).join(". ")
    : e instanceof Error
      ? e.message
      : String(e);

// GHES 422 bodies name the field and a code; this turns them into readable messages.
type FieldError = { field: string; message: string };
const CODES: Record<string, string> = {
  missing_field: "is required",
  invalid: "is not valid",
  already_exists: "is already taken",
  custom: "",
};

// openapi-fetch has already parsed the error body, so this reads that, not the Response.
function fieldErrors(body: unknown): FieldError[] {
  if (!body || typeof body !== "object" || !("errors" in body) || !Array.isArray(body.errors))
    return [];
  return body.errors.flatMap((e: unknown) => {
    if (!e || typeof e !== "object") return [];
    const r = e as Record<string, unknown>;
    const field = typeof r.field === "string" ? r.field : "";
    const text =
      typeof r.message === "string"
        ? r.message
        : `${field.replace(/_/g, " ")} ${CODES[String(r.code)] ?? String(r.code)}`;
    return [{ field, message: text }];
  });
}

// An ApiError that also carries the 422's field errors, so forms can show them.
export class FieldsError extends ApiError {
  readonly fields: FieldError[];
  constructor(status: number, body: unknown, fields: FieldError[]) {
    super(status, body);
    this.fields = fields;
  }
}

type Result<T> = { data?: T; error?: unknown; response: Response };

// unwrap, plus the field errors of a 422.
export async function send<T>(call: Promise<Result<T>>): Promise<T> {
  const { data, error, response } = await call;
  if (!response.ok || error !== undefined) {
    throw new FieldsError(
      response.status,
      error,
      response.status === 422 ? fieldErrors(error) : [],
    );
  }
  return data as T;
}

// Puts each 422 field error on the form field it names (`fields` maps the wire field to
// the form's), or returns false when none matched.
export function showFieldErrors(error: unknown, form: AnyFormApi, fields: Record<string, string>) {
  if (!(error instanceof FieldsError) || !error.fields.length) return false;
  let shown = false;
  for (const f of error.fields) {
    const name = fields[f.field];
    if (!name) continue;
    shown = true;
    form.setFieldMeta(name, (m) => ({ ...m, errorMap: { ...m.errorMap, onServer: f.message } }));
  }
  return shown;
}

// A labelled text input bound to a form field. Editing it clears its server error.
export function TextField({
  field,
  label,
  description,
  type = "text",
  placeholder,
  multiline,
  className,
}: {
  field: AnyFieldApi;
  label: string;
  description?: ReactNode;
  type?: string;
  placeholder?: string;
  multiline?: boolean;
  className?: string;
}) {
  const errors = field.state.meta.errors.filter(Boolean).map(String);
  const props = {
    id: `f-${field.name}`,
    name: field.name,
    placeholder,
    value: String(field.state.value ?? ""),
    "aria-invalid": errors.length > 0 || undefined,
    onBlur: field.handleBlur,
    onChange: (e: { target: { value: string } }) => {
      field.setMeta((m) => ({ ...m, errorMap: { ...m.errorMap, onServer: undefined } }));
      field.handleChange(e.target.value);
    },
  };
  return (
    <Field data-invalid={errors.length > 0 || undefined} className={className}>
      <FieldLabel htmlFor={props.id}>{label}</FieldLabel>
      {multiline ? (
        <Textarea {...props} className="min-h-28 rounded-sm font-mono text-xs" />
      ) : (
        <Input {...props} type={type} className="rounded-sm" />
      )}
      {description && <FieldDescription>{description}</FieldDescription>}
      {errors.length > 0 && <FieldError>{errors.join(". ")}</FieldError>}
    </Field>
  );
}

// A state word with its badge colour.
export function StateBadge({
  tone,
  children,
}: {
  tone: BadgeProps["variant"];
  children: ReactNode;
}) {
  return (
    <Badge variant={tone} size="sm" className="capitalize">
      {children}
    </Badge>
  );
}

// One confirmation word the user must type before a destructive action.
export function useTyped(word: string) {
  const [value, setValue] = useState("");
  return { value, setValue, ok: value === word };
}

export const when = (at: string | null | undefined) =>
  at
    ? new Date(at).toLocaleString("en-GB", {
        dateStyle: "medium",
        timeStyle: "short",
        timeZone: "UTC",
      }) + " UTC"
    : "Never";
