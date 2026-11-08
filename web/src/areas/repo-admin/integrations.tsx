import { useState } from "react";
import { useMutation, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { Link } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { Delete02Icon, Key01Icon, Link01Icon, PuzzleIcon } from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api, type Schemas } from "@/api/client";
import { repoQuery } from "@/areas/core/queries";
import { Ago } from "@/components/github";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Textarea } from "@/components/ui/textarea";
import { call, failed, optimistic, refetch } from "./client";
import { applyServerErrors, EmptyRow, FieldMessage, TypedConfirm, type RepoParams } from "./kit";
import { autolinksQuery, deployKeysQuery, installationsQuery, preReceiveQuery } from "./queries";

// Autolinks, deploy keys, pre-receive hooks, and installed GitHub Apps.

// ---- Autolinks ----

type Autolink = Schemas["autolink"];

export function AutolinkForm({ params }: { params: RepoParams }) {
  const [failure, setFailure] = useState<string>();
  const key = autolinksQuery(params).queryKey;
  const form = useForm({
    defaultValues: { key_prefix: "", url_template: "", is_alphanumeric: true },
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      try {
        const created = await call(
          api.POST("/repos/{owner}/{repo}/autolinks", { params: { path: params }, body: value }),
        );
        toast.success(`Added ${created.key_prefix} autolink`);
        formApi.reset();
      } catch (e) {
        setFailure(
          applyServerErrors(formApi, e, { key_prefix: "key_prefix", url_template: "url_template" }),
        );
        failed("Autolink not added")(e);
      } finally {
        void refetch(key);
      }
    },
  });
  return (
    <form
      className="grid gap-3 px-4 py-3 sm:grid-cols-[12rem_minmax(0,1fr)]"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
      data-testid="autolink-form"
    >
      <form.Field
        name="key_prefix"
        validators={{
          onChange: ({ value }) =>
            !value.trim()
              ? "A prefix is required."
              : /\s/.test(value)
                ? "No spaces in a prefix."
                : undefined,
        }}
      >
        {(field) => (
          <label className="flex flex-col gap-1 text-sm font-medium">
            Reference prefix
            <Input
              placeholder="TICKET-"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
              aria-invalid={field.state.meta.errors.length > 0 || undefined}
            />
            <FieldMessage field={field} />
          </label>
        )}
      </form.Field>
      <form.Field
        name="url_template"
        validators={{
          onChange: ({ value }) =>
            !value.includes("<num>")
              ? "The URL must contain <num>."
              : !/^https?:\/\//.test(value)
                ? "Start the URL with http:// or https://."
                : undefined,
        }}
      >
        {(field) => (
          <label className="flex flex-col gap-1 text-sm font-medium">
            Target URL
            <Input
              placeholder="https://tracker.example.com/TICKET-<num>"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
              aria-invalid={field.state.meta.errors.length > 0 || undefined}
            />
            <FieldMessage field={field} />
          </label>
        )}
      </form.Field>
      <form.Field name="is_alphanumeric">
        {(field) => (
          <label className="flex items-center gap-2 text-sm sm:col-span-2">
            <Checkbox
              checked={field.state.value}
              onCheckedChange={(v) => field.handleChange(Boolean(v))}
            />
            Alphanumeric:{" "}
            <span className="text-muted-foreground">
              match letters and numbers after the prefix, not just numbers
            </span>
          </label>
        )}
      </form.Field>
      <form.Subscribe
        selector={(s) => [s.isSubmitting, s.values.key_prefix, s.values.url_template] as const}
      >
        {([submitting, prefix, template]) => (
          <div className="flex flex-wrap items-center gap-3 sm:col-span-2">
            <Button type="submit" size="sm" disabled={submitting}>
              {submitting ? "Adding…" : "Add autolink reference"}
            </Button>
            {prefix && template.includes("<num>") && (
              <span className="text-muted-foreground text-xs">
                {prefix}123 links to{" "}
                <code className="font-mono">{template.replace("<num>", "123")}</code>
              </span>
            )}
          </div>
        )}
      </form.Subscribe>
      {failure && (
        <p className="text-destructive text-xs sm:col-span-2" role="alert">
          {failure}
        </p>
      )}
    </form>
  );
}

