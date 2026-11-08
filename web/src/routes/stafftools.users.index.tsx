import { useState } from "react";
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { HugeiconsIcon } from "@hugeicons/react";
import { UserAdd01Icon } from "@hugeicons/core-free-icons";
import { api } from "@/api/client";
import { staffUsersQuery } from "@/areas/admin/queries";
import type { StaffUser } from "@/areas/admin/stafftools-api";
import {
  columns,
  Grid,
  send,
  showFieldErrors,
  StateBadge,
  TextField,
  useWrite,
} from "@/areas/admin/ui";
import { Ago, UserAvatar } from "@/components/github";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldLabel } from "@/components/ui/field";

const FILTERS = ["all", "admins", "suspended", "active"] as const;
type Filter = (typeof FILTERS)[number];

export const Route = createFileRoute("/stafftools/users/")({
  validateSearch: (s: Record<string, unknown>): { q?: string; filter?: Filter } => ({
    q: typeof s.q === "string" && s.q ? s.q : undefined,
    filter:
      FILTERS.includes(s.filter as Filter) && s.filter !== "all" ? (s.filter as Filter) : undefined,
  }),
  loader: ({ context: { queryClient } }) => void queryClient.prefetchQuery(staffUsersQuery()),
  component: Users,
});

function Users() {
  const [creating, setCreating] = useState(false);
  return (
    <>
      <PageHeader
        title="Users"
        description="Every account on this instance. Open one to promote, suspend, rename, or impersonate it."
        actions={
          <Button size="sm" onClick={() => setCreating(true)}>
            <HugeiconsIcon icon={UserAdd01Icon} strokeWidth={2} data-icon="inline-start" />
            Create user
          </Button>
        }
      />
      <Region name="users" rows={8}>
        <UserGrid />
      </Region>
      <CreateUser open={creating} onOpenChange={setCreating} />
    </>
  );
}

const col = columns<StaffUser>();
const cols = [
  col.accessor((u) => `${u.login} ${u.name ?? ""}`, {
    id: "login",
    header: "User",
    cell: ({ row }) => (
      <span className="flex items-center gap-2">
        <UserAvatar user={row.original} />
        <span className="min-w-0">
          <Link
            to="/stafftools/users/$username"
            params={{ username: row.original.login }}
            className="font-semibold hover:underline"
          >
            {row.original.login}
          </Link>
          {row.original.name && (
            <span className="text-muted-foreground ml-1.5">{row.original.name}</span>
          )}
        </span>
      </span>
    ),
  }),
  col.accessor((u) => u.email ?? "", {
    id: "email",
    header: "Email",
    cell: ({ getValue }) => getValue() || "None",
  }),
  col.accessor((u) => (u.suspended_at ? "suspended" : u.site_admin ? "site admin" : "active"), {
    id: "status",
    header: "Status",
    cell: ({ getValue }) => {
      const v = getValue();
      return (
        <StateBadge
          tone={
            v === "suspended"
              ? "destructive-light"
              : v === "site admin"
                ? "info-light"
                : "success-light"
          }
        >
          {v}
        </StateBadge>
      );
    },
  }),
  col.accessor((u) => (u.two_factor ? "on" : "off"), { id: "2fa", header: "2FA" }),
  col.accessor("created_at", {
    header: "Created",
    cell: ({ getValue }) => <Ago at={getValue()} />,
  }),
];

function UserGrid() {
  const { data } = useSuspenseQuery(staffUsersQuery());
  const { q = "", filter = "all" } = Route.useSearch();
  const navigate = Route.useNavigate();
  const goto = useNavigate();
  const rows = data.filter((u) =>
    filter === "admins"
      ? u.site_admin
      : filter === "suspended"
        ? !!u.suspended_at
        : filter === "active"
          ? !u.suspended_at
          : true,
  );
  return (
    <Grid
      rows={rows}
      cols={cols}
      rowId={(u) => String(u.id)}
      search={q}
      onSearch={(v) => navigate({ search: (p) => ({ ...p, q: v || undefined }), replace: true })}
      placeholder="Search users by login, name, or email"
      empty="No users match."
      onRowClick={(u) => goto({ to: "/stafftools/users/$username", params: { username: u.login } })}
      toolbar={
        <div
          className="flex rounded-sm border p-0.5 text-xs"
          role="tablist"
          aria-label="User filter"
        >
          {FILTERS.map((f) => (
            <Link
              key={f}
              from={Route.fullPath}
              search={(p) => ({ ...p, filter: f === "all" ? undefined : f })}
              role="tab"
              aria-selected={filter === f}
              className="text-muted-foreground aria-selected:bg-muted aria-selected:text-foreground rounded-sm px-2 py-1 font-medium capitalize"
            >
              {f}
            </Link>
          ))}
        </div>
      }
    />
  );
}

function CreateUser({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
  const goto = useNavigate();
  const create = useWrite({
    run: (body: { login: string; email?: string; suspended: boolean }) =>
      send(api.POST("/admin/users", { body })),
    invalidate: [staffUsersQuery().queryKey, ["enterprise"]],
    success: (u) => `Created ${u.login}. An invitation went to their email.`,
    onSuccess: (u) => {
      onOpenChange(false);
      form.reset();
      void goto({ to: "/stafftools/users/$username", params: { username: u.login } });
    },
    onError: (e) => showFieldErrors(e, form, { login: "login", email: "email" }),
  });
  const form = useForm({
    defaultValues: { login: "", email: "", suspended: false },
    onSubmit: ({ value }) =>
      create
        .mutateAsync({
          login: value.login.trim(),
          email: value.email.trim() || undefined,
          suspended: value.suspended,
        })
        .catch(() => {}),
  });
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="rounded-sm sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Create a user</DialogTitle>
          <DialogDescription>
            With built-in authentication, the user gets an email to set a password. With LDAP or
            SAML, the account links on first sign-in.
          </DialogDescription>
        </DialogHeader>
        <form
          id="create-user"
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
                  : !/^[a-z\d](?:[a-z\d]|-(?=[a-z\d])){0,38}$/i.test(value.trim())
                    ? "Use letters, digits, and single hyphens, up to 39 characters."
                    : undefined,
            }}
          >
            {(field) => <TextField field={field} label="Login" placeholder="octocat" />}
          </form.Field>
          <form.Field
            name="email"
            validators={{
              onSubmit: ({ value }) =>
                value && !/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(value)
                  ? "That is not an email address."
                  : undefined,
            }}
          >
            {(field) => (
              <TextField
                field={field}
                label="Email"
                type="email"
                placeholder="octocat@graftyard.test"
              />
            )}
          </form.Field>
          <form.Field name="suspended">
            {(field) => (
              <Field orientation="horizontal">
                <Checkbox
                  id="suspended"
                  checked={field.state.value}
                  onCheckedChange={(v) => field.handleChange(!!v)}
                />
                <FieldLabel htmlFor="suspended">Create suspended</FieldLabel>
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
              <Button type="submit" form="create-user" size="sm" disabled={busy}>
                {busy ? "Creating" : "Create user"}
              </Button>
            )}
          </form.Subscribe>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
