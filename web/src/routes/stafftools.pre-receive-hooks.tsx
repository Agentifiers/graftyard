import { useEffect, useState, type ReactNode } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { useForm, type AnyFieldApi } from "@tanstack/react-form";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Add01Icon,
  Delete02Icon,
  Download04Icon,
  PencilEdit02Icon,
} from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import {
  envDownloadQuery,
  preReceiveEnvsQuery,
  preReceiveHooksQuery,
  staffReposQuery,
} from "@/areas/admin/queries";
import {
  message,
  send,
  showFieldErrors,
  StateBadge,
  TextField,
  useWrite,
  when,
} from "@/areas/admin/ui";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
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
import { Spinner } from "@/components/ui/spinner";

const TABS = ["hooks", "environments"] as const;
type Tab = (typeof TABS)[number];

export const Route = createFileRoute("/stafftools/pre-receive-hooks")({
  validateSearch: (s: Record<string, unknown>): { tab?: Tab } => ({
    tab: s.tab === "environments" ? "environments" : undefined,
  }),
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(preReceiveHooksQuery());
    void queryClient.prefetchQuery(preReceiveEnvsQuery());
  },
  component: Page,
});

type Hook = Schemas["pre-receive-hook"];
type Env = Schemas["pre-receive-environment"];
const HOOKS = preReceiveHooksQuery().queryKey;
const ENVS = preReceiveEnvsQuery().queryKey;
const ENFORCEMENTS = ["enabled", "testing", "disabled"] as const;
const enforcementTone = (e?: string) =>
  e === "enabled" ? "success-light" : e === "testing" ? "warning-light" : "outline";

function Page() {
  const { tab = "hooks" } = Route.useSearch();
  return (
    <>
      <PageHeader
        title="Pre-receive hooks"
        description="Scripts that run on every push and can reject it. A hook runs in an environment, a chroot image downloaded to every node."
      />
      <div className="mb-3 flex gap-1 border-b text-sm" role="tablist">
        {TABS.map((t) => (
          <Link
            key={t}
            from={Route.fullPath}
            search={{ tab: t === "hooks" ? undefined : t }}
            role="tab"
            aria-selected={tab === t}
            className="text-muted-foreground aria-selected:border-primary aria-selected:text-foreground -mb-px border-b-2 border-transparent px-3 py-1.5 font-medium capitalize"
          >
            {t}
          </Link>
        ))}
      </div>
      {tab === "hooks" ? (
        <Region name="pre-receive hooks" rows={4}>
          <Hooks />
        </Region>
      ) : (
        <Region name="pre-receive environments" rows={4}>
          <Envs />
        </Region>
      )}
    </>
  );
}

// ---- Hooks ----

