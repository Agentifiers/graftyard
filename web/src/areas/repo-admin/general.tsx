import { useState } from "react";
import { useMutation, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { useNavigate } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { Cancel01Icon } from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api, type Schemas } from "@/api/client";
import { repoQuery, userOrgsQuery, userQuery } from "@/areas/core/queries";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { call, failed, optimistic, refetch } from "./client";
import { MERGE_PRESETS, normalizeRepoName, presetOf, SQUASH_PRESETS, TOPIC_RE } from "./domain";
import {
  applyServerErrors,
  FieldMessage,
  RepoSwitch,
  SettingRow,
  TypedConfirm,
  useRepoPatch,
  type RepoParams,
} from "./kit";
import { ui } from "./private-api";
import { adminSettingsQuery, branchesQuery, topicsQuery } from "./queries";

type FullRepo = Schemas["full-repository"];

// ---- Name ----

export function RenameForm({ params }: { params: RepoParams }) {
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const navigate = useNavigate();
  const [failure, setFailure] = useState<string>();
  const form = useForm({
    defaultValues: { name: repo.name },
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const name = normalizeRepoName(value.name);
      try {
        const updated = await call(
          api.PATCH("/repos/{owner}/{repo}", { params: { path: params }, body: { name } }),
        );
        toast.success(`Renamed to ${updated.full_name}`);
        await refetch(["user", "repos"]);
        void navigate({
          to: "/$owner/$repo/settings/general",
          params: { owner: updated.owner.login, repo: updated.name },
          replace: true,
        });
      } catch (e) {
        setFailure(applyServerErrors(formApi, e, { name: "name" }));
      }
    },
  });
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <label htmlFor="repo-name" className="text-sm font-medium">
        Repository name
      </label>
      <form.Field
        name="name"
        validators={{
          onChange: ({ value }) =>
            !value.trim()
              ? "Name is required."
              : value.length > 100
                ? "Name is at most 100 characters."
                : undefined,
        }}
      >
        {(field) => (
          <>
            <div className="flex max-w-md gap-2">
              <Input
                id="repo-name"
                name={field.name}
                value={field.state.value}
                disabled={repo.archived}
                onBlur={field.handleBlur}
                onChange={(e) => field.handleChange(e.target.value)}
                aria-invalid={field.state.meta.errors.length > 0 || undefined}
              />
              <form.Subscribe selector={(s) => [s.isSubmitting, s.values.name] as const}>
                {([submitting, name]) => (
                  <Button
                    type="submit"
                    variant="outline"
                    disabled={submitting || repo.archived || name === repo.name}
                  >
                    {submitting ? "Renaming…" : "Rename"}
                  </Button>
                )}
              </form.Subscribe>
            </div>
            {normalizeRepoName(field.state.value) !== field.state.value.trim() &&
              field.state.value.trim() && (
                <p className="text-muted-foreground text-xs">
                  Your repository will be renamed to{" "}
                  <strong>{normalizeRepoName(field.state.value)}</strong>.
                </p>
              )}
            <FieldMessage field={field} />
          </>
        )}
      </form.Field>
      {failure && (
        <p className="text-destructive text-xs" role="alert">
          {failure}
        </p>
      )}
    </form>
  );
}

// ---- About: description and website ----

