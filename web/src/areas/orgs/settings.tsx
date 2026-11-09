import { useState, type ReactNode } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useMutation, useSuspenseQuery, useQueryClient } from "@tanstack/react-query";
import { useForm, type AnyFieldApi } from "@tanstack/react-form";
import { toast } from "sonner";
import { api, type Schemas } from "@/api/client";
import type { paths } from "@/api/ghes";
import { orgQuery, userOrgsQuery } from "@/areas/core/queries";
import { Panel } from "@/components/panel";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { Ago } from "@/components/github";
import { ext, send, ui, type MemberPrivileges, type PendingRename, type TeamSettings } from "./api";
import { applyServerErrors, Confirm, failed, FieldMessage, FormFailure, refetch } from "./kit";
import { renameQuery, teamSettingsQuery } from "./queries";

type Org = Schemas["organization-full"];
type OrgPatch = NonNullable<
  paths["/orgs/{org}"]["patch"]["requestBody"]
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

// PATCH /orgs/{org} with the change shown at once on the header and the page, and rolled
// back with a toast if the server refuses.
export function useOrgPatch(org: string, what: string) {
  const client = useQueryClient();
  const key = orgQuery(org).queryKey;
  return useMutation({
    mutationFn: (body: OrgPatch) =>
      send(api.PATCH("/orgs/{org}", { params: { path: { org } }, body })),
    onMutate: async (body) => {
      await client.cancelQueries({ queryKey: key });
      const before = client.getQueryData<Org>(key);
      client.setQueryData<Org>(key, (old) => old && { ...old, ...(body as Partial<Org>) });
      return before;
    },
    onError: (e, _b, before) => {
      client.setQueryData(key, before);
      failed(what)(e);
    },
    onSuccess: (org) => client.setQueryData(key, org),
    onSettled: () => refetch(key),
  });
}

// ---- Profile ----

const PROFILE_FIELDS = {
  name: "name",
  email: "email",
  description: "description",
  blog: "blog",
  location: "location",
  billing_email: "billing_email",
  company: "company",
} as const;

type ProfileValues = Record<keyof typeof PROFILE_FIELDS, string>;

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

function TextInput({
  field,
  label,
  help,
  multiline,
  type = "text",
}: {
  field: AnyFieldApi;
  label: string;
  help?: string;
  multiline?: boolean;
  type?: string;
}) {
  const id = `org-${field.name}`;
  const invalid = field.state.meta.errors.length > 0 || undefined;
  return (
    <div className="flex flex-col gap-1.5">
      <Label htmlFor={id}>{label}</Label>
      {multiline ? (
        <Textarea
          id={id}
          rows={3}
          value={field.state.value}
          onBlur={field.handleBlur}
          onChange={(e) => field.handleChange(e.target.value)}
          aria-invalid={invalid}
        />
      ) : (
        <Input
          id={id}
          type={type}
          value={field.state.value}
          onBlur={field.handleBlur}
          onChange={(e) => field.handleChange(e.target.value)}
          aria-invalid={invalid}
        />
      )}
      {help && <p className="text-muted-foreground text-xs">{help}</p>}
      <FieldMessage field={field} />
    </div>
  );
}

const emailCheck = ({ value }: { value: string }) =>
  value.trim() && !EMAIL_RE.test(value.trim()) ? "Not an email address." : undefined;