export function Autolinks({ params }: { params: RepoParams }) {
  const { data } = useSuspenseQuery(autolinksQuery(params));
  const key = autolinksQuery(params).queryKey;
  const del = useMutation({
    mutationFn: (id: number) =>
      call(
        api.DELETE("/repos/{owner}/{repo}/autolinks/{autolink_id}", {
          params: { path: { ...params, autolink_id: id } },
        }),
      ),
    onMutate: (id) => optimistic<Autolink[]>(key, (old) => old.filter((a) => a.id !== id)),
    onError: (e, _id, rollback) => {
      rollback?.();
      failed("Autolink not deleted")(e);
    },
    onSuccess: () => toast.success("Autolink deleted"),
    onSettled: () => refetch(key),
  });
  if (data.length === 0) return <EmptyRow>No autolink references.</EmptyRow>;
  return (
    <ul className="divide-y" data-testid="autolinks">
      {data.map((a) => (
        <li key={a.id} className="flex items-center gap-3 px-4 py-2" data-prefix={a.key_prefix}>
          <HugeiconsIcon
            icon={Link01Icon}
            strokeWidth={2}
            className="text-muted-foreground size-4"
          />
          <div className="min-w-0 flex-1">
            <div className="font-mono text-sm font-semibold">
              {a.key_prefix}&lt;{a.is_alphanumeric ? "alphanumeric" : "num"}&gt;
            </div>
            <div className="text-muted-foreground truncate font-mono text-xs">{a.url_template}</div>
          </div>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={`Delete ${a.key_prefix}`}
            onClick={() => del.mutate(a.id)}
          >
            <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
          </Button>
        </li>
      ))}
    </ul>
  );
}

// ---- Deploy keys ----

type DeployKey = Schemas["deploy-key"];

export function DeployKeyForm({ params, onDone }: { params: RepoParams; onDone?: () => void }) {
  const [failure, setFailure] = useState<string>();
  const form = useForm({
    defaultValues: { title: "", key: "", write: false },
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      try {
        const created = await call(
          api.POST("/repos/{owner}/{repo}/keys", {
            params: { path: params },
            body: { title: value.title, key: value.key.trim(), read_only: !value.write },
          }),
        );
        toast.success(`Added deploy key ${created.title}`);
        formApi.reset();
        onDone?.();
      } catch (e) {
        setFailure(applyServerErrors(formApi, e, { key: "key", title: "title" }));
        failed("Deploy key not added")(e);
      } finally {
        void refetch(deployKeysQuery(params).queryKey);
      }
    },
  });
  return (
    <form
      className="flex flex-col gap-3 px-4 py-3"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
      data-testid="deploy-key-form"
    >
      <form.Field
        name="title"
        validators={{
          onChange: ({ value }) => (!value.trim() ? "Give the key a title." : undefined),
        }}
      >
        {(field) => (
          <label className="flex max-w-md flex-col gap-1 text-sm font-medium">
            Title
            <Input
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
              aria-invalid={field.state.meta.errors.length > 0 || undefined}
            />
            <FieldMessage field={field} />
          </label>
        )}
      </form.Field>
      <form.Field
        name="key"
        validators={{
          onChange: ({ value }) =>
            !value.trim()
              ? "Paste a public key."
              : !/^(ssh-(ed25519|rsa|dss)|ecdsa-sha2-nistp\d+|sk-\S+@openssh\.com) \S+/.test(
                    value.trim(),
                  )
                ? "Begins with ssh-ed25519, ssh-rsa, ecdsa-sha2-nistp256, or sk-…@openssh.com."
                : undefined,
        }}
      >
        {(field) => (
          <label className="flex flex-col gap-1 text-sm font-medium">
            Key
            <Textarea
              rows={3}
              className="font-mono text-xs"
              placeholder="ssh-ed25519 AAAA… comment"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
              aria-invalid={field.state.meta.errors.length > 0 || undefined}
            />
            <FieldMessage field={field} />
          </label>
        )}
      </form.Field>
      <form.Field name="write">
        {(field) => (
          <label className="flex items-start gap-2 text-sm">
            <Checkbox
              className="mt-0.5"
              checked={field.state.value}
              onCheckedChange={(v) => field.handleChange(Boolean(v))}
            />
            <span>
              <span className="font-medium">Allow write access</span>
              <span className="text-muted-foreground block text-xs">
                Can this key push to the repository? A read-only key can still clone it.
              </span>
            </span>
          </label>
        )}
      </form.Field>
      {failure && (
        <p className="text-destructive text-xs" role="alert">
          {failure}
        </p>
      )}
      <form.Subscribe selector={(s) => s.isSubmitting}>
        {(submitting) => (
          <div>
            <Button type="submit" size="sm" disabled={submitting}>
              {submitting ? "Adding…" : "Add key"}
            </Button>
          </div>
        )}
      </form.Subscribe>
    </form>
  );
}

