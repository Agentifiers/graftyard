import { useState } from "react";
import { useForm } from "@tanstack/react-form";
import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { ArrowLeft02Icon, Edit02Icon } from "@hugeicons/core-free-icons";
import { pullState, StateBadge } from "@/components/github";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { BranchName } from "./bits";
import { useSetDraft, useUpdatePull, WriteError } from "./mutations";
import { branchesQuery, pullQuery, type Pull } from "./queries";

// Title (inline edit), state, branches (base can change while open), and the state
// actions: draft or ready, close or reopen.
export function PullHeader({ pull, actions }: { pull: Pull; actions?: React.ReactNode }) {
  const { data: pr } = useSuspenseQuery(pullQuery(pull));
  const [editing, setEditing] = useState(false);
  const update = useUpdatePull(pull, { quiet422: true });
  const draft = useSetDraft(pull);
  const open = pr.state === "open";
  const form = useForm({
    defaultValues: { title: pr.title },
    onSubmit: async ({ value, formApi }) => {
      try {
        await update.mutateAsync({ title: value.title.trim() });
        setEditing(false);
      } catch (e) {
        if (e instanceof WriteError)
          formApi.setFieldMeta("title", (m) => ({
            ...m,
            errorMap: { ...m.errorMap, onServer: e.fields.title ?? e.message },
          }));
      }
    },
  });

  return (
    <div className="flex flex-col gap-2 border-b pb-3" data-testid="pull-header">
      {editing ? (
        <form
          className="flex flex-wrap items-start gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <form.Field
            name="title"
            validators={{
              onSubmit: ({ value }) => (value.trim() ? undefined : "A title is required."),
            }}
          >
            {(f) => (
              <div className="flex min-w-64 flex-1 flex-col gap-1">
                <Input
                  autoFocus
                  aria-label="Title"
                  className="rounded-sm text-base"
                  value={f.state.value}
                  aria-invalid={f.state.meta.errors.length > 0 || undefined}
                  onChange={(e) => f.handleChange(e.target.value)}
                />
                {f.state.meta.errors.length > 0 && (
                  <span className="text-destructive text-xs" role="alert">
                    {f.state.meta.errors.join(", ")}
                  </span>
                )}
              </div>
            )}
          </form.Field>
          <form.Subscribe selector={(s) => s.isSubmitting}>
            {(busy) => (
              <Button type="submit" size="sm" disabled={busy}>
                Save
              </Button>
            )}
          </form.Subscribe>
          <Button
            type="button"
            size="sm"
            variant="ghost"
            onClick={() => (form.reset(), setEditing(false))}
          >
            Cancel
          </Button>
        </form>
      ) : (
        <div className="flex flex-wrap items-start gap-2">
          <h1 className="min-w-0 flex-1 text-xl font-semibold" data-testid="pull-title">
            {pr.title} <span className="text-muted-foreground font-normal">#{pr.number}</span>
          </h1>
          <Button
            size="sm"
            variant="outline"
            onClick={() => (form.setFieldValue("title", pr.title), setEditing(true))}
          >
            <HugeiconsIcon icon={Edit02Icon} strokeWidth={2} data-icon="inline-start" />
            Edit
          </Button>
          {actions}
        </div>
      )}
      <div className="text-muted-foreground flex flex-wrap items-center gap-2 text-sm">
        <StateBadge state={pullState(pr)} />
        <span>
          <span className="text-foreground font-medium">
            {pr.merged_at ? (pr.merged_by?.login ?? pr.user.login) : pr.user.login}
          </span>{" "}
          {pr.merged_at ? "merged" : "wants to merge"} {pr.commits}{" "}
          {pr.commits === 1 ? "commit" : "commits"} into
        </span>
        {open ? (
          <BaseSelect pull={pull} base={pr.base.ref} head={pr.head.ref} />
        ) : (
          <BranchName>{pr.base.ref}</BranchName>
        )}
        <HugeiconsIcon
          icon={ArrowLeft02Icon}
          strokeWidth={2}
          className="size-3.5"
          aria-label="from"
        />
        <BranchName>{pr.head.ref}</BranchName>
        <span className="font-mono text-xs">
          <span className="text-success">+{pr.additions}</span>{" "}
          <span className="text-destructive">-{pr.deletions}</span>
        </span>
        <span className="ml-auto flex gap-1.5">
          {open && (
            <Button
              size="sm"
              variant="ghost"
              disabled={draft.isPending}
              onClick={() => draft.mutate(!pr.draft)}
              data-testid="toggle-draft"
            >
              {pr.draft ? "Ready for review" : "Convert to draft"}
            </Button>
          )}
          {!pr.merged && (
            <Button
              size="sm"
              variant={open ? "outline" : "default"}
              disabled={update.isPending}
              onClick={() => update.mutate({ state: open ? "closed" : "open" })}
              data-testid="toggle-state"
            >
              {open ? "Close pull request" : "Reopen pull request"}
            </Button>
          )}
        </span>
      </div>
    </div>
  );
}

function BaseSelect({ pull, base, head }: { pull: Pull; base: string; head: string }) {
  const { data: branches } = useQuery(branchesQuery(pull));
  const update = useUpdatePull(pull);
  return (
    <NativeSelect
      size="sm"
      aria-label="Base branch"
      value={base}
      disabled={update.isPending}
      onChange={(e) => update.mutate({ base: e.target.value })}
      className="[&_select]:bg-info/10 [&_select]:text-foreground [&_select]:h-6 [&_select]:rounded-sm [&_select]:font-mono [&_select]:text-xs"
    >
      {(branches ?? [{ name: base }])
        .filter((b) => b.name !== head)
        .map((b) => (
          <NativeSelectOption key={b.name} value={b.name}>
            {b.name}
          </NativeSelectOption>
        ))}
    </NativeSelect>
  );
}