export function AboutForm({ params }: { params: RepoParams }) {
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const [failure, setFailure] = useState<string>();
  const key = repoQuery(params).queryKey;
  const form = useForm({
    defaultValues: { description: repo.description ?? "", homepage: repo.homepage ?? "" },
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const rollback = await optimistic<FullRepo>(key, (old) => ({
        ...old,
        description: value.description || null,
        homepage: value.homepage || null,
      }));
      try {
        await call(
          api.PATCH("/repos/{owner}/{repo}", {
            params: { path: params },
            body: { description: value.description, homepage: value.homepage },
          }),
        );
        toast.success("Saved the description and website");
        formApi.reset(value);
      } catch (e) {
        rollback();
        setFailure(
          applyServerErrors(formApi, e, { homepage: "homepage", description: "description" }),
        );
        failed("Could not save")(e);
      } finally {
        void refetch(key);
      }
    },
  });
  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field
        name="description"
        validators={{
          onChange: ({ value }) =>
            value.length > 350 ? "A description is at most 350 characters." : undefined,
        }}
      >
        {(field) => (
          <div className="flex flex-col gap-1">
            <label htmlFor="repo-description" className="text-sm font-medium">
              Description
            </label>
            <Textarea
              id="repo-description"
              rows={2}
              value={field.state.value}
              disabled={repo.archived}
              onChange={(e) => field.handleChange(e.target.value)}
            />
            <FieldMessage field={field} />
          </div>
        )}
      </form.Field>
      <form.Field
        name="homepage"
        validators={{
          onChange: ({ value }) =>
            value && !/^https?:\/\//.test(value)
              ? "Start the URL with http:// or https://."
              : undefined,
        }}
      >
        {(field) => (
          <div className="flex flex-col gap-1">
            <label htmlFor="repo-homepage" className="text-sm font-medium">
              Website
            </label>
            <Input
              id="repo-homepage"
              placeholder="https://example.com"
              value={field.state.value}
              disabled={repo.archived}
              onChange={(e) => field.handleChange(e.target.value)}
              aria-invalid={field.state.meta.errors.length > 0 || undefined}
            />
            <FieldMessage field={field} />
          </div>
        )}
      </form.Field>
      {failure && (
        <p className="text-destructive text-xs" role="alert">
          {failure}
        </p>
      )}
      <form.Subscribe selector={(s) => [s.isSubmitting, s.isDirty] as const}>
        {([submitting, dirty]) => (
          <div>
            <Button type="submit" size="sm" disabled={submitting || !dirty || repo.archived}>
              {submitting ? "Saving…" : "Save"}
            </Button>
          </div>
        )}
      </form.Subscribe>
    </form>
  );
}

// ---- Topics ----

