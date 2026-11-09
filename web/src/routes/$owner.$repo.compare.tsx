import { useState } from "react";
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useSuspenseQuery, useQueryClient } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  ArrowLeft02Icon,
  CheckmarkCircle02Icon,
  GitCompareIcon,
  Alert02Icon,
} from "@hugeicons/core-free-icons";
import { api } from "@/api/client";
import { CommitList } from "@/areas/pulls/commit-list";
import { describe, refreshPull, write, WriteError } from "@/areas/pulls/mutations";
import {
  assigneesQuery,
  branchesQuery,
  compareQuery,
  pullsQuery,
  teamsQuery,
} from "@/areas/pulls/queries";
import { Panel } from "@/components/panel";
import { FileDiff, type DiffView } from "@/components/diff";
import { MarkdownBody } from "@/components/markdown";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";

type Search = { base?: string; head?: string; expand?: boolean };

export const Route = createFileRoute("/$owner/$repo/compare")({
  validateSearch: (s: Record<string, unknown>): Search => ({
    base: typeof s.base === "string" && s.base ? s.base : undefined,
    head: typeof s.head === "string" && s.head ? s.head : undefined,
    expand: s.expand === true || s.expand === "1" || s.expand === "true" ? true : undefined,
  }),
  loaderDeps: ({ search }) => ({ base: search.base, head: search.head }),
  loader: ({ context, params, deps }) => {
    void context.queryClient.prefetchQuery(branchesQuery(params));
    if (deps.head)
      void context.queryClient.prefetchQuery(
        compareQuery({ ...params, base: deps.base ?? "main", head: deps.head }),
      );
  },
  component: ComparePage,
});

function ComparePage() {
  const { head } = Route.useSearch();
  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="flex items-center gap-2 text-xl font-semibold">
          <HugeiconsIcon icon={GitCompareIcon} strokeWidth={2} className="size-5" />
          {head ? "Comparing changes" : "Compare changes"}
        </h1>
        <p className="text-muted-foreground mt-1 text-sm">
          Choose two branches to see what changed, or to start a pull request.
        </p>
      </div>
      <Region name="branches" fallback={null}>
        <Pickers />
      </Region>
      {head ? (
        <Region name="comparison" rows={8}>
          <Comparison />
        </Region>
      ) : (
        <Panel>
          <p className="text-muted-foreground py-6 text-center text-sm">
            Pick a head branch to compare it against the base.
          </p>
        </Panel>
      )}
    </div>
  );
}

function Pickers() {
  const params = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const { data: branches } = useSuspenseQuery(branchesQuery(params));
  const base = search.base ?? "main";
  const pick = (name: string, value: string | undefined) => (
    <label className="flex items-center gap-1.5 text-sm">
      <span className="text-muted-foreground">{name}:</span>
      <NativeSelect
        size="sm"
        aria-label={`${name} branch`}
        value={value ?? ""}
        onChange={(e) =>
          navigate({
            search: (prev) => ({
              ...prev,
              [name === "base" ? "base" : "head"]: e.target.value || undefined,
              expand: undefined,
            }),
          })
        }
        className="[&_select]:rounded-sm [&_select]:font-mono"
      >
        {name === "compare" && <NativeSelectOption value="">choose a branch</NativeSelectOption>}
        {branches.map((b) => (
          <NativeSelectOption key={b.name} value={b.name}>
            {b.name}
          </NativeSelectOption>
        ))}
      </NativeSelect>
    </label>
  );
  return (
    <div
      className="bg-muted/40 flex flex-wrap items-center gap-2 rounded-sm border px-3 py-2"
      data-testid="compare-pickers"
    >
      {pick("base", base)}
      <HugeiconsIcon
        icon={ArrowLeft02Icon}
        strokeWidth={2}
        className="text-muted-foreground size-4"
        aria-label="from"
      />
      {pick("compare", search.head)}
      {search.head && (
        <Button
          size="sm"
          variant="ghost"
          onClick={() => navigate({ search: { base: search.head, head: base } })}
        >
          Swap
        </Button>
      )}
    </div>
  );
}