export function OrgProfileForm({ org }: { org: string }) {
  const { data } = useSuspenseQuery(orgQuery(org));
  const client = useQueryClient();
  const [failure, setFailure] = useState<string>();
  const form = useForm({
    defaultValues: {
      name: data.name ?? "",
      email: data.email ?? "",
      description: data.description ?? "",
      blog: data.blog ?? "",
      location: data.location ?? "",
      billing_email: data.billing_email ?? "",
      company: data.company ?? "",
    } satisfies ProfileValues,
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const key = orgQuery(org).queryKey;
      const before = client.getQueryData<Org>(key);
      const body = Object.fromEntries(
        Object.entries(value).map(([k, v]) => [k, v.trim()]),
      ) as ProfileValues;
      client.setQueryData<Org>(
        key,
        (old) => old && { ...old, ...body, name: body.name || old.login },
      );
      try {
        const saved = await send(api.PATCH("/orgs/{org}", { params: { path: { org } }, body }));
        client.setQueryData(key, saved);
        formApi.reset({
          name: saved.name ?? "",
          email: saved.email ?? "",
          description: saved.description ?? "",
          blog: saved.blog ?? "",
          location: saved.location ?? "",
          billing_email: saved.billing_email ?? "",
          company: saved.company ?? "",
        });
        toast.success("Profile updated");
        void refetch(["orgs", org], userOrgsQuery().queryKey);
      } catch (e) {
        client.setQueryData(key, before);
        setFailure(applyServerErrors(formApi, e, PROFILE_FIELDS));
        failed("Profile not saved")(e);
      }
    },
  });
  return (
    <form
      className="flex max-w-xl flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
      data-testid="org-profile-form"
    >
      <form.Field
        name="name"
        validators={{
          onChange: ({ value }) => (value.length > 100 ? "At most 100 characters." : undefined),
        }}
      >
        {(f) => <TextInput field={f} label="Organization display name" />}
      </form.Field>
      <form.Field name="email" validators={{ onChange: emailCheck }}>
        {(f) => <TextInput field={f} label="Public email" type="email" />}
      </form.Field>
      <form.Field
        name="description"
        validators={{
          onChange: ({ value }) => (value.length > 160 ? "At most 160 characters." : undefined),
        }}
      >
        {(f) => (
          <TextInput
            field={f}
            label="Description"
            multiline
            help={`${f.state.value.length} of 160 characters. Shown under the name in the header.`}
          />
        )}
      </form.Field>
      <form.Field
        name="blog"
        validators={{
          onChange: ({ value }) =>
            value.trim() && !/^https?:\/\/\S+\.\S+$/.test(value.trim())
              ? "Use a full address, such as https://example.com."
              : undefined,
        }}
      >
        {(f) => <TextInput field={f} label="URL" type="url" />}
      </form.Field>
      <form.Field name="location">{(f) => <TextInput field={f} label="Location" />}</form.Field>
      <form.Field name="company">{(f) => <TextInput field={f} label="Company" />}</form.Field>
      <form.Field
        name="billing_email"
        validators={{
          onChange: ({ value }) =>
            !value.trim() ? "A billing email is required." : emailCheck({ value }),
        }}
      >
        {(f) => (
          <TextInput
            field={f}
            label="Billing email"
            type="email"
            help="Private. Receipts and license notices go here."
          />
        )}
      </form.Field>
      <FormFailure>{failure}</FormFailure>
      <form.Subscribe selector={(s) => [s.isSubmitting, s.isDirty] as const}>
        {([submitting, dirty]) => (
          <Button
            type="submit"
            className="self-start"
            disabled={submitting || !dirty}
            data-testid="save-profile"
          >
            {submitting ? "Saving…" : "Update profile"}
          </Button>
        )}
      </form.Subscribe>
    </form>
  );
}

// ---- Danger zone ----

export function DangerZone({ org }: { org: string }) {
  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-sm font-semibold">Danger zone</h2>
      <div
        className="border-destructive/50 divide-destructive/30 divide-y rounded-sm border"
        data-testid="danger-zone"
      >
        <RenameRow org={org} />
        <DeleteRow org={org} />
      </div>
    </section>
  );
}

