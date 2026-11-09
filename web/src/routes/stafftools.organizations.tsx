import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { HugeiconsIcon } from "@hugeicons/react";
import { Add01Icon, PencilEdit02Icon } from "@hugeicons/core-free-icons";
import { api } from "@/api/client";
import { staffOrgsQuery, staffUsersQuery } from "@/areas/admin/queries";
import type { StaffOrg } from "@/areas/admin/stafftools-api";
import { columns, Grid, send, showFieldErrors, TextField, useWrite, when } from "@/areas/admin/ui";
import { UserAvatar } from "@/components/github";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldError, FieldLabel } from "@/components/ui/field";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";

export const Route = createFileRoute("/stafftools/organizations")({
  validateSearch: (s: Record<string, unknown>): { q?: string } => ({
    q: typeof s.q === "string" && s.q ? s.q : undefined,
  }),
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(staffOrgsQuery());
    void queryClient.prefetchQuery(staffUsersQuery());
  },
  component: Orgs,
});

const LOGIN = /^[a-z\d](?:[a-z\d]|-(?=[a-z\d])){0,38}$/i;
const ORGS = staffOrgsQuery().queryKey;

function Orgs() {
  const [creating, setCreating] = useState(false);
  const [renaming, setRenaming] = useState<StaffOrg>();
  return (
    <>
      <PageHeader
        title="Organizations"
        description="Every organization on this instance."
        actions={
          <Button size="sm" onClick={() => setCreating(true)}>
            <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
            New organization
          </Button>
        }
      />
      <Region name="organizations" rows={5}>
        <OrgGrid onRename={setRenaming} />
      </Region>
      <Region name="users" fallback={null}>
        <CreateOrg open={creating} onOpenChange={setCreating} />
      </Region>
      {renaming && <RenameOrg org={renaming} onClose={() => setRenaming(undefined)} />}
    </>
  );
}

function OrgGrid({ onRename }: { onRename: (o: StaffOrg) => void }) {
  const { data } = useSuspenseQuery(staffOrgsQuery());
  const { q = "" } = Route.useSearch();
  const navigate = Route.useNavigate();
  const col = columns<StaffOrg>();
  const cols = [
    col.accessor("login", {
      header: "Organization",
      cell: ({ row }) => (
        <span className="flex items-center gap-2">
          <UserAvatar user={row.original} />
          {row.original.login === "graftyard" ? (
            <Link
              to="/orgs/$org"
              params={{ org: row.original.login }}
              className="font-semibold hover:underline"
            >
              {row.original.login}
            </Link>
          ) : (
            <span className="font-semibold">{row.original.login}</span>
          )}
          {row.original.name && <span className="text-muted-foreground">{row.original.name}</span>}
        </span>
      ),
    }),
    col.accessor((o) => o.admins.join(", "), { id: "admins", header: "Owners" }),
    col.accessor("members", { header: "Members" }),
    col.accessor("repositories", { header: "Repositories" }),
    col.accessor("created_at", { header: "Created", cell: ({ getValue }) => when(getValue()) }),
    col.display({
      id: "actions",
      header: "",
      cell: ({ row }) => (
        <Button
          size="xs"
          variant="outline"
          aria-label={`Rename ${row.original.login}`}
          onClick={() => onRename(row.original)}
        >
          <HugeiconsIcon icon={PencilEdit02Icon} strokeWidth={2} />
          Rename
        </Button>
      ),
    }),
  ];
  return (
    <Grid
      rows={data}
      cols={cols}
      rowId={(o) => String(o.id)}
      search={q}
      onSearch={(v) => navigate({ search: { q: v || undefined }, replace: true })}
      placeholder="Search organizations"
      empty="No organizations match."
    />
  );
}