function Comparison() {
  const params = Route.useParams();
  const search = Route.useSearch();
  const base = search.base ?? "main";
  const head = search.head!;
  const { data: cmp } = useSuspenseQuery(compareQuery({ ...params, base, head }));
  const { data: pulls } = useSuspenseQuery(pullsQuery(params));
  const [view, setView] = useState<DiffView>("unified");
  const existing = pulls.find(
    (p) => p.state === "open" && p.head.ref === head && p.base.ref === base,
  );
  const files = cmp.files ?? [];
  const additions = files.reduce((n, f) => n + f.additions, 0);
  const deletions = files.reduce((n, f) => n + f.deletions, 0);

  if (base === head)
    return (
      <Panel>
        <p className="py-6 text-center text-sm">
          Choose different branches. {base} is being compared with itself.
        </p>
      </Panel>
    );

  return (
    <div className="flex flex-col gap-4">
      <StatusLine
        status={cmp.status}
        ahead={cmp.ahead_by}
        behind={cmp.behind_by}
        base={base}
        head={head}
      />
      {existing ? (
        <Alert>
          <HugeiconsIcon icon={Alert02Icon} strokeWidth={2} />
          <AlertTitle>A pull request already exists for {head}</AlertTitle>
          <AlertDescription>
            <Link
              to="/$owner/$repo/pull/$number"
              params={{ ...params, number: existing.number }}
              className="font-medium hover:underline"
            >
              #{existing.number} {existing.title}
            </Link>
          </AlertDescription>
        </Alert>
      ) : cmp.ahead_by > 0 ? (
        search.expand ? (
          <CreateForm
            base={base}
            head={head}
            firstCommit={cmp.commits[0]?.commit.message ?? ""}
            single={cmp.commits.length === 1}
          />
        ) : (
          <div className="flex items-center gap-3 rounded-sm border px-3 py-2">
            <p className="text-sm">
              Discuss and review the changes in this comparison with others.
            </p>
            <Button
              size="sm"
              className="ml-auto"
              render={<Link from={Route.fullPath} search={(p) => ({ ...p, expand: true })} />}
            >
              Create pull request
            </Button>
          </div>
        )
      ) : null}
      <div
        className="text-muted-foreground flex flex-wrap items-center gap-4 border-y py-2 text-sm"
        data-testid="compare-summary"
      >
        <span>
          <strong className="text-foreground">{cmp.total_commits}</strong>{" "}
          {cmp.total_commits === 1 ? "commit" : "commits"}
        </span>
        <span>
          <strong className="text-foreground">{files.length}</strong>{" "}
          {files.length === 1 ? "file" : "files"} changed
        </span>
        <span className="font-mono">
          <span className="text-success">+{additions}</span>{" "}
          <span className="text-destructive">-{deletions}</span>
        </span>
      </div>
      {cmp.ahead_by > 0 && (
        <Tabs defaultValue="files">
          <div className="flex items-center gap-2">
            <TabsList variant="line">
              <TabsTrigger value="files">Files changed {files.length}</TabsTrigger>
              <TabsTrigger value="commits">Commits {cmp.commits.length}</TabsTrigger>
            </TabsList>
            <ToggleGroup
              className="ml-auto"
              size="sm"
              variant="outline"
              spacing={0}
              value={[view]}
              onValueChange={(v) => v[0] && setView(v[0] as DiffView)}
              aria-label="Diff view"
            >
              <ToggleGroupItem value="unified">Unified</ToggleGroupItem>
              <ToggleGroupItem value="split">Split</ToggleGroupItem>
            </ToggleGroup>
          </div>
          <TabsContent value="files" className="flex flex-col gap-3 pt-3">
            {files.map((f) => (
              <FileDiff key={f.filename} file={f} view={view} />
            ))}
          </TabsContent>
          <TabsContent value="commits" className="pt-3">
            <CommitList {...params} commits={cmp.commits} />
          </TabsContent>
        </Tabs>
      )}
    </div>
  );
}

