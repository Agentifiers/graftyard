import { useState, type ReactNode } from "react";
import { useMutation, useSuspenseQuery } from "@tanstack/react-query";
import type { AnyFieldApi, AnyFormApi } from "@tanstack/react-form";
import { api, type Schemas } from "@/api/client";
import type { paths } from "@/api/ghes";
import { repoQuery } from "@/areas/core/queries";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  Autocomplete,
  AutocompleteContent,
  AutocompleteEmpty,
  AutocompleteInput,
  AutocompleteItem,
  AutocompleteList,
} from "@/components/reui/autocomplete";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { AdminError, call, failed, optimistic, refetch } from "./client";

// Pieces every repository admin page uses: a settings row, a switch that saves itself, a
// field's error line, server field errors mapped onto a form, and a typed confirmation.

export type RepoParams = { owner: string; repo: string };
type FullRepo = Schemas["full-repository"];
export type RepoPatch = NonNullable<
  paths["/repos/{owner}/{repo}"]["patch"]["requestBody"]
>["content"]["application/json"];

// One row of a settings list: what it is on the left, its control on the right.
export function SettingRow({
  title,
  description,
  control,
  children,
}: {
  title: ReactNode;
  description?: ReactNode;
  control?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-2 px-4 py-3">
      <div className="flex items-start gap-4">
        <div className="min-w-0 flex-1">
          <div className="text-sm font-medium">{title}</div>
          {description && <p className="text-muted-foreground mt-0.5 text-xs">{description}</p>}
        </div>
        {control && <div className="shrink-0">{control}</div>}
      </div>
      {children}
    </div>
  );
}

export function FieldMessage({ field }: { field: AnyFieldApi }) {
  const errors = field.state.meta.errors.filter(Boolean);
  if (!errors.length) return null;
  return (
    <p className="text-destructive text-xs" role="alert" data-field-error={field.name}>
      {errors.map(String).join(", ")}
    </p>
  );
}

// Shows a 422's field errors on the matching fields and returns what no field took. The
// error sits in the field's onSubmit slot, which TanStack Form clears on the next edit.
export function applyServerErrors(
  form: AnyFormApi,
  e: unknown,
  fields: Record<string, string>,
): string | undefined {
  if (!(e instanceof AdminError) || !e.errors.length) {
    return e instanceof Error ? e.message : String(e);
  }
  const left: string[] = [];
  for (const err of e.errors) {
    const name = err.field && fields[err.field];
    if (!name) left.push(err.message);
    else
      form.setFieldMeta(name, (m) => ({
        ...m,
        errorMap: { ...m.errorMap, onSubmit: err.message },
      }));
  }
  return left.length ? left.join("; ") : undefined;
}

// PATCH /repos/{owner}/{repo} with the change shown at once on the header and the page,
// rolled back with a toast if the server refuses.
export function useRepoPatch({ owner, repo }: RepoParams, what: string) {
  const key = repoQuery({ owner, repo }).queryKey;
  return useMutation({
    mutationFn: (body: RepoPatch) =>
      call(api.PATCH("/repos/{owner}/{repo}", { params: { path: { owner, repo } }, body })),
    onMutate: (body) =>
      optimistic<FullRepo>(key, (old) => ({
        ...old,
        ...(body as Partial<FullRepo>),
        ...(body.visibility && { private: body.visibility !== "public" }),
      })),
    onError: (e, _body, rollback) => {
      rollback?.();
      failed(what)(e);
    },
    onSettled: () => refetch(key, ["user", "repos"]),
  });
}

// A switch bound to one repository flag, saved on change.
export function RepoSwitch({
  params,
  field,
  label,
  disabled,
}: {
  params: RepoParams;
  field: keyof RepoPatch & keyof FullRepo;
  label: string;
  disabled?: boolean;
}) {
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const patch = useRepoPatch(params, `Could not change ${label.toLowerCase()}`);
  return (
    <Switch
      aria-label={label}
      data-setting={field}
      checked={Boolean(repo[field])}
      disabled={disabled || repo.archived}
      onCheckedChange={(on) => patch.mutate({ [field]: on })}
    />
  );
}