export function DeployKeys({ params }: { params: RepoParams }) {
  const { data } = useSuspenseQuery(deployKeysQuery(params));
  const [confirm, setConfirm] = useState<DeployKey>();
  const key = deployKeysQuery(params).queryKey;
  const del = useMutation({
    mutationFn: (id: number) =>
      call(
        api.DELETE("/repos/{owner}/{repo}/keys/{key_id}", {
          params: { path: { ...params, key_id: id } },
        }),
      ),
    onMutate: (id) => optimistic<DeployKey[]>(key, (old) => old.filter((k) => k.id !== id)),
    onError: (e, _id, rollback) => {
      rollback?.();
      failed("Deploy key not deleted")(e);
    },
    onSuccess: () => {
      toast.success("Deploy key deleted");
      setConfirm(undefined);
    },
    onSettled: () => refetch(key),
  });
  if (data.length === 0)
    return (
      <EmptyRow>
        No deploy keys. A deploy key gives one machine access to this repository only.
      </EmptyRow>
    );
  return (
    <>
      <ul className="divide-y" data-testid="deploy-keys">
        {data.map((k) => (
          <li key={k.id} className="flex items-center gap-3 px-4 py-2.5" data-key-title={k.title}>
            <HugeiconsIcon
              icon={Key01Icon}
              strokeWidth={2}
              className="text-muted-foreground size-5"
            />
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2 text-sm font-semibold">
                {k.title}
                <Badge variant={k.read_only ? "outline" : "warning-light"}>
                  {k.read_only ? "Read-only" : "Read/write"}
                </Badge>
              </div>
              <div className="text-muted-foreground truncate font-mono text-xs">
                {fingerprint(k.key)}
              </div>
              <div className="text-muted-foreground text-xs">
                Added <Ago at={k.created_at} /> by {k.added_by ?? "unknown"} ·{" "}
                {k.last_used ? (
                  <>
                    last used <Ago at={k.last_used} />
                  </>
                ) : (
                  "never used"
                )}
              </div>
            </div>
            <Button
              variant="destructive"
              size="sm"
              aria-label={`Delete key ${k.title}`}
              onClick={() => setConfirm(k)}
            >
              Delete
            </Button>
          </li>
        ))}
      </ul>
      <TypedConfirm
        open={confirm !== undefined}
        onOpenChange={(o) => !o && setConfirm(undefined)}
        title="Delete this deploy key"
        description="Machines that use it lose access to this repository at once."
        phrase={confirm?.title ?? ""}
        action="Delete key"
        pending={del.isPending}
        onConfirm={() => confirm && del.mutate(confirm.id)}
      />
    </>
  );
}

// A short, stable label for a key: its type and a slice of its body, as SHA256 fingerprints read.
function fingerprint(key: string) {
  const [type, body = ""] = key.split(/\s+/);
  return `${type} SHA256:${body.slice(-43, -11)}…`;
}

// ---- Pre-receive hooks ----

type PreReceive = Schemas["repository-pre-receive-hook"];
const ENFORCE = ["enabled", "testing", "disabled"] as const;