function StatusLine({
  status,
  ahead,
  behind,
  base,
  head,
}: {
  status: string;
  ahead: number;
  behind: number;
  base: string;
  head: string;
}) {
  const ok = status === "ahead" || status === "diverged";
  return (
    <p className="flex flex-wrap items-center gap-2 text-sm" data-status={status}>
      <HugeiconsIcon
        icon={ok ? CheckmarkCircle02Icon : Alert02Icon}
        strokeWidth={2}
        className={ok ? "text-success size-4" : "text-muted-foreground size-4"}
      />
      {status === "identical"
        ? `${base} and ${head} are identical.`
        : status === "behind"
          ? `There isn't anything to compare. ${head} is ${plural(behind, "commit")} behind ${base}.`
          : `${head} is ${plural(ahead, "commit")} ahead of ${base}${behind ? ` and ${behind} behind it` : ""}.`}
      {status === "diverged" && (
        <Badge variant="warning-light" size="sm">
          Diverged
        </Badge>
      )}
    </p>
  );
}

function CreateForm({
  base,
  head,
  firstCommit,
  single,
}: {
  base: string;
  head: string;
  firstCommit: string;
  single: boolean;
}) {
  const params = Route.useParams();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [title, ...rest] = firstCommit.split("\n");
  const [failure, setFailure] = useState<string>();
  const form = useForm({
    defaultValues: {
      title: single ? title : branchTitle(head),
      body: single ? rest.join("\n").trim() : "",
      draft: false,
      reviewers: [] as string[],
      teams: [] as string[],
    },
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      try {
        const pr = await write(
          api.POST("/repos/{owner}/{repo}/pulls", {
            params: { path: params },
            body: { title: value.title.trim(), body: value.body, head, base, draft: value.draft },
          }),
        );
        const p = { ...params, number: pr.number };
        if (value.reviewers.length || value.teams.length) {
          try {
            await write(
              api.POST("/repos/{owner}/{repo}/pulls/{pull_number}/requested_reviewers", {
                params: { path: { ...params, pull_number: pr.number } },
                body: { reviewers: value.reviewers, team_reviewers: value.teams },
              }),
            );
          } catch (e) {
            toast.error("Pull request opened, but requesting reviewers failed", {
              description: describe(e),
            });
          }
        }
        await refreshPull(qc, p);
        toast.success(`Opened #${pr.number}`);
        void navigate({ to: "/$owner/$repo/pull/$number", params: p });
      } catch (e) {
        if (e instanceof WriteError && e.status === 422) {
          // GHES names the field; show it in place. A duplicate PR names "head".
          for (const [field, msg] of Object.entries(e.fields)) {
            if (field === "title" || field === "body")
              formApi.setFieldMeta(field, (m) => ({
                ...m,
                errorMap: { ...m.errorMap, onServer: msg },
              }));
          }
          if (!e.fields.title && !e.fields.body) setFailure(e.message);
        } else {
          setFailure(describe(e));
          toast.error("Opening the pull request failed", { description: describe(e) });
        }
      }
    },
  });

  return (
    <Panel title="Open a pull request">
      <form
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          void form.handleSubmit();
        }}
        data-testid="create-pr"
      >
        <div className="grid gap-4 md:grid-cols-[minmax(0,1fr)_14rem]">
          <div className="flex flex-col gap-3">
            <form.Field
              name="title"
              validators={{ onSubmit: ({ value }) => (value.trim() ? undefined : "Add a title.") }}
            >
              {(field) => (
                <label className="flex flex-col gap-1 text-sm">
                  <span className="font-medium">Title</span>
                  <Input
                    name="title"
                    className="rounded-sm"
                    value={field.state.value}
                    aria-invalid={field.state.meta.errors.length > 0 || undefined}
                    onChange={(e) => field.handleChange(e.target.value)}
                    onBlur={field.handleBlur}
                  />
                  {field.state.meta.errors.length > 0 && (
                    <span className="text-destructive text-xs" role="alert">
                      {field.state.meta.errors.join(", ")}
                    </span>
                  )}
                </label>
              )}
            </form.Field>
            <form.Field name="body">
              {(field) => (
                <Tabs defaultValue="write" className="gap-1">
                  <div className="flex items-center justify-between">
                    <span className="text-sm font-medium">Description</span>
                    <TabsList variant="line">
                      <TabsTrigger value="write">Write</TabsTrigger>
                      <TabsTrigger value="preview">Preview</TabsTrigger>
                    </TabsList>
                  </div>
                  <TabsContent value="write">
                    <Textarea
                      aria-label="Description"
                      className="min-h-32 rounded-sm"
                      placeholder="Describe the change. 'Closes #4' links an issue."
                      value={field.state.value}
                      onChange={(e) => field.handleChange(e.target.value)}
                    />
                  </TabsContent>
                  <TabsContent value="preview" className="min-h-32 rounded-sm border px-3 py-1">
                    <MarkdownBody text={field.state.value || "_Nothing to preview._"} />
                  </TabsContent>
                </Tabs>
              )}
            </form.Field>
          </div>
          <Region name="reviewers" fallback={null}>
            <form.Subscribe selector={(s) => [s.values.reviewers, s.values.teams] as const}>
              {([reviewers, teams]) => (
                <ReviewerPicker
                  reviewers={reviewers}
                  teams={teams}
                  onChange={(field, list) => form.setFieldValue(field, list)}
                />
              )}
            </form.Subscribe>
          </Region>
        </div>
        {failure && (
          <Alert variant="destructive">
            <AlertTitle>Pull request not opened</AlertTitle>
            <AlertDescription>{failure}</AlertDescription>
          </Alert>
        )}
        <div className="flex items-center justify-end gap-3">
          <form.Field name="draft">
            {(field) => (
              <label className="flex items-center gap-2 text-sm">
                <Switch
                  checked={field.state.value}
                  onCheckedChange={(v) => field.handleChange(v)}
                  aria-label="Create as draft"
                />
                Create as draft
              </label>
            )}
          </form.Field>
          <form.Subscribe selector={(s) => [s.isSubmitting, s.values.draft] as const}>
            {([submitting, draft]) => (
              <Button type="submit" size="sm" disabled={submitting}>
                {submitting
                  ? "Opening"
                  : draft
                    ? "Create draft pull request"
                    : "Create pull request"}
              </Button>
            )}
          </form.Subscribe>
        </div>
      </form>
    </Panel>
  );
}

