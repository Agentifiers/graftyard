import { useState } from "react";
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import type { Schemas } from "@/api/client";
import { userQuery } from "@/areas/core/queries";
import { errorText, toastFailure, ValidationError } from "@/areas/issues/api";
import { createdNumberFor, draftIssue, issuesCollection } from "@/areas/issues/collections";
import { MarkdownEditor } from "@/areas/issues/markdown-editor";
import { AssigneePicker, formatDue, LabelPicker, MilestonePicker } from "@/areas/issues/pickers";
import { assigneesQuery, labelsQuery, milestonesQuery } from "@/areas/issues/queries";
import { LabelBadge, UserAvatar } from "@/components/github";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";

export const Route = createFileRoute("/$owner/$repo/issues/new")({
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(labelsQuery(params));
    void context.queryClient.prefetchQuery(milestonesQuery(params));
    void context.queryClient.prefetchQuery(assigneesQuery(params));
    void issuesCollection(params)
      .preload()
      .catch(() => {});
  },
  component: NewIssuePage,
});

function NewIssuePage() {
  return (
    <div className="flex flex-col">
      <PageHeader
        title="Create new issue"
        description="Describe the bug or the change. Markdown is supported."
      />
      <Region name="new issue" rows={6}>
        <NewIssueForm />
      </Region>
    </div>
  );
}

type Values = {
  title: string;
  body: string;
  labels: Schemas["label"][];
  assignees: Schemas["simple-user"][];
  milestone: Schemas["milestone"] | null;
};