export function PreReceiveHooks({ params }: { params: RepoParams }) {
  const { data } = useSuspenseQuery(preReceiveQuery(params));
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const key = preReceiveQuery(params).queryKey;
  const update = useMutation({
    mutationFn: (v: { id: number; enforcement: (typeof ENFORCE)[number] }) =>
      call(
        api.PATCH("/repos/{owner}/{repo}/pre-receive-hooks/{pre_receive_hook_id}", {
          params: { path: { ...params, pre_receive_hook_id: v.id } },
          body: { enforcement: v.enforcement },
        }),
      ),
    onMutate: (v) =>
      optimistic<PreReceive[]>(key, (old) =>
        old.map((h) => (h.id === v.id ? { ...h, enforcement: v.enforcement } : h)),
      ),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed("Enforcement not changed")(e);
    },
    onSuccess: (h) => toast.success(`${h.name} is now ${h.enforcement}`),
    onSettled: () => refetch(key),
  });
  const reset = useMutation({
    mutationFn: (id: number) =>
      call(
        api.DELETE("/repos/{owner}/{repo}/pre-receive-hooks/{pre_receive_hook_id}", {
          params: { path: { ...params, pre_receive_hook_id: id } },
        }),
      ),
    onError: failed("Enforcement not reset"),
    onSuccess: (h) => toast.success(`${h.name} follows the instance default (${h.enforcement})`),
    onSettled: () => refetch(key),
  });
  if (data.length === 0)
    return <EmptyRow>The site administrator has not installed pre-receive hooks.</EmptyRow>;
  return (
    <ul className="divide-y" data-testid="pre-receive-hooks">
      {data.map((h) => (
        <li key={h.id} className="flex items-center gap-3 px-4 py-2.5" data-hook={h.name}>
          <div className="min-w-0 flex-1">
            <div className="font-mono text-sm font-semibold">{h.name}</div>
            <div className="text-muted-foreground text-xs">
              {h.enforcement === "enabled"
                ? "Rejects pushes that fail the script."
                : h.enforcement === "testing"
                  ? "Runs on every push and reports failures without rejecting."
                  : "Does not run on this repository."}
            </div>
          </div>
          <NativeSelect
            size="sm"
            aria-label={`Enforcement for ${h.name}`}
            value={h.enforcement}
            disabled={repo.archived}
            onChange={(e) =>
              update.mutate({ id: h.id!, enforcement: e.target.value as (typeof ENFORCE)[number] })
            }
          >
            {ENFORCE.map((x) => (
              <NativeSelectOption key={x} value={x}>
                {x[0].toUpperCase() + x.slice(1)}
              </NativeSelectOption>
            ))}
          </NativeSelect>
          <Button variant="ghost" size="sm" onClick={() => reset.mutate(h.id!)}>
            Use default
          </Button>
        </li>
      ))}
    </ul>
  );
}

// ---- Installed GitHub Apps ----

export function Installations({ params }: { params: RepoParams }) {
  const { data } = useSuspenseQuery(installationsQuery(params));
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  if (data.length === 0)
    return <EmptyRow>No GitHub Apps are installed on this repository.</EmptyRow>;
  return (
    <ul className="divide-y" data-testid="installations">
      {data.map((i) => (
        <li key={i.id} className="flex items-start gap-3 px-4 py-2.5" data-app={i.app_slug}>
          <HugeiconsIcon
            icon={PuzzleIcon}
            strokeWidth={2}
            className="text-muted-foreground mt-0.5 size-5"
          />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2 text-sm font-semibold">
              {i.app_slug}
              <Badge variant="outline">
                {i.repository_selection === "all" ? "All repositories" : "Selected repositories"}
              </Badge>
              {i.suspended_at && <Badge variant="destructive-light">Suspended</Badge>}
            </div>
            <div className="text-muted-foreground mt-0.5 flex flex-wrap gap-1 text-xs">
              {Object.entries(i.permissions).map(([k, v]) => (
                <span key={k} className="bg-muted rounded-sm px-1">
                  {k.replace(/_/g, " ")}: {v}
                </span>
              ))}
            </div>
            <div className="text-muted-foreground mt-0.5 text-xs">
              Installed <Ago at={i.created_at} /> · events: {i.events.join(", ")}
            </div>
          </div>
          {repo.owner.type === "Organization" && (
            <Button
              variant="outline"
              size="sm"
              render={<Link to="/orgs/$org/settings" params={{ org: repo.owner.login }} />}
            >
              Configure
            </Button>
          )}
        </li>
      ))}
    </ul>
  );
}