function RenameRow({ org }: { org: string }) {
  const { data: pending } = useSuspenseQuery(renameQuery(org));
  const client = useQueryClient();
  const [open, setOpen] = useState(false);
  const [login, setLogin] = useState("");
  const [error, setError] = useState<string>();
  const key = renameQuery(org).queryKey;
  const rename = useMutation({
    mutationFn: (next: string) =>
      send(ext.POST("/orgs/{org}/rename", { params: { path: { org } }, body: { login: next } })),
    onSuccess: (r) => {
      client.setQueryData(key, r);
      setOpen(false);
      setLogin("");
      toast.success("Rename queued", { description: r.message });
    },
    onError: (e) => {
      setError(e instanceof Error ? e.message : String(e));
      failed("Rename not queued")(e);
    },
  });
  const cancel = useMutation({
    mutationFn: () => send(ext.DELETE("/orgs/{org}/rename", { params: { path: { org } } })),
    onMutate: async () => {
      await client.cancelQueries({ queryKey: key });
      const before = client.getQueryData<PendingRename>(key);
      client.setQueryData<PendingRename>(key, { login: null, queued_at: null, message: "" });
      return before;
    },
    onError: (e, _v, before) => {
      client.setQueryData(key, before);
      failed("Rename not cancelled")(e);
    },
    onSuccess: () => toast.success("Rename cancelled"),
    onSettled: () => refetch(key),
  });
  return (
    <SettingRow
      title="Rename organization"
      description={
        pending.login ? (
          <span data-pending-rename={pending.login}>
            Becomes <strong>{pending.login}</strong> in the next maintenance window. Queued{" "}
            {pending.queued_at && <Ago at={pending.queued_at} />}.
          </span>
        ) : (
          "Renaming changes every URL. Graftyard redirects the old ones, but a new org can claim the old name."
        )
      }
      control={
        pending.login ? (
          <Button
            variant="outline"
            size="sm"
            onClick={() => cancel.mutate()}
            disabled={cancel.isPending}
          >
            Cancel rename
          </Button>
        ) : (
          <Button
            variant="outline"
            size="sm"
            onClick={() => setOpen(true)}
            data-testid="rename-org"
          >
            Rename organization
          </Button>
        )
      }
    >
      <Confirm
        open={open}
        onOpenChange={(o) => {
          setOpen(o);
          if (!o) setError(undefined);
        }}
        title={`Rename ${org}`}
        description="The rename runs in the next maintenance window. Until then the org keeps its name."
        phrase={org}
        action="Queue rename"
        pending={rename.isPending}
        onConfirm={() => {
          const next = login.trim();
          if (!next) return setError("Choose a new name.");
          setError(undefined);
          rename.mutate(next);
        }}
      >
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="org-new-login">New name</Label>
          <Input
            id="org-new-login"
            value={login}
            onChange={(e) => {
              setLogin(e.target.value);
              setError(undefined);
            }}
            aria-invalid={Boolean(error) || undefined}
            autoComplete="off"
          />
          <FormFailure>{error}</FormFailure>
        </div>
      </Confirm>
    </SettingRow>
  );
}

function DeleteRow({ org }: { org: string }) {
  const [open, setOpen] = useState(false);
  const navigate = useNavigate();
  const client = useQueryClient();
  const del = useMutation({
    mutationFn: () => send(api.DELETE("/orgs/{org}", { params: { path: { org } } })),
    onError: failed("Organization not deleted"),
    onSuccess: () => {
      setOpen(false);
      toast.success(`Deleted ${org}`, { description: "Its repositories are queued for deletion." });
      client.removeQueries({ queryKey: ["orgs", org] });
      void refetch(userOrgsQuery().queryKey);
      void navigate({ to: "/", replace: true });
    },
  });
  return (
    <SettingRow
      title="Delete this organization"
      description="Deletes the organization, its teams, and every repository it owns. There is no undo."
      control={
        <Button
          variant="destructive"
          size="sm"
          onClick={() => setOpen(true)}
          data-testid="delete-org"
        >
          Delete organization
        </Button>
      }
    >
      <Confirm
        open={open}
        onOpenChange={setOpen}
        title={`Delete ${org}?`}
        description="Every repository, team, and setting goes with it."
        phrase={org}
        action="Delete this organization"
        pending={del.isPending}
        onConfirm={() => del.mutate()}
      />
    </SettingRow>
  );
}

// ---- Member privileges ----

type PrivilegeKey = keyof MemberPrivileges;