function Hooks() {
  const { data } = useSuspenseQuery(preReceiveHooksQuery());
  const [editing, setEditing] = useState<Hook | "new">();
  const [deleting, setDeleting] = useState<Hook>();
  const queryClient = useQueryClient();
  const setEnforcement = useWrite({
    run: async ({ hook, enforcement }: { hook: Hook; enforcement: string }) => {
      const before = queryClient.getQueryData(HOOKS);
      queryClient.setQueryData(HOOKS, (old) =>
        old?.map((h) => (h.id === hook.id ? { ...h, enforcement } : h)),
      );
      try {
        return await send(
          api.PATCH("/admin/pre-receive-hooks/{pre_receive_hook_id}", {
            params: { path: { pre_receive_hook_id: hook.id! } },
            body: { enforcement },
          }),
        );
      } catch (e) {
        queryClient.setQueryData(HOOKS, before);
        throw e;
      }
    },
    invalidate: [HOOKS],
    success: (h) => `${h.name} is ${h.enforcement}`,
  });
  const remove = useWrite({
    run: (hook: Hook) =>
      send(
        api.DELETE("/admin/pre-receive-hooks/{pre_receive_hook_id}", {
          params: { path: { pre_receive_hook_id: hook.id! } },
        }),
      ),
    invalidate: [HOOKS, ENVS],
    success: (_, h) => `Deleted ${h.name}`,
    onSuccess: () => setDeleting(undefined),
  });
  return (
    <Panel
      title={`${data.length} hooks`}
      actions={
        <Button size="sm" onClick={() => setEditing("new")}>
          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
          Add hook
        </Button>
      }
      flush
    >
      {data.length === 0 ? (
        <p className="text-muted-foreground p-6 text-center text-sm">No pre-receive hooks yet.</p>
      ) : (
        <ul className="divide-y" data-testid="pre-receive-hooks">
          {data.map((h) => (
            <li key={h.id} className="flex flex-wrap items-center gap-3 px-4 py-2.5 text-sm">
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="font-semibold">{h.name}</span>
                  {h.allow_downstream_configuration && (
                    <Badge variant="outline" size="sm">
                      orgs and repos may override
                    </Badge>
                  )}
                </div>
                <div className="text-muted-foreground text-xs">
                  <code>{h.script}</code> in {h.script_repository?.full_name}, runs in{" "}
                  {h.environment?.name}
                </div>
              </div>
              <NativeSelect
                size="sm"
                aria-label={`Enforcement of ${h.name}`}
                value={h.enforcement}
                disabled={setEnforcement.isPending}
                onChange={(e) => setEnforcement.mutate({ hook: h, enforcement: e.target.value })}
              >
                {ENFORCEMENTS.map((x) => (
                  <NativeSelectOption key={x} value={x}>
                    {x}
                  </NativeSelectOption>
                ))}
              </NativeSelect>
              <StateBadge tone={enforcementTone(h.enforcement)}>{h.enforcement}</StateBadge>
              <Button
                size="icon-sm"
                variant="ghost"
                aria-label={`Edit ${h.name}`}
                onClick={() => setEditing(h)}
              >
                <HugeiconsIcon icon={PencilEdit02Icon} strokeWidth={2} />
              </Button>
              <Button
                size="icon-sm"
                variant="ghost"
                aria-label={`Delete ${h.name}`}
                onClick={() => setDeleting(h)}
              >
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
              </Button>
            </li>
          ))}
        </ul>
      )}
      {editing && (
        <Region name="hook form" fallback={null}>
          <HookForm
            hook={editing === "new" ? undefined : editing}
            onClose={() => setEditing(undefined)}
          />
        </Region>
      )}
      <AlertDialog open={!!deleting} onOpenChange={(o) => !o && setDeleting(undefined)}>
        <AlertDialogContent className="rounded-sm">
          <AlertDialogHeader>
            <AlertDialogTitle>Delete {deleting?.name}?</AlertDialogTitle>
            <AlertDialogDescription>
              Pushes stop running it at once, on every repository.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
            <Button
              size="sm"
              variant="destructive"
              disabled={remove.isPending}
              onClick={() => remove.mutate(deleting!)}
            >
              Delete hook
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Panel>
  );
}