export function TopicsEditor({ params }: { params: RepoParams }) {
  const { data } = useSuspenseQuery(topicsQuery(params));
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string>();
  const key = topicsQuery(params).queryKey;
  const save = useMutation({
    mutationFn: (names: string[]) =>
      call(api.PUT("/repos/{owner}/{repo}/topics", { params: { path: params }, body: { names } })),
    onMutate: (names) => optimistic<{ names: string[] }>(key, () => ({ names })),
    onError: (e, _names, rollback) => {
      rollback?.();
      setError(e instanceof Error ? e.message : String(e));
      failed("Topics not saved")(e);
    },
    onSuccess: () => setError(undefined),
    onSettled: () => refetch(key, repoQuery(params).queryKey),
  });
  const add = () => {
    const names = draft
      .split(/[\s,]+/)
      .map((t) => t.trim().toLowerCase())
      .filter(Boolean);
    if (!names.length) return;
    const bad = names.find((n) => !TOPIC_RE.test(n));
    if (bad) {
      setError(`${bad}: topics use lowercase letters, numbers, and hyphens, up to 50 characters.`);
      return;
    }
    setDraft("");
    save.mutate([...new Set([...data.names, ...names])]);
  };
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap gap-1.5" data-testid="topics">
        {data.names.length === 0 && (
          <span className="text-muted-foreground text-xs">No topics yet.</span>
        )}
        {data.names.map((t) => (
          <Badge key={t} variant="info-light" size="lg" className="gap-1">
            {t}
            <button
              type="button"
              aria-label={`Remove topic ${t}`}
              disabled={repo.archived}
              className="hover:text-foreground"
              onClick={() => save.mutate(data.names.filter((n) => n !== t))}
            >
              <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} className="size-3" />
            </button>
          </Badge>
        ))}
      </div>
      <form
        className="flex max-w-md gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          add();
        }}
      >
        <Input
          aria-label="Add topics"
          placeholder="Add topics, separated by spaces"
          value={draft}
          disabled={repo.archived}
          onChange={(e) => setDraft(e.target.value)}
        />
        <Button type="submit" variant="outline" disabled={!draft.trim() || repo.archived}>
          Add
        </Button>
      </form>
      {error && (
        <p className="text-destructive text-xs" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

// ---- Features ----

export function Features({ params }: { params: RepoParams }) {
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const key = repoQuery(params).queryKey;
  const discussions = useMutation({
    mutationFn: (on: boolean) =>
      call(
        ui.PATCH("/repos/{owner}/{repo}/admin-settings", {
          params: { path: params },
          body: { has_discussions: on },
        }),
      ),
    onMutate: (on) => optimistic<FullRepo>(key, (old) => ({ ...old, has_discussions: on })),
    onError: (e, _on, rollback) => {
      rollback?.();
      failed("Could not change discussions")(e);
    },
    onSettled: () => refetch(key),
  });
  return (
    <div className="divide-y">
      <SettingRow
        title="Issues"
        description="Track bugs, tasks, and feature requests."
        control={<RepoSwitch params={params} field="has_issues" label="Issues" />}
      />
      <SettingRow
        title="Projects"
        description="Plan and track work in project boards linked to this repository."
        control={<RepoSwitch params={params} field="has_projects" label="Projects" />}
      />
      <SettingRow
        title="Wikis"
        description="Host documentation for the repository in a wiki."
        control={<RepoSwitch params={params} field="has_wiki" label="Wikis" />}
      />
      <SettingRow
        title="Discussions"
        description="A place for the community to ask questions and share ideas."
        control={
          <Switch
            aria-label="Discussions"
            data-setting="has_discussions"
            checked={repo.has_discussions}
            disabled={repo.archived}
            onCheckedChange={(on) => discussions.mutate(on)}
          />
        }
      />
      <SettingRow
        title="Template repository"
        description="Let people generate new repositories with the same structure and files."
        control={<RepoSwitch params={params} field="is_template" label="Template repository" />}
      />
    </div>
  );
}

// ---- Default branch ----

export function DefaultBranch({ params }: { params: RepoParams }) {
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const { data: branches } = useSuspenseQuery(branchesQuery(params));
  const [choice, setChoice] = useState(repo.default_branch);
  const patch = useRepoPatch(params, "Default branch not changed");
  if (branches.length === 0) {
    return (
      <p className="text-muted-foreground text-sm">
        This repository is empty. Its first push sets the default branch.
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-2">
      <p className="text-muted-foreground text-xs">
        The default branch is the base for pull requests and the branch clones check out.
      </p>
      <div className="flex max-w-md items-center gap-2">
        <NativeSelect
          aria-label="Default branch"
          value={choice}
          disabled={repo.archived}
          onChange={(e) => setChoice(e.target.value)}
          className="flex-1"
        >
          {branches.map((b) => (
            <NativeSelectOption key={b.name} value={b.name}>
              {b.name}
            </NativeSelectOption>
          ))}
        </NativeSelect>
        <Button
          variant="outline"
          disabled={choice === repo.default_branch || patch.isPending || repo.archived}
          onClick={() =>
            patch.mutate(
              { default_branch: choice },
              {
                onSuccess: () => toast.success(`Default branch is now ${choice}`),
                onError: () => setChoice(repo.default_branch),
              },
            )
          }
        >
          Update
        </Button>
      </div>
    </div>
  );
}

// ---- Pull requests ----

export function PullRequestSettings({ params }: { params: RepoParams }) {
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const patch = useRepoPatch(params, "Pull request setting not saved");
  const key = repoQuery(params).queryKey;
  const autoMerge = useMutation({
    mutationFn: (on: boolean) =>
      call(
        ui.PATCH("/repos/{owner}/{repo}/admin-settings", {
          params: { path: params },
          body: { allow_auto_merge: on },
        }),
      ),
    onMutate: (on) => optimistic<FullRepo>(key, (old) => ({ ...old, allow_auto_merge: on })),
    onError: (e, _on, rollback) => {
      rollback?.();
      failed("Could not change auto-merge")(e);
    },
    onSettled: () => refetch(key),
  });
  const squash = presetOf(
    SQUASH_PRESETS,
    repo.squash_merge_commit_title,
    repo.squash_merge_commit_message,
  );
  const merge = presetOf(MERGE_PRESETS, repo.merge_commit_title, repo.merge_commit_message);
  const methods = [
    repo.allow_merge_commit,
    repo.allow_squash_merge,
    repo.allow_rebase_merge,
  ].filter(Boolean).length;
  return (
    <div className="divide-y">
      <SettingRow
        title="Allow merge commits"
        description="Add all commits from the head branch to the base branch with a merge commit."
        control={
          <RepoSwitch
            params={params}
            field="allow_merge_commit"
            label="Allow merge commits"
            disabled={repo.allow_merge_commit && methods === 1}
          />
        }
      >
        {repo.allow_merge_commit && (
          <PresetSelect
            label="Default merge commit message"
            value={merge.id}
            options={MERGE_PRESETS}
            disabled={repo.archived}
            onChange={(p) =>
              patch.mutate({ merge_commit_title: p.title, merge_commit_message: p.message })
            }
          />
        )}
      </SettingRow>
      <SettingRow
        title="Allow squash merging"
        description="Combine all commits from the head branch into a single commit in the base branch."
        control={
          <RepoSwitch
            params={params}
            field="allow_squash_merge"
            label="Allow squash merging"
            disabled={repo.allow_squash_merge && methods === 1}
          />
        }
      >
        {repo.allow_squash_merge && (
          <PresetSelect
            label="Default commit message"
            value={squash.id}
            options={SQUASH_PRESETS}
            disabled={repo.archived}
            onChange={(p) =>
              patch.mutate({
                squash_merge_commit_title: p.title,
                squash_merge_commit_message: p.message,
                use_squash_pr_title_as_default: p.title === "PR_TITLE",
              })
            }
          />
        )}
      </SettingRow>
      <SettingRow
        title="Allow rebase merging"
        description="Add all commits from the head branch onto the base branch individually."
        control={
          <RepoSwitch
            params={params}
            field="allow_rebase_merge"
            label="Allow rebase merging"
            disabled={repo.allow_rebase_merge && methods === 1}
          />
        }
      />
      {methods === 1 && (
        <p className="text-muted-foreground px-4 py-2 text-xs">
          At least one merge method stays on, so the last one cannot be switched off.
        </p>
      )}
      <SettingRow
        title="Always suggest updating pull request branches"
        description="Offer to update a head branch that is behind its base."
        control={
          <RepoSwitch
            params={params}
            field="allow_update_branch"
            label="Suggest updating branches"
          />
        }
      />
      <SettingRow
        title="Allow auto-merge"
        description="Let people set a pull request to merge once its requirements are met."
        control={
          <Switch
            aria-label="Allow auto-merge"
            data-setting="allow_auto_merge"
            checked={repo.allow_auto_merge ?? false}
            disabled={repo.archived}
            onCheckedChange={(on) => autoMerge.mutate(on)}
          />
        }
      />
      <SettingRow
        title="Automatically delete head branches"
        description="Delete the head branch once its pull request merges."
        control={
          <RepoSwitch params={params} field="delete_branch_on_merge" label="Delete head branches" />
        }
      />
    </div>
  );
}

function PresetSelect<P extends { id: string; label: string }>({
  label,
  value,
  options,
  disabled,
  onChange,
}: {
  label: string;
  value: string;
  options: P[];
  disabled?: boolean;
  onChange: (p: P) => void;
}) {
  return (
    <label className="text-muted-foreground flex flex-wrap items-center gap-2 pl-4 text-xs">
      {label}
      <NativeSelect
        size="sm"
        aria-label={label}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(options.find((o) => o.id === e.target.value)!)}
      >
        {options.map((o) => (
          <NativeSelectOption key={o.id} value={o.id}>
            {o.label}
          </NativeSelectOption>
        ))}
      </NativeSelect>
    </label>
  );
}

// ---- Archives: Git LFS ----

export function LfsSetting({ params }: { params: RepoParams }) {
  const { data: settings } = useSuspenseQuery({
    ...adminSettingsQuery(params),
    // While enabling runs in the background, poll until it settles.
    refetchInterval: (q) => (q.state.data?.lfs === "enabling" ? 1000 : false),
  });
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const key = adminSettingsQuery(params).queryKey;
  const toggle = useMutation({
    mutationFn: async (on: boolean) => {
      const req = on
        ? api.PUT("/repos/{owner}/{repo}/lfs", { params: { path: params } })
        : api.DELETE("/repos/{owner}/{repo}/lfs", { params: { path: params } });
      return call(req);
    },
    onMutate: (on) =>
      optimistic<typeof settings>(key, (old) => ({ ...old, lfs: on ? "enabling" : "disabled" })),
    onError: (e, _on, rollback) => {
      rollback?.();
      failed("Git LFS not changed")(e);
    },
    onSettled: () => refetch(key),
  });
  const mb = (settings.lfs_bytes / 1_048_576).toFixed(1);
  return (
    <SettingRow
      title="Include Git LFS objects in archives"
      description={
        settings.lfs === "enabling"
          ? "Turning on: moving stored objects into archives. This takes a few seconds."
          : settings.lfs === "enabled"
            ? `Source code archives include ${mb} MB of LFS objects.`
            : "Source code archives contain LFS pointer files only."
      }
      control={
        <div className="flex items-center gap-2">
          {settings.lfs === "enabling" && (
            <Badge variant="warning-light" data-testid="lfs-state">
              Enabling
            </Badge>
          )}
          <Switch
            aria-label="Include Git LFS objects in archives"
            checked={settings.lfs !== "disabled"}
            disabled={settings.lfs === "enabling" || repo.archived}
            onCheckedChange={(on) => toggle.mutate(on)}
          />
        </div>
      }
    />
  );
}

// ---- Danger zone ----

const VISIBILITIES = ["public", "internal", "private"] as const;
type Visibility = (typeof VISIBILITIES)[number];

export function DangerZone({ params }: { params: RepoParams }) {
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const [open, setOpen] = useState<"visibility" | "archive" | "transfer" | "delete" | null>(null);
  const full = `${repo.owner.login}/${repo.name}`;
  const visibility = (repo.visibility ?? (repo.private ? "private" : "public")) as Visibility;
  const rows: {
    id: NonNullable<typeof open>;
    title: string;
    description: string;
    action: string;
  }[] = [
    {
      id: "visibility",
      title: "Change repository visibility",
      description: `This repository is ${visibility}.`,
      action: "Change visibility",
    },
    {
      id: "archive",
      title: repo.archived ? "Unarchive this repository" : "Archive this repository",
      description: repo.archived
        ? "Make this repository writable again."
        : "Mark this repository archived and read-only.",
      action: repo.archived ? "Unarchive" : "Archive this repository",
    },
    {
      id: "transfer",
      title: "Transfer ownership",
      description: "Transfer this repository to another user or to an organization.",
      action: "Transfer",
    },
    {
      id: "delete",
      title: "Delete this repository",
      description: "Once you delete a repository, there is no going back.",
      action: "Delete this repository",
    },
  ];
  return (
    <div
      className="border-destructive/50 divide-destructive/30 divide-y rounded-sm border"
      data-testid="danger-zone"
    >
      {rows.map((r) => (
        <SettingRow
          key={r.id}
          title={r.title}
          description={r.description}
          control={
            <Button
              variant="destructive"
              size="sm"
              disabled={repo.archived && r.id !== "archive" && r.id !== "delete"}
              onClick={() => setOpen(r.id)}
            >
              {r.action}
            </Button>
          }
        />
      ))}
      <VisibilityDialog
        params={params}
        open={open === "visibility"}
        onOpenChange={(o) => setOpen(o ? "visibility" : null)}
        current={visibility}
        phrase={full}
        ownerType={repo.owner.type}
      />
      <ArchiveDialog
        params={params}
        open={open === "archive"}
        onOpenChange={(o) => setOpen(o ? "archive" : null)}
        archived={repo.archived}
        phrase={full}
      />
      <TransferDialog
        params={params}
        open={open === "transfer"}
        onOpenChange={(o) => setOpen(o ? "transfer" : null)}
        phrase={full}
      />
      <DeleteDialog
        params={params}
        open={open === "delete"}
        onOpenChange={(o) => setOpen(o ? "delete" : null)}
        phrase={full}
      />
    </div>
  );
}

type DialogProps = {
  params: RepoParams;
  open: boolean;
  onOpenChange: (o: boolean) => void;
  phrase: string;
};

function VisibilityDialog({
  current,
  ownerType,
  ...p
}: DialogProps & { current: Visibility; ownerType: string }) {
  const [choice, setChoice] = useState<Visibility>(current === "public" ? "private" : "public");
  const patch = useRepoPatch(p.params, "Visibility not changed");
  const options = VISIBILITIES.filter(
    (v) => v !== current && (v !== "internal" || ownerType === "Organization"),
  );
  return (
    <TypedConfirm
      {...p}
      title="Change repository visibility"
      action={`Make this repository ${choice}`}
      pending={patch.isPending}
      description={
        choice === "public"
          ? "Anyone on this instance, including signed-out users, will be able to see the code."
          : choice === "internal"
            ? "Every member of the enterprise will be able to see the code."
            : "Only people and teams you give access will see the code. Stars and watchers from people without access are removed."
      }
      onConfirm={() =>
        patch.mutate(
          { visibility: choice },
          {
            onSuccess: () => {
              toast.success(`The repository is now ${choice}`);
              p.onOpenChange(false);
            },
          },
        )
      }
    >
      <div className="flex flex-col gap-1.5" role="radiogroup" aria-label="New visibility">
        {options.map((v) => (
          <label
            key={v}
            className="hover:bg-muted flex items-center gap-2 rounded-sm px-2 py-1 text-sm capitalize"
          >
            <input
              type="radio"
              name="visibility"
              value={v}
              checked={choice === v}
              onChange={() => setChoice(v)}
            />
            {v}
          </label>
        ))}
      </div>
    </TypedConfirm>
  );
}

function ArchiveDialog({ archived, ...p }: DialogProps & { archived: boolean }) {
  const patch = useRepoPatch(p.params, archived ? "Not unarchived" : "Not archived");
  return (
    <TypedConfirm
      {...p}
      title={archived ? "Unarchive this repository" : "Archive this repository"}
      action={
        archived
          ? "I understand, unarchive this repository"
          : "I understand, archive this repository"
      }
      pending={patch.isPending}
      description={
        archived
          ? "People with access will be able to push, open issues, and change settings again."
          : "The repository becomes read-only: no pushes, issues, pull requests, comments, or setting changes. You can unarchive it later."
      }
      onConfirm={() =>
        patch.mutate(
          { archived: !archived },
          {
            onSuccess: () => {
              toast.success(archived ? "Unarchived" : "Archived");
              p.onOpenChange(false);
            },
          },
        )
      }
    />
  );
}

function TransferDialog(p: DialogProps) {
  const { data: me } = useSuspenseQuery(userQuery());
  const { data: orgs } = useSuspenseQuery(userOrgsQuery());
  const { data: repo } = useSuspenseQuery(repoQuery(p.params));
  const navigate = useNavigate();
  const owners = [me.login, ...orgs.map((o) => o.login)].filter((l) => l !== repo.owner.login);
  const [owner, setOwner] = useState(owners[0] ?? "");
  const [name, setName] = useState(repo.name);
  const [error, setError] = useState<string>();
  const transfer = useMutation({
    mutationFn: () =>
      call(
        api.POST("/repos/{owner}/{repo}/transfer", {
          params: { path: p.params },
          body: { new_owner: owner, new_name: name === repo.name ? undefined : name },
        }),
      ),
    onError: (e) => {
      setError(e instanceof Error ? e.message : String(e));
      failed("Transfer refused")(e);
    },
    onSuccess: async (moved) => {
      toast.success(`Transferred to ${moved.full_name}`);
      p.onOpenChange(false);
      await refetch(["user", "repos"]);
      void navigate({
        to: "/$owner/$repo",
        params: { owner: moved.owner.login, repo: moved.name },
      });
    },
  });
  return (
    <TypedConfirm
      {...p}
      title="Transfer repository"
      action="I understand, transfer this repository"
      pending={transfer.isPending}
      description="Issues, pull requests, stars, and settings move with it. Links to the old address redirect."
      onConfirm={() => {
        setError(undefined);
        transfer.mutate();
      }}
    >
      <div className="grid gap-2 text-sm">
        <label className="flex flex-col gap-1">
          New owner
          <NativeSelect
            aria-label="New owner"
            value={owner}
            onChange={(e) => setOwner(e.target.value)}
            className="w-full"
          >
            {owners.map((o) => (
              <NativeSelectOption key={o} value={o}>
                {o}
              </NativeSelectOption>
            ))}
          </NativeSelect>
        </label>
        <label className="flex flex-col gap-1">
          Repository name
          <Input
            aria-label="New repository name"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </label>
        {error && (
          <p className="text-destructive text-xs" role="alert">
            {error}
          </p>
        )}
      </div>
    </TypedConfirm>
  );
}

function DeleteDialog(p: DialogProps) {
  const navigate = useNavigate();
  const del = useMutation({
    mutationFn: () => call(api.DELETE("/repos/{owner}/{repo}", { params: { path: p.params } })),
    onError: failed("Repository not deleted"),
    onSuccess: async () => {
      toast.success(`Deleted ${p.phrase}`);
      p.onOpenChange(false);
      await refetch(["user", "repos"]);
      void navigate({ to: "/" });
    },
  });
  return (
    <TypedConfirm
      {...p}
      title={`Delete ${p.phrase}`}
      action="Delete this repository"
      pending={del.isPending}
      description="This deletes the repository, its wiki, issues, pull requests, webhooks, and deploy keys, and removes every collaborator's access. It cannot be undone."
      onConfirm={() => del.mutate()}
    />
  );
}
