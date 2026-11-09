import { useState } from "react";
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  ArrowLeft02Icon,
  Copy01Icon,
  Delete02Icon,
  Key01Icon,
  Refresh01Icon,
  ShieldUserIcon,
  UserBlock01Icon,
  UserCheck01Icon,
  UserSwitchIcon,
} from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { keysQuery, staffUserQuery, staffUsersQuery, tokensQuery } from "@/areas/admin/queries";
import type { StaffUser } from "@/areas/admin/stafftools-api";
import {
  message,
  send,
  showFieldErrors,
  StateBadge,
  TextField,
  useTyped,
  useWrite,
  when,
} from "@/areas/admin/ui";
import { userQuery } from "@/areas/core/queries";
import { UserAvatar } from "@/components/github";
import { Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
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
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";

export const Route = createFileRoute("/stafftools/users/$username")({
  loader: ({ context: { queryClient }, params }) => {
    void queryClient.prefetchQuery(staffUserQuery(params.username));
    void queryClient.prefetchQuery(keysQuery());
    void queryClient.prefetchQuery(tokensQuery());
  },
  component: UserPage,
});

function UserPage() {
  const { username } = Route.useParams();
  return (
    <div className="flex flex-col gap-4">
      <Link
        to="/stafftools/users"
        className="text-muted-foreground flex items-center gap-1 text-sm hover:underline"
      >
        <HugeiconsIcon icon={ArrowLeft02Icon} strokeWidth={2} className="size-4" />
        Users
      </Link>
      <Region name="user" rows={3} fallback={<Skeleton className="h-24" />}>
        <UserRegions key={username} username={username} />
      </Region>
    </div>
  );
}

// The user record is the region every section reads; sections with their own reads
// (keys, authorizations) are their own regions.
function UserRegions({ username }: { username: string }) {
  const { data: u } = useSuspenseQuery(staffUserQuery(username));
  return (
    <>
      <Header u={u} />
      <div className="grid items-start gap-4 lg:grid-cols-2">
        <div className="flex flex-col gap-4">
          <SiteAdmin u={u} />
          <Suspension u={u} />
          <Ldap u={u} />
        </div>
        <div className="flex flex-col gap-4">
          <Region name="impersonation" rows={3}>
            <Impersonation u={u} />
          </Region>
          <Rename u={u} />
        </div>
      </div>
      <Region name="SSH keys" rows={3}>
        <Keys u={u} />
      </Region>
      <Region name="authorizations" rows={3}>
        <Authorizations u={u} />
      </Region>
      <DeleteUser u={u} />
    </>
  );
}

const userKey = (login: string) => staffUserQuery(login).queryKey;
const statsKeys = [["enterprise"], staffUsersQuery().queryKey];

// Writes that change the user record show their result at once and roll back on failure.
function useUserPatch(u: StaffUser) {
  const queryClient = useQueryClient();
  return {
    apply(patch: Partial<StaffUser>) {
      const key = userKey(u.login);
      const before = queryClient.getQueryData(key);
      queryClient.setQueryData(key, { ...u, ...patch });
      return () => queryClient.setQueryData(key, before);
    },
  };
}

function Header({ u }: { u: StaffUser }) {
  return (
    <div className="flex flex-wrap items-center gap-3">
      <UserAvatar user={u} size="default" />
      <div className="min-w-0">
        <h1 className="flex items-center gap-2 text-xl font-semibold" data-testid="user-login">
          {u.login}
          {u.site_admin && (
            <StateBadge tone="info-light">
              <HugeiconsIcon icon={ShieldUserIcon} strokeWidth={2} />
              site admin
            </StateBadge>
          )}
          {u.suspended_at && <StateBadge tone="destructive-light">suspended</StateBadge>}
        </h1>
        <p className="text-muted-foreground text-sm">
          {u.name ?? "No name"} · {u.email ?? "no email"} · joined {when(u.created_at)} · 2FA{" "}
          {u.two_factor ? "on" : "off"}
        </p>
      </div>
    </div>
  );
}

function SiteAdmin({ u }: { u: StaffUser }) {
  const { apply } = useUserPatch(u);
  const { data: me } = useSuspenseQuery(userQuery());
  const self = me.login === u.login;
  const toggle = useWrite({
    run: async (promote: boolean) => {
      const undo = apply({ site_admin: promote });
      try {
        const path = { params: { path: { username: u.login } } };
        return await send(
          promote
            ? api.PUT("/users/{username}/site_admin", path)
            : api.DELETE("/users/{username}/site_admin", path),
        );
      } catch (e) {
        undo();
        throw e;
      }
    },
    invalidate: [userKey(u.login), ...statsKeys],
    success: (_, promote) =>
      promote
        ? `${u.login} is now a site administrator`
        : `${u.login} is no longer a site administrator`,
  });
  return (
    <Panel title="Site administrator">
      <p className="text-sm">
        {u.site_admin
          ? "This user can open this console, see every repository, and bypass org and repository permissions. Every bypass is audited."
          : "This user has no site administration rights."}
      </p>
      <div className="mt-3 flex items-center gap-2">
        {u.site_admin ? (
          <Button
            size="sm"
            variant="outline"
            disabled={toggle.isPending || self}
            onClick={() => toggle.mutate(false)}
          >
            Demote to user
          </Button>
        ) : (
          <Button
            size="sm"
            disabled={toggle.isPending || !!u.suspended_at}
            onClick={() => toggle.mutate(true)}
          >
            <HugeiconsIcon icon={ShieldUserIcon} strokeWidth={2} data-icon="inline-start" />
            Promote to site admin
          </Button>
        )}
        {self && <span className="text-muted-foreground text-xs">You cannot demote yourself.</span>}
        {u.suspended_at && !u.site_admin && (
          <span className="text-muted-foreground text-xs">Unsuspend first.</span>
        )}
      </div>
    </Panel>
  );
}

function Suspension({ u }: { u: StaffUser }) {
  const { apply } = useUserPatch(u);
  const [reason, setReason] = useState("");
  const write = useWrite({
    run: async (suspend: boolean) => {
      const undo = apply(
        suspend
          ? { suspended_at: new Date().toISOString(), suspension_reason: reason.trim() || null }
          : { suspended_at: null, suspension_reason: null },
      );
      try {
        const path = { username: u.login };
        return await send(
          suspend
            ? api.PUT("/users/{username}/suspended", {
                params: { path },
                body: reason.trim() ? { reason: reason.trim() } : {},
              })
            : api.DELETE("/users/{username}/suspended", { params: { path }, body: {} }),
        );
      } catch (e) {
        undo();
        throw e;
      }
    },
    invalidate: [userKey(u.login), ...statsKeys],
    success: (_, suspend) => (suspend ? `${u.login} is suspended` : `${u.login} is unsuspended`),
    onSuccess: () => setReason(""),
  });
  return (
    <Panel title="Suspension">
      {u.suspended_at ? (
        <>
          <Alert variant="destructive">
            <HugeiconsIcon icon={UserBlock01Icon} strokeWidth={2} />
            <AlertTitle>Suspended {when(u.suspended_at)}</AlertTitle>
            <AlertDescription data-testid="suspension-reason">
              {u.suspension_reason ?? "No reason was given."}
            </AlertDescription>
          </Alert>
          <Button
            size="sm"
            className="mt-3"
            disabled={write.isPending}
            onClick={() => write.mutate(false)}
          >
            <HugeiconsIcon icon={UserCheck01Icon} strokeWidth={2} data-icon="inline-start" />
            Unsuspend
          </Button>
        </>
      ) : (
        <div className="flex flex-col gap-2">
          <p className="text-sm">
            A suspended user cannot sign in, push, or use tokens, and frees their license seat.
          </p>
          <Input
            aria-label="Suspension reason"
            placeholder="Reason, shown in the audit log"
            className="rounded-sm"
            value={reason}
            maxLength={200}
            onChange={(e) => setReason(e.target.value)}
          />
          <div className="flex items-center gap-2">
            <Button
              size="sm"
              variant="destructive"
              disabled={write.isPending || u.site_admin}
              onClick={() => write.mutate(true)}
            >
              <HugeiconsIcon icon={UserBlock01Icon} strokeWidth={2} data-icon="inline-start" />
              Suspend
            </Button>
            {u.site_admin && (
              <span className="text-muted-foreground text-xs">
                Demote a site admin before suspending.
              </span>
            )}
          </div>
        </div>
      )}
    </Panel>
  );
}

function Rename({ u }: { u: StaffUser }) {
  const queryClient = useQueryClient();
  const goto = useNavigate();
  const rename = useWrite({
    run: (login: string) =>
      send(
        api.PATCH("/admin/users/{username}", {
          params: { path: { username: u.login } },
          body: { login },
        }),
      ),
    invalidate: [staffUsersQuery().queryKey, keysQuery().queryKey, tokensQuery().queryKey],
    success: (r) => r.message ?? "Rename queued",
    onSuccess: (_, login) => {
      queryClient.removeQueries({ queryKey: userKey(u.login) });
      void goto({ to: "/stafftools/users/$username", params: { username: login }, replace: true });
    },
    onError: (e) => showFieldErrors(e, form, { login: "login" }),
  });
  const form = useForm({
    defaultValues: { login: u.login },
    onSubmit: ({ value }) => rename.mutateAsync(value.login.trim()).catch(() => {}),
  });
  return (
    <Panel title="Rename">
      <form
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
              value.trim() === u.login
                ? "Choose a different login."
                : !/^[a-z\d](?:[a-z\d]|-(?=[a-z\d])){0,38}$/i.test(value.trim())
                  ? "Use letters, digits, and single hyphens."
                  : undefined,
          }}
        >
          {(field) => (
            <TextField
              field={field}
              label="New login"
              description="Old URLs redirect. Git remotes with the old login keep working until someone takes the name."
            />
          )}
        </form.Field>
        <div>
          <form.Subscribe selector={(s) => s.isSubmitting}>
            {(busy) => (
              <Button size="sm" variant="outline" type="submit" disabled={busy}>
                Rename user
              </Button>
            )}
          </form.Subscribe>
        </div>
      </form>
    </Panel>
  );
}

