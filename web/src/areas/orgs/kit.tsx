import { useState, type ReactNode } from "react";
import type { AnyFieldApi, AnyFormApi } from "@tanstack/react-form";
import type { QueryKey } from "@tanstack/react-query";
import { toast } from "sonner";
import { queryClient } from "@/api/query-client";
import { Badge } from "@/components/reui/badge";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { messageOf, WriteError } from "./api";

// Pieces every orgs page uses: a write's toast and rollback, a 422's field errors on a
// TanStack Form, and the small badges people and team lists share.

export function failed(what: string) {
  return (e: unknown) => toast.error(what, { description: messageOf(e) });
}

// An optimistic cache edit for every query under `key`, with the rollback a mutation's
// onError calls.
export async function optimistic<T>(key: QueryKey, edit: (old: T) => T) {
  await queryClient.cancelQueries({ queryKey: key });
  const before = queryClient.getQueriesData<T>({ queryKey: key });
  for (const [k, v] of before) if (v !== undefined) queryClient.setQueryData<T>(k, edit(v));
  return () => before.forEach(([k, v]) => queryClient.setQueryData(k, v));
}

export const refetch = (...keys: QueryKey[]) =>
  Promise.all(keys.map((queryKey) => queryClient.invalidateQueries({ queryKey })));

// Puts a 422's field errors under the matching form fields and returns what no field
// took, for a line under the form.
export function applyServerErrors(
  form: AnyFormApi,
  e: unknown,
  fields: Record<string, string>,
): string | undefined {
  if (!(e instanceof WriteError) || e.status !== 422) return messageOf(e);
  const left: string[] = [];
  for (const [server, message] of Object.entries(e.fields)) {
    const name = fields[server];
    if (!name) left.push(message);
    else form.setFieldMeta(name, (m) => ({ ...m, errorMap: { ...m.errorMap, onSubmit: message } }));
  }
  return left.length ? left.join(" ") : Object.keys(e.fields).length ? undefined : e.message;
}

export function FieldMessage({ field }: { field: AnyFieldApi }) {
  const errors = field.state.meta.errors.filter(Boolean);
  if (!errors.length) return null;
  return (
    <p className="text-destructive text-xs" role="alert" data-field-error={field.name}>
      {errors.map(String).join(" ")}
    </p>
  );
}

export function FormFailure({ children }: { children?: ReactNode }) {
  if (!children) return null;
  return (
    <p className="text-destructive text-xs" role="alert" data-form-error>
      {children}
    </p>
  );
}

export function RoleBadge({ role }: { role: "admin" | "member" | "maintainer" | string }) {
  const label =
    role === "admin" ? "Owner" : role === "maintainer" ? "Maintainer" : role.replace("_", " ");
  return (
    <Badge
      variant={role === "admin" || role === "maintainer" ? "primary-light" : "outline"}
      size="sm"
      className="capitalize"
      data-role={role}
    >
      {label}
    </Badge>
  );
}

export function EmptyRow({ children }: { children: ReactNode }) {
  return (
    <p className="text-muted-foreground px-4 py-8 text-center text-sm" data-empty>
      {children}
    </p>
  );
}

// GHES's repository permissions, in rank order, with the names the UI shows.
export const PERMISSIONS = [
  { api: "pull", label: "Read" },
  { api: "triage", label: "Triage" },
  { api: "push", label: "Write" },
  { api: "maintain", label: "Maintain" },
  { api: "admin", label: "Admin" },
] as const;
export type Permission = (typeof PERMISSIONS)[number]["api"];

// A repository's permission flags as the highest role they grant.
export function permissionOf(
  p: { admin?: boolean; maintain?: boolean; push?: boolean; triage?: boolean } | undefined,
): Permission {
  if (p?.admin) return "admin";
  if (p?.maintain) return "maintain";
  if (p?.push) return "push";
  if (p?.triage) return "triage";
  return "pull";
}

// A destructive action behind a dialog. With `phrase`, the user types it to confirm.
export function Confirm({
  open,
  onOpenChange,
  title,
  description,
  action,
  pending,
  phrase,
  onConfirm,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: ReactNode;
  action: string;
  pending?: boolean;
  phrase?: string;
  onConfirm: () => void;
  children?: ReactNode;
}) {
  const [typed, setTyped] = useState("");
  const ok = !phrase || typed.trim() === phrase;
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
            // The dialog portals out, but React bubbles submit through the tree.
            e.stopPropagation();
            if (ok) onConfirm();
          }}
        >
          {phrase && (
            <>
              <label className="text-sm" htmlFor="confirm-phrase">
                To confirm, type{" "}
                <code className="bg-muted px-1 font-mono text-xs font-semibold">{phrase}</code>{" "}
                below.
              </label>
              <Input
                id="confirm-phrase"
                autoComplete="off"
                value={typed}
                onChange={(e) => setTyped(e.target.value)}
              />
            </>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <Button
              type="submit"
              variant="destructive"
              disabled={!ok || pending}
              data-testid="confirm-action"
            >
              {pending ? "Working…" : action}
            </Button>
          </AlertDialogFooter>
        </form>
      </AlertDialogContent>
    </AlertDialog>
  );
}
