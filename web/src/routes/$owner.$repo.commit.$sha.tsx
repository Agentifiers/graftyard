import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useForm } from "@tanstack/react-form";
import { useMutation, useSuspenseQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Add01Icon,
  CancelCircleIcon,
  CheckmarkCircle02Icon,
  Clock01Icon,
  CodeIcon,
  Loading03Icon,
  MinusSignCircleIcon,
  SecurityCheckIcon,
} from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { queryClient } from "@/api/query-client";
import { createStatus, errorText, WriteError, type FieldErrors } from "@/areas/code/mutations";
import { combinedStatusQuery, commitQuery } from "@/areas/code/queries";
import { CopyButton, short } from "@/areas/code/refs";
import { checkRunsQuery } from "@/areas/pulls/queries";
import { FileDiff } from "@/components/diff";
import { Ago, UserAvatar } from "@/components/github";
import { Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Skeleton } from "@/components/ui/skeleton";

// One commit: its message, author, parents, statuses, check runs, and file diffs.

export const Route = createFileRoute("/$owner/$repo/commit/$sha")({
  loader: ({ context, params }) => {
    const { owner, repo, sha } = params;
    void context.queryClient.prefetchQuery(commitQuery({ owner, repo, ref: sha }));
    void context.queryClient.prefetchQuery(combinedStatusQuery({ owner, repo, ref: sha }));
    void context.queryClient.prefetchQuery(checkRunsQuery({ owner, repo, ref: sha }));
  },
  component: CommitPage,
});

function CommitPage() {
  return (
    <div className="flex flex-col gap-4">
      <Region name="this commit" fallback={<Skeleton className="h-28" />}>
        <CommitHeader />
      </Region>
      <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_20rem]">
        <Region name="files" rows={10}>
          <Files />
        </Region>
        <div className="flex flex-col gap-4 lg:sticky lg:top-4">
          <Region name="statuses" rows={3}>
            <Statuses />
          </Region>
          <Region name="checks" rows={3}>
            <Checks />
          </Region>
        </div>
      </div>
    </div>
  );
}

function CommitHeader() {
  const { owner, repo, sha } = Route.useParams();
  const { data: c } = useSuspenseQuery(commitQuery({ owner, repo, ref: sha }));
  const [title, ...rest] = c.commit.message.split("\n");
  const body = rest.join("\n").trim();
  const author =
    c.author && "login" in c.author
      ? { login: c.author.login, avatar_url: c.author.avatar_url }
      : null;
  return (
    <Panel flush>
      <div className="flex flex-col gap-2 px-4 py-3">
        <div className="flex flex-wrap items-start gap-2">
          <h1 className="min-w-0 flex-1 text-lg font-semibold" data-testid="commit-title">
            {title}
          </h1>
          <Button
            size="sm"
            variant="outline"
            render={<Link to="/$owner/$repo/tree/$" params={{ owner, repo, _splat: c.sha }} />}
          >
            <HugeiconsIcon icon={CodeIcon} strokeWidth={2} data-icon="inline-start" />
            Browse files
          </Button>
        </div>
        {body && <pre className="text-muted-foreground text-sm whitespace-pre-wrap">{body}</pre>}
      </div>
      <div className="bg-muted/40 flex flex-wrap items-center gap-x-4 gap-y-1 border-t px-4 py-2 text-xs">
        <span className="flex items-center gap-1.5">
          <UserAvatar user={author} />
          <strong>{author?.login ?? c.commit.author?.name}</strong>
          <span className="text-muted-foreground">
            committed <Ago at={c.commit.author?.date ?? ""} />
          </span>
        </span>
        {c.commit.verification?.verified ? (
          <Badge variant="success-light" size="sm">
            <HugeiconsIcon icon={SecurityCheckIcon} strokeWidth={2} />
            Verified
          </Badge>
        ) : (
          <Badge variant="outline" size="sm">
            Unsigned
          </Badge>
        )}
        <span
          className="text-muted-foreground ml-auto flex flex-wrap items-center gap-2"
          data-testid="parents"
        >
          {c.parents.length === 0
            ? "No parents"
            : c.parents.length === 1
              ? "1 parent"
              : `${c.parents.length} parents`}
          {c.parents.map((p) => (
            <Link
              key={p.sha}
              to="/$owner/$repo/commit/$sha"
              params={{ owner, repo, sha: p.sha }}
              className="text-foreground font-mono hover:underline"
            >
              {short(p.sha)}
            </Link>
          ))}
          <span>commit</span>
          <code className="text-foreground font-mono">{c.sha}</code>
          <CopyButton value={c.sha} label="Copy the full SHA" />
        </span>
      </div>
    </Panel>
  );
}