const SCOPES = ["repo", "read:org", "user", "workflow", "admin:repo_hook", "gist"];

function Impersonation({ u }: { u: StaffUser }) {
  const { data } = useSuspenseQuery(tokensQuery());
  const existing = data.find(
    (t) => t.user?.login === u.login && t.app.client_id.includes("impersonation"),
  );
  const [scopes, setScopes] = useState<string[]>(existing?.scopes ?? ["repo"]);
  const [shown, setShown] = useState<string>();
  const create = useWrite({
    run: () =>
      send(
        api.POST("/admin/users/{username}/authorizations", {
          params: { path: { username: u.login } },
          body: { scopes },
        }),
      ),
    invalidate: [tokensQuery().queryKey],
    onSuccess: (auth) => {
      if (auth.token) {
        setShown(auth.token);
        toast.success(`Impersonation token ${existing ? "replaced" : "created"} for ${u.login}`);
      } else {
        toast.info(
          "The existing token already has these scopes. GHES shows a token only when it is created.",
        );
      }
    },
  });
  const revoke = useWrite({
    run: () =>
      send(
        api.DELETE("/admin/users/{username}/authorizations", {
          params: { path: { username: u.login } },
        }),
      ),
    invalidate: [tokensQuery().queryKey],
    success: "Impersonation token revoked",
    onSuccess: () => setShown(undefined),
  });
  return (
    <Panel
      title={
        <span className="flex items-center gap-2">
          <HugeiconsIcon icon={UserSwitchIcon} strokeWidth={2} className="size-4" />
          Impersonation
        </span>
      }
    >
      <p className="text-sm">
        An impersonation token acts as {u.login}. Its use is audited under your name. Use it to
        reproduce what they see.
      </p>
      <fieldset className="mt-3 flex flex-wrap gap-x-4 gap-y-2" aria-label="Token scopes">
        {SCOPES.map((s) => (
          <Field key={s} orientation="horizontal" className="w-auto">
            <Checkbox
              id={`scope-${s}`}
              checked={scopes.includes(s)}
              onCheckedChange={(v) => setScopes((p) => (v ? [...p, s] : p.filter((x) => x !== s)))}
            />
            <FieldLabel htmlFor={`scope-${s}`} className="font-mono text-xs">
              {s}
            </FieldLabel>
          </Field>
        ))}
      </fieldset>
      {shown && (
        <Alert variant="success" className="mt-3">
          <HugeiconsIcon icon={Key01Icon} strokeWidth={2} />
          <AlertTitle>Copy the token now. It is not shown again.</AlertTitle>
          <AlertDescription>
            <span className="flex items-center gap-2">
              <code
                className="bg-muted rounded-sm px-1 py-0.5 text-xs break-all"
                data-testid="impersonation-token"
              >
                {shown}
              </code>
              <Button
                size="icon-xs"
                variant="ghost"
                aria-label="Copy token"
                onClick={() =>
                  void navigator.clipboard?.writeText(shown).then(() => toast.success("Copied"))
                }
              >
                <HugeiconsIcon icon={Copy01Icon} strokeWidth={2} />
              </Button>
            </span>
          </AlertDescription>
        </Alert>
      )}
      <div className="mt-3 flex items-center gap-2">
        <Button
          size="sm"
          disabled={create.isPending || scopes.length === 0 || !!u.suspended_at}
          onClick={() => create.mutate(undefined)}
        >
          {existing ? "Update token scopes" : "Create impersonation token"}
        </Button>
        {existing && (
          <Button
            size="sm"
            variant="destructive"
            disabled={revoke.isPending}
            onClick={() => revoke.mutate(undefined)}
          >
            Revoke
          </Button>
        )}
        {existing && (
          <span className="text-muted-foreground text-xs" data-testid="impersonation-existing">
            Active: <code>{(existing.scopes ?? []).join(", ")}</code>, ends in{" "}
            {existing.token_last_eight}
          </span>
        )}
      </div>
      {scopes.length === 0 && (
        <p className="text-destructive mt-1 text-xs">Pick at least one scope.</p>
      )}
    </Panel>
  );
}