// The new issue shows in the issues collection at once, and the page moves to it once the
// server answers with its number. A failed create removes it and keeps the form filled.
function NewIssueForm() {
  const params = Route.useParams();
  const navigate = useNavigate();
  const { data: viewer } = useSuspenseQuery(userQuery());
  const [failure, setFailure] = useState<string>();
  const form = useForm({
    defaultValues: { title: "", body: "", labels: [], assignees: [], milestone: null } as Values,
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const draft = draftIssue(viewer, { ...value, title: value.title.trim() });
      const tx = issuesCollection(params).insert(draft);
      try {
        await tx.when("settled");
        const number = createdNumberFor(draft.number);
        if (number)
          await navigate({ to: "/$owner/$repo/issues/$number", params: { ...params, number } });
      } catch (e) {
        if (e instanceof ValidationError) {
          for (const [field, message] of Object.entries(e.fields)) {
            if (field === "title" || field === "body" || field === "milestone") {
              formApi.setFieldMeta(field, (m) => ({ ...m, errorMap: { onServer: message } }));
            }
          }
        }
        setFailure(errorText(e));
        toastFailure("Creating the issue", e);
      }
    },
  });
  const repo = { owner: params.owner, repo: params.repo };
  return (
    <form
      className="grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_16rem]"
      noValidate
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <div className="bg-card flex flex-col gap-3 rounded-sm border p-4">
        <form.Field
          name="title"
          validators={{
            // A blank title is left to the server so its 422 shows here; only length is checked locally.
            onChange: ({ value }) =>
              value.length > 256 ? "Keep the title under 256 characters." : undefined,
          }}
        >
          {(field) => (
            <label className="flex flex-col gap-1 text-sm">
              <span className="font-semibold">
                Add a title <span className="text-destructive">*</span>
              </span>
              <Input
                name="title"
                aria-label="Title"
                autoFocus
                placeholder="Title"
                className="rounded-sm"
                value={field.state.value}
                aria-invalid={field.state.meta.errors.length > 0 || undefined}
                onBlur={field.handleBlur}
                onChange={(e) => field.handleChange(e.target.value)}
              />
              {field.state.meta.errors.length > 0 && (
                <span className="text-destructive text-xs" data-field-error="title">
                  Title {String(field.state.meta.errors.join(", ")).toLowerCase()}
                </span>
              )}
            </label>
          )}
        </form.Field>
        <form.Field name="body">
          {(field) => (
            <div className="flex flex-col gap-1 text-sm">
              <span className="font-semibold">Add a description</span>
              <MarkdownEditor
                label="Description"
                placeholder="Type your description here…"
                context={`${params.owner}/${params.repo}`}
                className="min-h-48"
                value={field.state.value}
                onBlur={field.handleBlur}
                onChange={field.handleChange}
                onSubmitShortcut={() => void form.handleSubmit()}
              />
            </div>
          )}
        </form.Field>
        {failure && (
          <Alert variant="destructive" data-testid="create-failure">
            <AlertTitle>The issue was not created</AlertTitle>
            <AlertDescription>
              {failure}. Your title and description are still here.
            </AlertDescription>
          </Alert>
        )}
        <div className="flex justify-end gap-2">
          <Button
            variant="outline"
            size="sm"
            nativeButton={false}
            render={<Link to="/$owner/$repo/issues" params={repo} />}
          >
            Cancel
          </Button>
          <form.Subscribe selector={(s) => s.isSubmitting}>
            {(submitting) => (
              <Button type="submit" size="sm" disabled={submitting}>
                {submitting ? "Creating…" : "Create"}
              </Button>
            )}
          </form.Subscribe>
        </div>
      </div>
      <aside className="flex flex-col gap-4 text-sm">
        <form.Field name="assignees">
          {(field) => (
            <SideSection
              title="Assignees"
              action={
                <AssigneePicker
                  {...repo}
                  selected={field.state.value.map((u) => u.login)}
                  onCommit={field.handleChange}
                />
              }
            >
              {field.state.value.length === 0 ? (
                <span className="text-muted-foreground text-xs">
                  No one —{" "}
                  <button
                    type="button"
                    className="text-primary hover:underline"
                    onClick={() => field.handleChange([viewer])}
                  >
                    assign yourself
                  </button>
                </span>
              ) : (
                <ul className="flex flex-col gap-1" data-testid="new-assignees">
                  {field.state.value.map((u) => (
                    <li key={u.login} className="flex items-center gap-2">
                      <UserAvatar user={u} />
                      {u.login}
                    </li>
                  ))}
                </ul>
              )}
            </SideSection>
          )}
        </form.Field>
        <Separator />
        <form.Field name="labels">
          {(field) => (
            <SideSection
              title="Labels"
              action={
                <LabelPicker
                  {...repo}
                  selected={field.state.value.map((l) => l.name)}
                  onCommit={field.handleChange}
                />
              }
            >
              {field.state.value.length === 0 ? (
                <span className="text-muted-foreground text-xs">None yet</span>
              ) : (
                <div className="flex flex-wrap gap-1" data-testid="new-labels">
                  {field.state.value.map((l) => (
                    <LabelBadge key={l.name} label={l} />
                  ))}
                </div>
              )}
            </SideSection>
          )}
        </form.Field>
        <Separator />
        <form.Field
          name="milestone"
          // A validator, even an empty one, lets the next change or submit clear the server's 422.
          validators={{ onChange: () => undefined }}
        >
          {(field) => (
            <SideSection
              title="Milestone"
              action={
                <MilestonePicker
                  {...repo}
                  selected={field.state.value?.number ?? null}
                  onCommit={field.handleChange}
                />
              }
            >
              {field.state.value ? (
                <span data-testid="new-milestone">
                  <span className="font-medium">{field.state.value.title}</span>
                  {field.state.value.due_on && (
                    <span className="text-muted-foreground block text-xs">
                      Due {formatDue(field.state.value.due_on)}
                    </span>
                  )}
                </span>
              ) : (
                <span className="text-muted-foreground text-xs">No milestone</span>
              )}
              {field.state.meta.errors.length > 0 && (
                <span className="text-destructive text-xs">
                  Milestone {String(field.state.meta.errors[0]).toLowerCase()}
                </span>
              )}
            </SideSection>
          )}
        </form.Field>
      </aside>
    </form>
  );
}

function SideSection({
  title,
  action,
  children,
}: {
  title: string;
  action: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-1.5">
      <div className="text-muted-foreground flex items-center justify-between text-xs font-semibold">
        {title}
        {action}
      </div>
      {children}
    </section>
  );
}
