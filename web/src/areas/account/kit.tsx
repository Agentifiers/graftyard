import { useState, type ComponentProps, type ReactNode } from "react";
import type { AnyFieldApi } from "@tanstack/react-form";
import { useMutation, useQueryClient, type QueryKey } from "@tanstack/react-query";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { Alert02Icon, Copy01Icon, Tick02Icon } from "@hugeicons/core-free-icons";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { ValidationError } from "./queries";

// Shared pieces of the account pages: fields bound to TanStack Form that also show a
// mock 422's field errors, the one-time secret box, and the confirm dialog.

type FormLike = { setFieldMeta(field: never, updater: never): void };

// Puts each 422 error under the form field `fields` maps its server field to. Returns
// whether any error found a field.
export function applyServerErrors(
  form: FormLike,
  error: unknown,
  fields: Record<string, string>,
): boolean {
  if (!(error instanceof ValidationError)) return false;
  let placed = false;
  for (const e of error.errors) {
    const name = e.field && fields[e.field];
    if (!name) continue;
    placed = true;
    form.setFieldMeta(
      name as never,
      ((prev: { errorMap?: object }) => ({
        ...prev,
        errorMap: { ...prev.errorMap, onServer: e.message ?? e.code },
      })) as never,
    );
  }
  return placed;
}

// The toast every failed write shows. A 422 that placed its errors on fields says so.
export function failed(what: string, error: unknown) {
  toast.error(`${what} failed`, {
    description: error instanceof Error ? error.message : String(error),
  });
}

const errorsOf = (field: AnyFieldApi) =>
  (field.state.meta.errors as unknown[])
    .filter((e): e is string => typeof e === "string" && e.length > 0)
    .map((message) => ({ message }));

// Clears a server error once the person edits the field again.
export function change(field: AnyFieldApi, value: unknown) {
  if (field.state.meta.errorMap.onServer) field.setErrorMap({ onServer: undefined });
  field.handleChange(value);
}

export function TextField({
  field,
  label,
  description,
  multiline,
  mono,
  ...props
}: {
  field: AnyFieldApi;
  label: ReactNode;
  description?: ReactNode;
  multiline?: boolean;
  mono?: boolean;
} & Omit<ComponentProps<"input">, "value" | "onChange" | "onBlur" | "name">) {
  const errors = errorsOf(field);
  const invalid = errors.length > 0;
  const id = `f-${field.name}`;
  return (
    <Field data-invalid={invalid || undefined}>
      <FieldLabel htmlFor={id}>{label}</FieldLabel>
      {multiline ? (
        <Textarea
          id={id}
          name={field.name}
          value={field.state.value ?? ""}
          onBlur={field.handleBlur}
          onChange={(e) => change(field, e.target.value)}
          aria-invalid={invalid || undefined}
          placeholder={props.placeholder}
          className={mono ? "min-h-24 font-mono text-xs" : "min-h-20"}
          spellCheck={mono ? false : undefined}
        />
      ) : (
        <Input
          id={id}
          name={field.name}
          value={field.state.value ?? ""}
          onBlur={field.handleBlur}
          onChange={(e) => change(field, e.target.value)}
          aria-invalid={invalid || undefined}
          {...props}
        />
      )}
      {description && !invalid && (
        <FieldDescription className="text-xs">{description}</FieldDescription>
      )}
      <FieldError errors={errors} className="text-xs" />
    </Field>
  );
}

export function FieldErrors({ field }: { field: AnyFieldApi }) {
  return <FieldError errors={errorsOf(field)} className="text-xs" />;
}

export function CopyButton({ value, label = "Copy" }: { value: string; label?: string }) {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  return (
    <Button
      type="button"
      variant="outline"
      size="sm"
      onClick={() => copyToClipboard(value)}
      aria-label={label}
    >
      <HugeiconsIcon
        icon={isCopied ? Tick02Icon : Copy01Icon}
        strokeWidth={2}
        data-icon="inline-start"
      />
      {isCopied ? "Copied" : label}
    </Button>
  );
}