function CreateOrg({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
  const { data: users } = useSuspenseQuery(staffUsersQuery());
  const create = useWrite({
    run: (body: { login: string; admin: string; profile_name?: string }) =>
      send(api.POST("/admin/organizations", { body })),
    invalidate: [ORGS, ["enterprise"]],
    success: (o) => `Created ${o.login}`,
    onSuccess: () => {
      onOpenChange(false);
      form.reset();
    },
    onError: (e) => showFieldErrors(e, form, { login: "login", admin: "admin" }),
  });
  const form = useForm({
    defaultValues: { login: "", admin: "mira", profile_name: "" },
    onSubmit: ({ value }) =>
      create
        .mutateAsync({
          login: value.login.trim(),
          admin: value.admin,
          profile_name: value.profile_name.trim() || undefined,
        })
        .catch(() => {}),
  });
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="rounded-sm sm:max-w-md">
        <DialogHeader>
          <DialogTitle>New organization</DialogTitle>
          <DialogDescription>The admin you pick becomes its first owner.</DialogDescription>
        </DialogHeader>
        <form
          id="create-org"
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <form.Field
            name="login"
            validators={{
              onSubmit: ({ value }) =>
                !value.trim()
                  ? "Login is required."
                  : !LOGIN.test(value.trim())
                    ? "Use letters, digits, and single hyphens."
                    : undefined,
            }}
          >
            {(field) => <TextField field={field} label="Login" placeholder="platform" />}
          </form.Field>
          <form.Field name="profile_name">
            {(field) => (
              <TextField field={field} label="Display name" placeholder="Platform team" />
            )}
          </form.Field>
          <form.Field name="admin">
            {(field) => (
              <Field data-invalid={field.state.meta.errors.length > 0 || undefined}>
                <FieldLabel htmlFor="org-admin">Admin</FieldLabel>
                <NativeSelect
                  id="org-admin"
                  value={field.state.value}
                  onChange={(e) => {
                    field.setMeta((m) => ({
                      ...m,
                      errorMap: { ...m.errorMap, onServer: undefined },
                    }));
                    field.handleChange(e.target.value);
                  }}
                >
                  {users.map((u) => (
                    <NativeSelectOption key={u.login} value={u.login}>
                      {u.login}
                      {u.suspended_at ? " (suspended)" : ""}
                    </NativeSelectOption>
                  ))}
                </NativeSelect>
                {field.state.meta.errors.length > 0 && (
                  <FieldError>{field.state.meta.errors.join(". ")}</FieldError>
                )}
              </Field>
            )}
          </form.Field>
        </form>
        <DialogFooter>
          <Button variant="outline" size="sm" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <form.Subscribe selector={(s) => s.isSubmitting}>
            {(busy) => (
              <Button type="submit" form="create-org" size="sm" disabled={busy}>
                Create organization
              </Button>
            )}
          </form.Subscribe>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function RenameOrg({ org, onClose }: { org: StaffOrg; onClose: () => void }) {
  const rename = useWrite({
    run: (login: string) =>
      send(
        api.PATCH("/admin/organizations/{org}", {
          params: { path: { org: org.login } },
          body: { login },
        }),
      ),
    invalidate: [ORGS],
    success: (r) => r.message ?? "Rename queued",
    onSuccess: onClose,
    onError: (e) => showFieldErrors(e, form, { login: "login" }),
  });
  const form = useForm({
    defaultValues: { login: org.login },
    onSubmit: ({ value }) => rename.mutateAsync(value.login.trim()).catch(() => {}),
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="rounded-sm sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Rename {org.login}</DialogTitle>
          <DialogDescription>
            Repository URLs redirect. Other orgs and users may take the old name.
          </DialogDescription>
        </DialogHeader>
        <form
          id="rename-org"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <form.Field
            name="login"
            validators={{
              onSubmit: ({ value }) =>
                value.trim() === org.login
                  ? "Choose a different name."
                  : !LOGIN.test(value.trim())
                    ? "Use letters, digits, and single hyphens."
                    : undefined,
            }}
          >
            {(field) => <TextField field={field} label="New login" />}
          </form.Field>
        </form>
        <DialogFooter>
          <Button variant="outline" size="sm" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" form="rename-org" size="sm" disabled={rename.isPending}>
            Rename
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