function Files() {
  const { owner, repo, sha } = Route.useParams();
  const { data: c } = useSuspenseQuery(commitQuery({ owner, repo, ref: sha }));
  const files = c.files ?? [];
  return (
    <section className="flex flex-col gap-3" data-testid="commit-files">
      <p className="text-sm">
        Showing{" "}
        <strong>
          {files.length} changed {files.length === 1 ? "file" : "files"}
        </strong>{" "}
        with <span className="text-success font-semibold">{c.stats?.additions ?? 0} additions</span>{" "}
        and{" "}
        <span className="text-destructive font-semibold">{c.stats?.deletions ?? 0} deletions</span>.
      </p>
      {files.length === 0 && (
        <p className="text-muted-foreground border px-3 py-6 text-center text-sm">
          This commit changes no files.
        </p>
      )}
      {files.map((f) => (
        <div key={f.filename} className="flex flex-col gap-1">
          {f.previous_filename && (
            <p className="text-muted-foreground text-xs">
              Renamed from <code className="font-mono">{f.previous_filename}</code>
            </p>
          )}
          <FileDiff file={f} />
          {f.status !== "removed" && (
            <Link
              to="/$owner/$repo/blob/$"
              params={{ owner, repo, _splat: `${c.sha}/${f.filename}` }}
              className="text-primary self-end text-xs hover:underline"
            >
              View {f.filename.split("/").at(-1)} at {short(c.sha)}
            </Link>
          )}
        </div>
      ))}
    </section>
  );
}

const STATE_LOOK: Record<string, { icon: typeof Clock01Icon; tone: string; label: string }> = {
  success: { icon: CheckmarkCircle02Icon, tone: "text-success", label: "Success" },
  failure: { icon: CancelCircleIcon, tone: "text-destructive", label: "Failure" },
  error: { icon: CancelCircleIcon, tone: "text-destructive", label: "Error" },
  pending: { icon: Clock01Icon, tone: "text-warning", label: "Pending" },
};

function Statuses() {
  const { owner, repo, sha } = Route.useParams();
  const { data } = useSuspenseQuery(combinedStatusQuery({ owner, repo, ref: sha }));
  const [adding, setAdding] = useState(false);
  const overall = STATE_LOOK[data.state] ?? STATE_LOOK.pending;
  return (
    <Panel
      title={
        <span className="flex items-center gap-1.5">
          <HugeiconsIcon icon={overall.icon} strokeWidth={2} className={`size-4 ${overall.tone}`} />
          Statuses
        </span>
      }
      actions={
        <Button size="xs" variant="ghost" onClick={() => setAdding(!adding)} aria-expanded={adding}>
          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
          Add
        </Button>
      }
    >
      <div className="flex flex-col gap-2" data-testid="statuses" data-state={data.state}>
        {data.total_count === 0 ? (
          <p className="text-muted-foreground text-xs">No statuses reported for this commit.</p>
        ) : (
          <>
            <p className="text-muted-foreground text-xs">
              Combined state: <strong className="text-foreground capitalize">{data.state}</strong>{" "}
              from {data.total_count} {data.total_count === 1 ? "context" : "contexts"}
            </p>
            <ul className="flex flex-col gap-1.5">
              {data.statuses.map((s) => {
                const look = STATE_LOOK[s.state] ?? STATE_LOOK.pending;
                return (
                  <li key={s.id} className="flex items-start gap-2 text-sm">
                    <HugeiconsIcon
                      icon={look.icon}
                      strokeWidth={2}
                      className={`mt-0.5 size-4 shrink-0 ${look.tone}`}
                      aria-label={look.label}
                    />
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-1.5">
                        <span className="truncate font-medium">{s.context}</span>
                        {s.required && (
                          <Badge variant="outline" size="xs">
                            Required
                          </Badge>
                        )}
                      </div>
                      <p className="text-muted-foreground text-xs">
                        {s.description} · <Ago at={s.updated_at} />
                      </p>
                    </div>
                  </li>
                );
              })}
            </ul>
          </>
        )}
        {adding && <AddStatus onDone={() => setAdding(false)} />}
      </div>
    </Panel>
  );
}

type StatusState = "error" | "failure" | "pending" | "success";