// A secret the server returned once: a token, a client secret, recovery codes. Dismissed,
// it is gone for good; only its last characters stay on the page.
export function OneTimeSecret({
  title,
  value,
  note,
  onDismiss,
}: {
  title: string;
  value: string;
  note: ReactNode;
  onDismiss: () => void;
}) {
  return (
    <Alert variant="success" data-testid="one-time-secret">
      <HugeiconsIcon icon={Alert02Icon} strokeWidth={2} />
      <AlertTitle>{title}</AlertTitle>
      <AlertDescription className="flex flex-col gap-2">
        <span>{note}</span>
        <div className="flex flex-wrap items-center gap-2">
          <code
            className="bg-background text-foreground rounded-sm border px-2 py-1 font-mono text-xs break-all"
            data-testid="secret-value"
          >
            {value}
          </code>
          <CopyButton value={value} />
          <Button type="button" size="sm" variant="ghost" onClick={onDismiss}>
            Done
          </Button>
        </div>
      </AlertDescription>
    </Alert>
  );
}

// A destructive action behind a dialog that names what it does.
export function Confirm({
  trigger,
  title,
  description,
  action,
  onConfirm,
  pending,
  destructive = true,
}: {
  trigger: ReactNode;
  title: string;
  description: ReactNode;
  action: string;
  onConfirm: () => unknown;
  pending?: boolean;
  destructive?: boolean;
}) {
  const [open, setOpen] = useState(false);
  return (
    <AlertDialog open={open} onOpenChange={setOpen}>
      <AlertDialogTrigger render={trigger as React.ReactElement} />
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <Button
            variant={destructive ? "destructive" : "default"}
            disabled={pending}
            onClick={() => {
              setOpen(false);
              onConfirm();
            }}
          >
            {action}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

export function SubmitButton({
  pending,
  children,
  ...props
}: { pending: boolean; children: ReactNode } & ComponentProps<typeof Button>) {
  return (
    <Button type="submit" size="sm" disabled={pending} {...props}>
      {pending ? "Saving…" : children}
    </Button>
  );
}

export function EmptyRow({ children }: { children: ReactNode }) {
  return <p className="text-muted-foreground px-4 py-8 text-center text-sm">{children}</p>;
}

export function Section({
  title,
  description,
  actions,
  children,
}: {
  title: string;
  description?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="flex flex-col gap-3" data-section={title}>
      <div className="flex flex-wrap items-end gap-2 border-b pb-2">
        <div className="min-w-0 flex-1">
          <h2 className="text-base font-semibold">{title}</h2>
          {description && <p className="text-muted-foreground text-xs">{description}</p>}
        </div>
        {actions}
      </div>
      {children}
    </section>
  );
}

export function formatDate(at: string | null) {
  if (!at) return "Never";
  return new Date(at).toLocaleDateString("en", { year: "numeric", month: "short", day: "numeric" });
}

export function download(name: string, text: string, type = "application/x-pem-file") {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

// A write that shows at once: it patches the cached read, sends once (mutations never
// retry), rolls the cache back and toasts on failure, and refetches the reads it changed.
export function useOptimistic<TData, TVars, TResult>({
  queryKey,
  mutationFn,
  apply,
  what,
  invalidate = [],
  onSuccess,
}: {
  queryKey: QueryKey;
  mutationFn: (vars: TVars) => Promise<TResult>;
  apply: (old: TData, vars: TVars) => TData;
  what: string;
  invalidate?: QueryKey[];
  onSuccess?: (result: TResult, vars: TVars) => void;
}) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onMutate: async (vars: TVars) => {
      await queryClient.cancelQueries({ queryKey });
      const previous = queryClient.getQueryData<TData>(queryKey);
      if (previous !== undefined) queryClient.setQueryData<TData>(queryKey, apply(previous, vars));
      return { previous };
    },
    onError: (error, _vars, context) => {
      if (context?.previous !== undefined) queryClient.setQueryData(queryKey, context.previous);
      failed(what, error);
    },
    onSuccess,
    onSettled: () =>
      Promise.all(
        [queryKey, ...invalidate].map((key) => queryClient.invalidateQueries({ queryKey: key })),
      ),
  });
}