function Ldap({ u }: { u: StaffUser }) {
  const queryClient = useQueryClient();
  const map = useWrite({
    run: (ldap_dn: string) =>
      send(
        api.PATCH("/admin/ldap/users/{username}/mapping", {
          params: { path: { username: u.login } },
          body: { ldap_dn },
        }),
      ),
    invalidate: [userKey(u.login)],
    success: "LDAP mapping saved",
    onError: (e) => showFieldErrors(e, form, { ldap_dn: "ldap_dn" }),
  });
  const sync = useWrite({
    run: () =>
      send(
        api.POST("/admin/ldap/users/{username}/sync", { params: { path: { username: u.login } } }),
      ),
    invalidate: [userKey(u.login)],
    success: "LDAP sync queued",
    // The sync lands a few seconds later; read the user again once it should have.
    onSuccess: () =>
      setTimeout(() => void queryClient.invalidateQueries({ queryKey: userKey(u.login) }), 4500),
  });
  const form = useForm({
    defaultValues: { ldap_dn: u.ldap_dn ?? "" },
    onSubmit: ({ value }) => map.mutateAsync(value.ldap_dn.trim()).catch(() => {}),
  });
  return (
    <Panel
      title="LDAP"
      actions={
        <StateBadge
          tone={
            u.ldap_sync === "synced"
              ? "success-light"
              : u.ldap_sync === "queued"
                ? "warning-light"
                : "outline"
          }
        >
          <span data-testid="ldap-sync">
            {u.ldap_sync === "never" ? "not synced" : u.ldap_sync}
          </span>
        </StateBadge>
      }
    >
      <form
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          void form.handleSubmit();
        }}
      >
        <form.Field
          name="ldap_dn"
          validators={{
            onSubmit: ({ value }) => (!value.trim() ? "Enter a distinguished name." : undefined),
          }}
        >
          {(field) => (
            <TextField
              field={field}
              label="Distinguished name"
              placeholder="uid=octocat,ou=people,dc=graftyard,dc=test"
              description={
                u.ldap_synced_at ? `Last synced ${when(u.ldap_synced_at)}.` : "Never synced."
              }
            />
          )}
        </form.Field>
        <div className="flex gap-2">
          <Button size="sm" variant="outline" type="submit" disabled={map.isPending}>
            Save mapping
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={sync.isPending || !u.ldap_dn || u.ldap_sync === "queued"}
            onClick={() => sync.mutate(undefined)}
          >
            <HugeiconsIcon icon={Refresh01Icon} strokeWidth={2} data-icon="inline-start" />
            Sync now
          </Button>
        </div>
      </form>
    </Panel>
  );
}