function ReviewerPicker({
  reviewers,
  teams: chosen,
  onChange,
}: {
  reviewers: string[];
  teams: string[];
  onChange: (field: "reviewers" | "teams", list: string[]) => void;
}) {
  const params = Route.useParams();
  const { data: people } = useSuspenseQuery(assigneesQuery(params));
  const { data: teams } = useSuspenseQuery(teamsQuery(params));
  const toggle = (list: string[], v: string, on: boolean) =>
    on ? [...list, v] : list.filter((x) => x !== v);
  return (
    <fieldset className="flex flex-col gap-2 text-sm" data-testid="create-reviewers">
      <legend className="mb-1 font-medium">Reviewers</legend>
      {people
        .filter((u) => u.login !== "mira")
        .map((u) => (
          <label key={u.login} className="flex items-center gap-2">
            <Checkbox
              checked={reviewers.includes(u.login)}
              onCheckedChange={(on) => onChange("reviewers", toggle(reviewers, u.login, on))}
            />
            {u.login}
          </label>
        ))}
      {teams.map((t) => (
        <label key={t.slug} className="flex items-center gap-2">
          <Checkbox
            checked={chosen.includes(t.slug)}
            onCheckedChange={(on) => onChange("teams", toggle(chosen, t.slug, on))}
          />
          graftyard/{t.slug}
        </label>
      ))}
    </fieldset>
  );
}

const plural = (n: number, noun: string) => `${n} ${noun}${n === 1 ? "" : "s"}`;

// "mira/idempotent-retry" reads as "Idempotent retry"; "release/0.4" keeps its prefix.
function branchTitle(head: string) {
  const tail = head.replace(/^[^/]+\//, "");
  const name = /[a-z]/i.test(tail) ? tail : head;
  return name.replace(/[-_]/g, " ").replace(/^./, (c) => c.toUpperCase());
}
