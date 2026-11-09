import { useState } from "react";
import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { Link, useNavigate } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { GitForkIcon } from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api } from "@/api/client";
import { repoQuery, userOrgsQuery, userQuery } from "@/areas/core/queries";
import type { SlotEntry } from "@/areas/slots";
import { Ago } from "@/components/github";
import { ButtonGroup } from "@/components/ui/button-group";
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
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Textarea } from "@/components/ui/textarea";
import { call, failed, refetch } from "./client";
import { NameAvailability } from "./create";
import { normalizeRepoName } from "./domain";
import { applyServerErrors, FieldMessage, type RepoParams } from "./kit";
import { forksQuery } from "./queries";

function ForkButton(params: RepoParams) {
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const [open, setOpen] = useState(false);
  return (
    <>
      <ButtonGroup>
        <Button variant="outline" size="sm" onClick={() => setOpen(true)} data-testid="fork-button">
          <HugeiconsIcon icon={GitForkIcon} strokeWidth={2} data-icon="inline-start" />
          Fork
        </Button>
        <Popover>
          <PopoverTrigger
            render={
              <Button
                variant="outline"
                size="sm"
                className="tabular-nums"
                aria-label={`${repo.forks_count} forks`}
                data-testid="fork-count"
              />
            }
          >
            {repo.forks_count}
          </PopoverTrigger>
          <PopoverContent align="end" className="w-80 rounded-sm p-0">
            <ForkList params={params} />
          </PopoverContent>
        </Popover>
      </ButtonGroup>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="sm:max-w-lg">
          {open && <ForkForm params={params} onDone={() => setOpen(false)} />}
        </DialogContent>
      </Dialog>
    </>
  );
}

// The newest forks, read when the count is opened.
function ForkList({ params }: { params: RepoParams }) {
  const { data, isPending, error } = useQuery(forksQuery(params));
  if (isPending) return <p className="text-muted-foreground px-3 py-2 text-xs">Loading forks…</p>;
  if (error)
    return (
      <p className="text-destructive px-3 py-2 text-xs" role="alert">
        Could not load forks: {error.message}
      </p>
    );
  if (data.length === 0)
    return <p className="text-muted-foreground px-3 py-2 text-xs">No forks yet.</p>;
  return (
    <ul className="max-h-72 divide-y overflow-auto" data-testid="fork-list">
      {data.map((f) => (
        <li key={f.id} className="flex items-center gap-2 px-3 py-2 text-sm">
          <HugeiconsIcon
            icon={GitForkIcon}
            strokeWidth={2}
            className="text-muted-foreground size-4"
          />
          <Link
            to="/$owner/$repo"
            params={{ owner: f.owner.login, repo: f.name }}
            className="min-w-0 flex-1 truncate font-medium hover:underline"
          >
            {f.full_name}
          </Link>
          {f.created_at && (
            <span className="text-muted-foreground text-xs">
              <Ago at={f.created_at} />
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}

function ForkForm({ params, onDone }: { params: RepoParams; onDone: () => void }) {
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const { data: me } = useSuspenseQuery(userQuery());
  const { data: orgs } = useSuspenseQuery(userOrgsQuery());
  const navigate = useNavigate();
  const [failure, setFailure] = useState<string>();
  const owners = [me.login, ...orgs.map((o) => o.login)].filter((o) => o !== repo.owner.login);
  const form = useForm({
    defaultValues: {
      owner: owners[0] ?? me.login,
      name: repo.name,
      description: repo.description ?? "",
      defaultBranchOnly: true,
    },
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      try {
        const fork = await call(
          api.POST("/repos/{owner}/{repo}/forks", {
            params: { path: params },
            body: {
              organization: value.owner === me.login ? undefined : value.owner,
              name: normalizeRepoName(value.name),
              default_branch_only: value.defaultBranchOnly,
            },
          }),
        );
        toast.success(`Forked to ${fork.full_name}`);
        onDone();
        await refetch(repoQuery(params).queryKey, forksQuery(params).queryKey, ["user", "repos"]);
        void navigate({
          to: "/$owner/$repo",
          params: { owner: fork.owner.login, repo: fork.name },
        });
      } catch (e) {
        setFailure(applyServerErrors(formApi, e, { name: "name", organization: "owner" }));
        failed("Fork not created")(e);
      }
    },
  });
  return (
    <form
      className="flex flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
      data-testid="fork-form"
    >
      <DialogHeader>
        <DialogTitle>Create a new fork</DialogTitle>
        <DialogDescription>
          A fork is a copy of {repo.full_name}. You can change it without affecting the original.
        </DialogDescription>
      </DialogHeader>
      <div className="flex flex-wrap items-start gap-2">
        <form.Field name="owner">
          {(field) => (
            <label className="flex flex-col gap-1 text-sm font-medium">
              Owner
              <NativeSelect
                aria-label="Fork owner"
                value={field.state.value}
                onChange={(e) => field.handleChange(e.target.value)}
              >
                {owners.map((o) => (
                  <NativeSelectOption key={o} value={o}>
                    {o}
                  </NativeSelectOption>
                ))}
              </NativeSelect>
              <FieldMessage field={field} />
            </label>
          )}
        </form.Field>
        <span className="text-muted-foreground pt-7">/</span>
        <form.Field
          name="name"
          validators={{
            onChange: ({ value }) => (!value.trim() ? "A name is required." : undefined),
          }}
        >
          {(field) => (
            <div className="flex min-w-48 flex-1 flex-col gap-1">
              <label htmlFor="fork-name" className="text-sm font-medium">
                Repository name
              </label>
              <Input
                id="fork-name"
                value={field.state.value}
                onChange={(e) => field.handleChange(e.target.value)}
                aria-invalid={field.state.meta.errors.length > 0 || undefined}
              />
              <form.Subscribe selector={(s) => s.values.owner}>
                {(owner) => <NameAvailability owner={owner} name={field.state.value} />}
              </form.Subscribe>
              <FieldMessage field={field} />
            </div>
          )}
        </form.Field>
      </div>
      <form.Field name="description">
        {(field) => (
          <label className="flex flex-col gap-1 text-sm font-medium">
            Description{" "}
            <span className="text-muted-foreground text-xs font-normal">(optional)</span>
            <Textarea
              rows={2}
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
            />
          </label>
        )}
      </form.Field>
      <form.Field name="defaultBranchOnly">
        {(field) => (
          <label className="flex items-start gap-2 text-sm">
            <Checkbox
              className="mt-0.5"
              checked={field.state.value}
              onCheckedChange={(v) => field.handleChange(Boolean(v))}
            />
            <span>
              <span className="font-medium">Copy the {repo.default_branch} branch only</span>
              <span className="text-muted-foreground block text-xs">
                Contribute back to {repo.full_name} by adding your own branch.
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
      <DialogFooter>
        <Button type="button" variant="outline" onClick={onDone}>
          Cancel
        </Button>
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <Button type="submit" disabled={submitting}>
              {submitting ? "Forking…" : "Create fork"}
            </Button>
          )}
        </form.Subscribe>
      </DialogFooter>
    </form>
  );
}

// A typed literal, not slot(): src/areas/slots.tsx globs this module eagerly while it is
// still evaluating, so calling its slot() at load time throws.
export const slots: SlotEntry[] = [
  { slot: "repo-actions", region: "fork", order: 20, Component: ForkButton },
];