// Settings in PATCH /orgs/{org} save there; the rest of the page saves through the private
// UI API. Both answer the whole org, so the switches read one record.
export function MemberPrivilegesForm({ org }: { org: string }) {
  const { data } = useSuspenseQuery(orgQuery(org));
  const client = useQueryClient();
  const patch = useOrgPatch(org, "Setting not saved");
  const privilege = useMutation({
    mutationFn: (body: Partial<MemberPrivileges>) =>
      send(ui.PATCH("/orgs/{org}/member-privileges", { params: { path: { org } }, body })),
    onMutate: async (body) => {
      const key = orgQuery(org).queryKey;
      await client.cancelQueries({ queryKey: key });
      const before = client.getQueryData<Org>(key);
      client.setQueryData<Org>(key, (old) => old && { ...old, ...body });
      return before;
    },
    onError: (e, _b, before) => {
      client.setQueryData(orgQuery(org).queryKey, before);
      failed("Setting not saved")(e);
    },
    onSuccess: (o) => client.setQueryData(orgQuery(org).queryKey, o),
  });
  const saved = () => toast.success("Saved");
  const flag = (key: keyof OrgPatch & keyof Org, label: string) => (
    <Switch
      aria-label={label}
      data-setting={key}
      checked={Boolean(data[key])}
      onCheckedChange={(on) => patch.mutate({ [key]: on }, { onSuccess: saved })}
    />
  );
  const extra = (key: PrivilegeKey, label: string) => (
    <Switch
      aria-label={label}
      data-setting={key}
      checked={Boolean(data[key])}
      onCheckedChange={(on) => privilege.mutate({ [key]: on }, { onSuccess: saved })}
    />
  );
  const creating = data.members_can_create_repositories !== false;
  return (
    <div className="flex flex-col gap-4">
      <Panel title="Base permissions" flush>
        <SettingRow
          title="Base permission"
          description="Every member gets at least this access to every repository. Teams and collaborators add more."
          control={
            <NativeSelect
              size="sm"
              aria-label="Base permission"
              value={data.default_repository_permission ?? "read"}
              data-setting="default_repository_permission"
              onChange={(e) =>
                patch.mutate(
                  {
                    default_repository_permission: e.target.value as NonNullable<
                      OrgPatch["default_repository_permission"]
                    >,
                  },
                  { onSuccess: saved },
                )
              }
            >
              <NativeSelectOption value="none">No permission</NativeSelectOption>
              <NativeSelectOption value="read">Read</NativeSelectOption>
              <NativeSelectOption value="write">Write</NativeSelectOption>
              <NativeSelectOption value="admin">Admin</NativeSelectOption>
            </NativeSelect>
          }
        />
      </Panel>
      <Panel title="Repository creation" flush>
        <div className="divide-y">
          <SettingRow
            title="Members can create repositories"
            description="Owners can always create repositories."
            control={flag("members_can_create_repositories", "Members can create repositories")}
          />
          {(
            [
              ["members_can_create_public_repositories", "Public"],
              ["members_can_create_internal_repositories", "Internal"],
              ["members_can_create_private_repositories", "Private"],
            ] as const
          ).map(([key, label]) => (
            <SettingRow
              key={key}
              title={<span className="pl-4">{label}</span>}
              control={
                <Switch
                  aria-label={`Members can create ${label.toLowerCase()} repositories`}
                  data-setting={key}
                  checked={Boolean(data[key])}
                  disabled={!creating}
                  onCheckedChange={(on) => patch.mutate({ [key]: on }, { onSuccess: saved })}
                />
              }
            />
          ))}
        </div>
      </Panel>
      <Panel title="Repository forking, pages, and visibility" flush>
        <div className="divide-y">
          <SettingRow
            title="Allow forking of private and internal repositories"
            description="Members with read access can fork into their own accounts."
            control={flag(
              "members_can_fork_private_repositories",
              "Allow forking of private repositories",
            )}
          />
          <SettingRow
            title="Members can create Pages sites"
            control={flag("members_can_create_pages", "Members can create Pages sites")}
          />
          <SettingRow
            title="Require sign-off on web-based commits"
            control={flag("web_commit_signoff_required", "Require sign-off on web-based commits")}
          />
          <SettingRow
            title="Members can change repository visibility"
            description="Admins of a repository may switch it between public, internal, and private."
            control={extra(
              "members_can_change_repo_visibility",
              "Members can change repository visibility",
            )}
          />
          <SettingRow
            title="Members can delete or transfer repositories"
            control={extra("members_can_delete_repositories", "Members can delete repositories")}
          />
        </div>
      </Panel>
      <Panel title="Collaboration" flush>
        <div className="divide-y">
          <SettingRow
            title="Repository admins can invite outside collaborators"
            control={extra(
              "members_can_invite_outside_collaborators",
              "Admins can invite outside collaborators",
            )}
          />
          <SettingRow
            title="Members can delete issues"
            control={extra("members_can_delete_issues", "Members can delete issues")}
          />
          <SettingRow
            title="Readers can create discussions"
            control={extra("readers_can_create_discussions", "Readers can create discussions")}
          />
          <SettingRow
            title="Show commenters' full names"
            control={extra(
              "display_commenter_full_name_setting_enabled",
              "Show commenters' full names",
            )}
          />
        </div>
      </Panel>
    </div>
  );
}