function Keys({ u }: { u: StaffUser }) {
  const queryClient = useQueryClient();
  const { data } = useSuspenseQuery(keysQuery());
  const mine = data.filter((k) => k.user_id === u.id);
  const remove = useWrite({
    run: async (key: Schemas["public-key-full"]) => {
      const before = queryClient.getQueryData(keysQuery().queryKey);
      queryClient.setQueryData(keysQuery().queryKey, (old) => old?.filter((k) => k.id !== key.id));
      try {
        return await send(
          api.DELETE("/admin/keys/{key_ids}", { params: { path: { key_ids: String(key.id) } } }),
        );
      } catch (e) {
        queryClient.setQueryData(keysQuery().queryKey, before);
        throw e;
      }
    },
    invalidate: [keysQuery().queryKey],
    success: (_, k) => `Deleted key ${k.title}`,
  });
  return (
    <Panel title={`SSH keys (${mine.length})`} flush>
      {mine.length === 0 ? (
        <p className="text-muted-foreground p-4 text-sm">No SSH keys.</p>
      ) : (
        <ul className="divide-y" data-testid="ssh-keys">
          {mine.map((k) => (
            <li key={k.id} className="flex items-center gap-3 px-4 py-2 text-sm">
              <HugeiconsIcon
                icon={Key01Icon}
                strokeWidth={2}
                className="text-muted-foreground size-4"
              />
              <div className="min-w-0 flex-1">
                <div className="font-medium">{k.title}</div>
                <code className="text-muted-foreground block truncate text-xs">{k.key}</code>
                <div className="text-muted-foreground text-xs">
                  Added {when(k.created_at)} · last used {when(k.last_used)}
                </div>
              </div>
              {k.read_only && (
                <Badge variant="outline" size="sm">
                  read only
                </Badge>
              )}
              <Button
                size="sm"
                variant="destructive"
                disabled={remove.isPending}
                onClick={() => remove.mutate(k)}
              >
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} data-icon="inline-start" />
                Delete
              </Button>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

function Authorizations({ u }: { u: StaffUser }) {
  const { data } = useSuspenseQuery(tokensQuery());
  const mine = data.filter((t) => t.user?.login === u.login);
  return (
    <Panel
      title={`Authorizations (${mine.length})`}
      actions={
        <Link to="/stafftools/tokens" search={{ q: u.login }} className="text-xs hover:underline">
          Manage in Tokens
        </Link>
      }
      flush
    >
      {mine.length === 0 ? (
        <p className="text-muted-foreground p-4 text-sm">No tokens or authorizations.</p>
      ) : (
        <ul className="divide-y" data-testid="authorizations">
          {mine.map((t) => (
            <li key={t.id} className="flex flex-wrap items-center gap-2 px-4 py-2 text-sm">
              <span className="font-medium">{t.note ?? t.app.name}</span>
              <span className="text-muted-foreground text-xs">{t.app.name}</span>
              <span className="flex flex-wrap gap-1">
                {(t.scopes ?? []).map((s) => (
                  <Badge key={s} variant="outline" size="sm" className="font-mono">
                    {s}
                  </Badge>
                ))}
              </span>
              <span className="text-muted-foreground ml-auto text-xs">
                …{t.token_last_eight} ·{" "}
                {t.expires_at ? `expires ${when(t.expires_at)}` : "no expiry"}
              </span>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

function DeleteUser({ u }: { u: StaffUser }) {
  const [open, setOpen] = useState(false);
  const typed = useTyped(u.login);
  const goto = useNavigate();
  const queryClient = useQueryClient();
  const { data: me } = useSuspenseQuery(userQuery());
  const remove = useWrite({
    run: () =>
      send(api.DELETE("/admin/users/{username}", { params: { path: { username: u.login } } })),
    invalidate: [...statsKeys, keysQuery().queryKey, tokensQuery().queryKey],
    success: `Deleted ${u.login}`,
    onSuccess: () => {
      setOpen(false);
      queryClient.removeQueries({ queryKey: userKey(u.login) });
      void goto({ to: "/stafftools/users" });
    },
  });
  return (
    <Panel title="Danger zone" className="border-destructive/40">
      <div className="flex flex-wrap items-center gap-3">
        <div className="min-w-0 flex-1 text-sm">
          <div className="font-medium">Delete this user</div>
          <div className="text-muted-foreground">
            Deletes their repositories, gists, keys, and tokens. Their issues and comments stay,
            under the ghost user.
          </div>
        </div>
        <Button
          size="sm"
          variant="destructive"
          disabled={me.login === u.login}
          onClick={() => setOpen(true)}
        >
          Delete user
        </Button>
      </div>
      <AlertDialog open={open} onOpenChange={setOpen}>
        <AlertDialogContent className="rounded-sm">
          <AlertDialogHeader>
            <AlertDialogTitle>Delete {u.login}?</AlertDialogTitle>
            <AlertDialogDescription>
              This cannot be undone. Type <strong>{u.login}</strong> to confirm.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <Input
            aria-label="Type the login to confirm"
            className="rounded-sm"
            value={typed.value}
            onChange={(e) => typed.setValue(e.target.value)}
          />
          {remove.error && <p className="text-destructive text-xs">{message(remove.error)}</p>}
          <AlertDialogFooter>
            <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
            <Button
              variant="destructive"
              size="sm"
              disabled={!typed.ok || remove.isPending}
              onClick={() => remove.mutate(undefined)}
            >
              Delete user
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Panel>
  );
}