function HookForm({ hook, onClose }: { hook?: Hook; onClose: () => void }) {
  const { data: envs } = useSuspenseQuery(preReceiveEnvsQuery());
  const { data: repos } = useSuspenseQuery(staffReposQuery());
  const fields = {
    name: "name",
    script: "script",
    script_repository: "repo",
    environment: "env",
    enforcement: "enforcement",
  };
  const save = useWrite({
    run: (v: {
      name: string;
      script: string;
      repo: string;
      env: number;
      enforcement: string;
      downstream: boolean;
    }) => {
      const body = {
        name: v.name,
        script: v.script,
        script_repository: { full_name: v.repo },
        environment: { id: v.env },
        enforcement: v.enforcement,
        allow_downstream_configuration: v.downstream,
      };
      return hook
        ? send(
            api.PATCH("/admin/pre-receive-hooks/{pre_receive_hook_id}", {
              params: { path: { pre_receive_hook_id: hook.id! } },
              body,
            }),
          )
        : send(api.POST("/admin/pre-receive-hooks", { body }));
    },
    invalidate: [HOOKS, ENVS],
    success: (h) => (hook ? `Saved ${h.name}` : `Added ${h.name}`),
    onSuccess: onClose,
    onError: (e) => showFieldErrors(e, form, fields),
  });
  const form = useForm({
    defaultValues: {
      name: hook?.name ?? "",
      script: hook?.script ?? "",
      repo: hook?.script_repository?.full_name ?? repos[0]?.full_name ?? "",
      env: hook?.environment?.id ?? envs[0]?.id ?? 0,
      enforcement: hook?.enforcement ?? "disabled",
      downstream: hook?.allow_downstream_configuration ?? false,
    },
    onSubmit: ({ value }) =>
      save
        .mutateAsync({ ...value, name: value.name.trim(), script: value.script.trim() })
        .catch(() => {}),
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="rounded-sm sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{hook ? `Edit ${hook.name}` : "Add a pre-receive hook"}</DialogTitle>
          <DialogDescription>
            The script runs from the repository's default branch, inside the environment you pick.
          </DialogDescription>
        </DialogHeader>
        <form
          id="hook-form"
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <form.Field
            name="name"
            validators={{
              onSubmit: ({ value }) => (!value.trim() ? "Name is required." : undefined),
            }}
          >
            {(field) => <TextField field={field} label="Name" placeholder="block-force-push" />}
          </form.Field>
          <div className="grid gap-3 sm:grid-cols-2">
            <form.Field name="repo">
              {(field) => (
                <SelectField field={field} label="Script repository">
                  {repos.map((r) => (
                    <NativeSelectOption key={r.id} value={r.full_name}>
                      {r.full_name}
                    </NativeSelectOption>
                  ))}
                </SelectField>
              )}
            </form.Field>
            <form.Field
              name="script"
              validators={{
                onSubmit: ({ value }) => (!value.trim() ? "Script path is required." : undefined),
              }}
            >
              {(field) => (
                <TextField field={field} label="Script path" placeholder="hooks/check.sh" />
              )}
            </form.Field>
            <form.Field name="env">
              {(field) => (
                <SelectField field={field} label="Environment" number>
                  {envs.map((e) => (
                    <NativeSelectOption key={e.id} value={e.id}>
                      {e.name}
                      {e.download?.state !== "success" ? ` (${e.download?.state})` : ""}
                    </NativeSelectOption>
                  ))}
                </SelectField>
              )}
            </form.Field>
            <form.Field name="enforcement">
              {(field) => (
                <SelectField field={field} label="Enforcement">
                  {ENFORCEMENTS.map((x) => (
                    <NativeSelectOption key={x} value={x}>
                      {x}
                    </NativeSelectOption>
                  ))}
                </SelectField>
              )}
            </form.Field>
          </div>
          <form.Field name="downstream">
            {(field) => (
              <Field orientation="horizontal">
                <Checkbox
                  id="downstream"
                  checked={field.state.value}
                  onCheckedChange={(v) => field.handleChange(!!v)}
                />
                <FieldLabel htmlFor="downstream">
                  Organization and repository admins may change its enforcement
                </FieldLabel>
              </Field>
            )}
          </form.Field>
        </form>
        <DialogFooter>
          <Button size="sm" variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" type="submit" form="hook-form" disabled={save.isPending}>
            {hook ? "Save hook" : "Add hook"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function SelectField({
  field,
  label,
  number,
  children,
}: {
  field: AnyFieldApi;
  label: string;
  number?: boolean;
  children: ReactNode;
}) {
  const errors = field.state.meta.errors.filter(Boolean).map(String);
  return (
    <Field data-invalid={errors.length > 0 || undefined}>
      <FieldLabel htmlFor={`f-${field.name}`}>{label}</FieldLabel>
      <NativeSelect
        id={`f-${field.name}`}
        value={String(field.state.value)}
        onChange={(e) => {
          field.setMeta((m) => ({ ...m, errorMap: { ...m.errorMap, onServer: undefined } }));
          field.handleChange(number ? Number(e.target.value) : e.target.value);
        }}
      >
        {children}
      </NativeSelect>
      {errors.length > 0 && <FieldError>{errors.join(". ")}</FieldError>}
    </Field>
  );
}

// ---- Environments ----

const downloadTone = (s?: string) =>
  s === "success"
    ? "success-light"
    : s === "failed"
      ? "destructive-light"
      : s === "in_progress"
        ? "warning-light"
        : "outline";

function Envs() {
  const { data } = useSuspenseQuery(preReceiveEnvsQuery());
  const [editing, setEditing] = useState<Env | "new">();
  const [deleting, setDeleting] = useState<Env>();
  const remove = useWrite({
    run: (env: Env) =>
      send(
        api.DELETE("/admin/pre-receive-environments/{pre_receive_environment_id}", {
          params: { path: { pre_receive_environment_id: env.id! } },
        }),
      ),
    invalidate: [ENVS],
    success: (_, e) => `Deleted ${e.name}`,
    onSuccess: () => setDeleting(undefined),
    onError: () => {
      setDeleting(undefined);
      return false;
    },
  });
  return (
    <Panel
      title={`${data.length} environments`}
      actions={
        <Button size="sm" onClick={() => setEditing("new")}>
          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
          Add environment
        </Button>
      }
      flush
    >
      <ul className="divide-y" data-testid="pre-receive-envs">
        {data.map((e) => (
          <li key={e.id} className="flex flex-wrap items-center gap-3 px-4 py-2.5 text-sm">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="font-semibold">{e.name}</span>
                {e.default_environment && <Badge size="sm">default</Badge>}
                <span className="text-muted-foreground text-xs">{e.hooks_count} hooks</span>
              </div>
              <code className="text-muted-foreground block truncate text-xs">{e.image_url}</code>
            </div>
            <Download env={e} />
            {!e.default_environment && (
              <>
                <Button
                  size="icon-sm"
                  variant="ghost"
                  aria-label={`Edit ${e.name}`}
                  onClick={() => setEditing(e)}
                >
                  <HugeiconsIcon icon={PencilEdit02Icon} strokeWidth={2} />
                </Button>
                <Button
                  size="icon-sm"
                  variant="ghost"
                  aria-label={`Delete ${e.name}`}
                  onClick={() => setDeleting(e)}
                >
                  <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
                </Button>
              </>
            )}
          </li>
        ))}
      </ul>
      {editing && (
        <EnvForm
          env={editing === "new" ? undefined : editing}
          onClose={() => setEditing(undefined)}
        />
      )}
      <AlertDialog open={!!deleting} onOpenChange={(o) => !o && setDeleting(undefined)}>
        <AlertDialogContent className="rounded-sm">
          <AlertDialogHeader>
            <AlertDialogTitle>Delete {deleting?.name}?</AlertDialogTitle>
            <AlertDialogDescription>
              An environment that hooks still use cannot be deleted.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
            <Button
              size="sm"
              variant="destructive"
              disabled={remove.isPending}
              onClick={() => remove.mutate(deleting!)}
            >
              Delete environment
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Panel>
  );
}

// The download state, polled every second while a download runs.
function Download({ env }: { env: Env }) {
  const queryClient = useQueryClient();
  const [watching, setWatching] = useState(env.download?.state === "in_progress");
  const status = useQuery({
    ...envDownloadQuery(env.id!),
    enabled: watching,
    refetchInterval: 1000,
  });
  const polled = status.data?.state;
  // A finished download stops the poll and refreshes the list's copy.
  useEffect(() => {
    if (watching && (polled === "success" || polled === "failed")) {
      setWatching(false);
      void queryClient.invalidateQueries({ queryKey: ENVS });
    }
  }, [watching, polled, queryClient]);
  const d = watching && status.data ? status.data : env.download;
  const start = useWrite({
    run: () =>
      send(
        api.POST("/admin/pre-receive-environments/{pre_receive_environment_id}/downloads", {
          params: { path: { pre_receive_environment_id: env.id! } },
        }),
      ),
    invalidate: [],
    success: `Downloading ${env.name}`,
    onSuccess: () => {
      queryClient.setQueryData(envDownloadQuery(env.id!).queryKey, {
        ...env.download,
        state: "in_progress",
      });
      setWatching(true);
    },
  });
  const state = d?.state ?? "not_started";
  return (
    <span className="flex items-center gap-2" data-testid={`download-${env.name}`}>
      <StateBadge tone={downloadTone(state)}>
        {state === "in_progress" && <Spinner className="size-3" />}
        <span data-download-state>{state.replace("_", " ")}</span>
      </StateBadge>
      <span className="text-muted-foreground hidden text-xs md:inline">
        {d?.message ? (
          <span className="text-destructive">{d.message}</span>
        ) : d?.downloaded_at ? (
          `downloaded ${when(d.downloaded_at)}`
        ) : null}
      </span>
      {status.error && <span className="text-destructive text-xs">{message(status.error)}</span>}
      {!env.default_environment && (
        <Button
          size="sm"
          variant="outline"
          disabled={start.isPending || state === "in_progress"}
          onClick={() => start.mutate(undefined)}
        >
          <HugeiconsIcon icon={Download04Icon} strokeWidth={2} data-icon="inline-start" />
          {state === "success" ? "Download again" : "Download"}
        </Button>
      )}
    </span>
  );
}

function EnvForm({ env, onClose }: { env?: Env; onClose: () => void }) {
  const save = useWrite({
    run: (body: { name: string; image_url: string }) =>
      env
        ? send(
            api.PATCH("/admin/pre-receive-environments/{pre_receive_environment_id}", {
              params: { path: { pre_receive_environment_id: env.id! } },
              body,
            }),
          )
        : send(api.POST("/admin/pre-receive-environments", { body })),
    invalidate: [ENVS],
    success: (e) =>
      env ? `Saved ${e.name}` : `Added ${e.name}. Download it before hooks can use it.`,
    onSuccess: onClose,
    onError: (e) => showFieldErrors(e, form, { name: "name", image_url: "image_url" }),
  });
  const form = useForm({
    defaultValues: { name: env?.name ?? "", image_url: env?.image_url ?? "" },
    onSubmit: ({ value }) =>
      save
        .mutateAsync({ name: value.name.trim(), image_url: value.image_url.trim() })
        .catch(() => {}),
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="rounded-sm sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{env ? `Edit ${env.name}` : "Add an environment"}</DialogTitle>
          <DialogDescription>
            A tarball of a chroot. Changing the URL needs a new download.
          </DialogDescription>
        </DialogHeader>
        <form
          id="env-form"
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <form.Field
            name="name"
            validators={{
              onSubmit: ({ value }) => (!value.trim() ? "Name is required." : undefined),
            }}
          >
            {(field) => <TextField field={field} label="Name" placeholder="go-1.24" />}
          </form.Field>
          <form.Field
            name="image_url"
            validators={{
              onSubmit: ({ value }) =>
                !/^https?:\/\/\S+$/.test(value.trim()) ? "Enter the tarball's URL." : undefined,
            }}
          >
            {(field) => (
              <TextField
                field={field}
                label="Image URL"
                placeholder="https://artifacts.graftyard.test/pre-receive/go-1.24.tar.gz"
                description="A .tar.gz the appliance can reach."
              />
            )}
          </form.Field>
        </form>
        <DialogFooter>
          <Button size="sm" variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" type="submit" form="env-form" disabled={save.isPending}>
            {env ? "Save" : "Add environment"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