// ---- Teams settings ----

export function TeamSettingsForm({ org }: { org: string }) {
  const { data } = useSuspenseQuery(teamSettingsQuery(org));
  const client = useQueryClient();
  const key = teamSettingsQuery(org).queryKey;
  const save = useMutation({
    mutationFn: (body: Partial<TeamSettings>) =>
      send(ui.PATCH("/orgs/{org}/team-settings", { params: { path: { org } }, body })),
    onMutate: async (body) => {
      await client.cancelQueries({ queryKey: key });
      const before = client.getQueryData<TeamSettings>(key);
      client.setQueryData<TeamSettings>(key, (old) => old && { ...old, ...body });
      return before;
    },
    onError: (e, _b, before) => {
      client.setQueryData(key, before);
      failed("Team setting not saved")(e);
    },
    onSuccess: (s) => {
      client.setQueryData(key, s);
      toast.success("Saved");
    },
    onSettled: () => refetch(key, orgQuery(org).queryKey),
  });
  return (
    <Panel flush>
      <div className="divide-y" data-testid="team-settings">
        <SettingRow
          title="Members can create teams"
          description="When off, only owners create teams."
          control={
            <Switch
              aria-label="Members can create teams"
              data-setting="members_can_create_teams"
              checked={data.members_can_create_teams}
              onCheckedChange={(on) => save.mutate({ members_can_create_teams: on })}
            />
          }
        />
        <SettingRow
          title="Default visibility for new teams"
          description="Secret teams can't be nested."
          control={
            <NativeSelect
              size="sm"
              aria-label="Default visibility"
              value={data.default_privacy}
              onChange={(e) =>
                save.mutate({ default_privacy: e.target.value as TeamSettings["default_privacy"] })
              }
            >
              <NativeSelectOption value="closed">Visible</NativeSelectOption>
              <NativeSelectOption value="secret">Secret</NativeSelectOption>
            </NativeSelect>
          }
        />
        <SettingRow
          title="Notify members on @mention by default"
          control={
            <Switch
              aria-label="Notify members on mention"
              checked={data.default_notification_setting === "notifications_enabled"}
              onCheckedChange={(on) =>
                save.mutate({
                  default_notification_setting: on
                    ? "notifications_enabled"
                    : "notifications_disabled",
                })
              }
            />
          }
        />
        <SettingRow
          title="Team discussions"
          description="GHES 3.22 retired team discussions in favour of repository discussions."
          control={
            <Switch
              aria-label="Team discussions"
              checked={data.team_discussions_enabled}
              onCheckedChange={(on) => save.mutate({ team_discussions_enabled: on })}
            />
          }
        />
      </div>
    </Panel>
  );
}