function AddStatus({ onDone }: { onDone: () => void }) {
  const { owner, repo, sha } = Route.useParams();
  const [serverErrors, setServerErrors] = useState<FieldErrors>({});
  const key = combinedStatusQuery({ owner, repo, ref: sha }).queryKey;
  const post = useMutation({
    mutationFn: (v: {
      state: StatusState;
      context: string;
      description: string;
      target_url: string;
    }) => createStatus({ owner, repo }, { sha, ...v, target_url: v.target_url || undefined }),
    onMutate: async (v) => {
      // The new context shows at once as the latest for its name; a failure removes it.
      await queryClient.cancelQueries({ queryKey: key });
      const previous = queryClient.getQueryData<Schemas["combined-commit-status"]>(key);
      if (previous) {
        const now = new Date().toISOString();
        const draft: Schemas["simple-commit-status"] = {
          id: -1,
          node_id: "",
          url: "",
          avatar_url: null,
          created_at: now,
          updated_at: now,
          ...v,
          target_url: v.target_url || null,
        };
        const rest = previous.statuses.filter((s) => s.context !== v.context);
        queryClient.setQueryData(key, {
          ...previous,
          statuses: [draft, ...rest],
          total_count: rest.length + 1,
        });
      }
      return { previous };
    },
    onError: (e, _v, ctx) => {
      if (ctx?.previous) queryClient.setQueryData(key, ctx.previous);
      setServerErrors(e instanceof WriteError ? e.fields : {});
      toast.error(`Could not add the status: ${errorText(e)}`);
    },
    onSuccess: (s) => {
      toast.success(`Reported ${s.context} as ${s.state}`);
      onDone();
    },
    onSettled: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: key }),
        queryClient.invalidateQueries({
          queryKey: ["_t", "repos", owner, repo, "branch-summaries"],
        }),
      ]),
  });
  const form = useForm({
    defaultValues: {
      state: "success" as StatusState,
      context: "",
      description: "",
      target_url: "",
    },
    onSubmit: ({ value }) => {
      setServerErrors({});
      return post.mutateAsync({ ...value, context: value.context.trim() }).catch(() => {});
    },
  });
  return (
    <form
      className="flex flex-col gap-2 border-t pt-2"
      data-testid="add-status"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field
        name="context"
        validators={{
          onSubmit: ({ value }) =>
            value.trim() ? undefined : "Name the context, such as ci/lint.",
        }}
      >
        {(field) => (
          <Field>
            <FieldLabel htmlFor="status-context">Context</FieldLabel>
            <Input
              id="status-context"
              className="font-mono text-xs"
              placeholder="ci/lint"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
              aria-invalid={field.state.meta.errors.length > 0 || undefined}
            />
            <FieldError errors={field.state.meta.errors.map((m) => ({ message: String(m) }))} />
          </Field>
        )}
      </form.Field>
      <form.Field name="state">
        {(field) => (
          <Field>
            <FieldLabel htmlFor="status-state">State</FieldLabel>
            <NativeSelect
              id="status-state"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value as StatusState)}
              aria-invalid={Boolean(serverErrors.state) || undefined}
            >
              <NativeSelectOption value="success">Success</NativeSelectOption>
              <NativeSelectOption value="pending">Pending</NativeSelectOption>
              <NativeSelectOption value="failure">Failure</NativeSelectOption>
              <NativeSelectOption value="error">Error</NativeSelectOption>
            </NativeSelect>
            <FieldError errors={serverErrors.state ? [{ message: serverErrors.state }] : []} />
          </Field>
        )}
      </form.Field>
      <form.Field name="description">
        {(field) => (
          <Input
            aria-label="Description"
            placeholder="Description (optional)"
            value={field.state.value}
            onChange={(e) => field.handleChange(e.target.value)}
          />
        )}
      </form.Field>
      <form.Field name="target_url">
        {(field) => (
          <Field>
            <Input
              aria-label="Target URL"
              className="font-mono text-xs"
              placeholder="Target URL (optional)"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
              aria-invalid={Boolean(serverErrors.target_url) || undefined}
            />
            <FieldError
              errors={serverErrors.target_url ? [{ message: serverErrors.target_url }] : []}
            />
          </Field>
        )}
      </form.Field>
      {serverErrors.sha && (
        <p className="text-destructive text-xs">This commit is not in the repository.</p>
      )}
      <form.Subscribe selector={(s) => s.isSubmitting}>
        {(submitting) => (
          <Button type="submit" size="sm" disabled={submitting}>
            {submitting ? "Reporting" : "Report status"}
          </Button>
        )}
      </form.Subscribe>
    </form>
  );
}

function Checks() {
  const { owner, repo, sha } = Route.useParams();
  const { data } = useSuspenseQuery(checkRunsQuery({ owner, repo, ref: sha }));
  return (
    <Panel title="Check runs">
      {data.total_count === 0 ? (
        <p className="text-muted-foreground text-xs" data-testid="checks">
          No check runs for this commit.
        </p>
      ) : (
        <ul className="flex flex-col gap-1.5" data-testid="checks">
          {data.check_runs.map((run) => {
            const look =
              run.status === "in_progress"
                ? { icon: Loading03Icon, tone: "text-warning animate-spin" }
                : run.status !== "completed"
                  ? { icon: Clock01Icon, tone: "text-muted-foreground" }
                  : run.conclusion === "success"
                    ? { icon: CheckmarkCircle02Icon, tone: "text-success" }
                    : run.conclusion === "failure"
                      ? { icon: CancelCircleIcon, tone: "text-destructive" }
                      : { icon: MinusSignCircleIcon, tone: "text-muted-foreground" };
            return (
              <li key={run.id} className="flex items-start gap-2 text-sm">
                <HugeiconsIcon
                  icon={look.icon}
                  strokeWidth={2}
                  className={`mt-0.5 size-4 shrink-0 ${look.tone}`}
                />
                <div className="min-w-0">
                  <div className="truncate font-medium">{run.name}</div>
                  <p className="text-muted-foreground text-xs">
                    {run.output.summary ?? run.status}
                  </p>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </Panel>
  );
}