// A destructive action the user confirms by typing a phrase, such as owner/repo.
export function TypedConfirm({
  open,
  onOpenChange,
  title,
  description,
  phrase,
  action,
  pending,
  onConfirm,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: ReactNode;
  phrase: string;
  action: string;
  pending?: boolean;
  onConfirm: () => void;
  children?: ReactNode;
}) {
  const [typed, setTyped] = useState("");
  const ok = typed.trim() === phrase;
  return (
    <AlertDialog
      open={open}
      onOpenChange={(o) => {
        if (!o) setTyped("");
        onOpenChange(o);
      }}
    >
      <AlertDialogContent className="sm:max-w-md">
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription render={<div />}>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        {children}
        <form
          className="flex flex-col gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            // The dialog portals out of the DOM, but React bubbles submit through the component
            // tree, so an editor form around this dialog would otherwise save as well.
            e.stopPropagation();
            if (ok) onConfirm();
          }}
        >
          <label className="text-sm">
            To confirm, type{" "}
            <code className="bg-muted px-1 font-mono text-xs font-semibold">{phrase}</code> in the
            box below.
          </label>
          <Input
            aria-label={`Type ${phrase} to confirm`}
            autoComplete="off"
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
          />
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <Button type="submit" variant="destructive" disabled={!ok || pending}>
              {pending ? "Working…" : action}
            </Button>
          </AlertDialogFooter>
        </form>
      </AlertDialogContent>
    </AlertDialog>
  );
}

// A short notice that the repository is archived, so its settings are read-only.
export function ArchivedNotice({ params }: { params: RepoParams }) {
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  if (!repo.archived) return null;
  return (
    <p
      className="border-warning/40 bg-warning/10 text-warning-foreground dark:text-warning mb-4 rounded-sm border px-3 py-2 text-sm"
      data-testid="archived-notice"
    >
      This repository is archived. It is read-only until you unarchive it in the Danger zone.
    </p>
  );
}

export function EmptyRow({ children }: { children: ReactNode }) {
  return <p className="text-muted-foreground px-4 py-8 text-center text-sm">{children}</p>;
}

// A text box that suggests values as you type: users for an invite, check names for a
// required check. `onPick` fires on a suggestion or on Enter with free text.
export function Suggest({
  label,
  placeholder,
  items,
  onPick,
  render,
  disabled,
  allowFree,
  unavailable,
}: {
  label: string;
  placeholder?: string;
  items: string[];
  onPick: (value: string) => void;
  render?: (value: string) => ReactNode;
  disabled?: boolean;
  allowFree?: boolean;
  // Set when the suggestions failed to load; free entry still works.
  unavailable?: string;
}) {
  const [value, setValue] = useState("");
  const pick = (v: string) => {
    if (!v.trim()) return;
    onPick(v.trim());
    setValue("");
  };
  return (
    <Autocomplete
      items={items}
      value={value}
      onValueChange={(v, details) => {
        setValue(v);
        if (details.reason === "item-press") pick(v);
      }}
      openOnInputClick
    >
      <AutocompleteInput
        aria-label={label}
        placeholder={placeholder}
        disabled={disabled}
        onKeyDown={(e) => {
          if (e.key === "Enter" && allowFree && value.trim()) {
            e.preventDefault();
            pick(value);
          }
        }}
      />
      <AutocompleteContent>
        <AutocompleteEmpty>No matches.</AutocompleteEmpty>
        <AutocompleteList>
          {(item: string) => (
            <AutocompleteItem key={item} value={item}>
              {render ? render(item) : item}
            </AutocompleteItem>
          )}
        </AutocompleteList>
      </AutocompleteContent>
      {unavailable && (
        <p className="text-muted-foreground mt-1 text-xs" role="alert">
          Suggestions unavailable ({unavailable}). Type a name and press Enter.
        </p>
      )}
    </Autocomplete>
  );
}
