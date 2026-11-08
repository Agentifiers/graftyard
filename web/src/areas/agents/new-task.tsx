import { useState } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { Add01Icon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { issuesQuery } from "@/areas/issues/queries";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import {
  Autocomplete,
  AutocompleteContent,
  AutocompleteEmpty,
  AutocompleteInput,
  AutocompleteItem,
  AutocompleteList,
} from "@/components/reui/autocomplete";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { shortId } from "./states";
import { call, GraftyardError, gy } from "./api";
import { tasksKey } from "./queries";

type Repo = { owner: string; repo: string };
type Issue = Schemas["issue"];

export const PROMPT_MAX_BYTES = 64 * 1024;
const bytes = (s: string) => new TextEncoder().encode(s).length;

export type CreateInput = { issue_number?: number; prompt?: string };

// POST .../tasks: no optimistic row, since the server mints the id and the prefix. On
// success the lists refetch; on failure the dialog keeps its input.
export function useCreateTask(r: Repo) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateInput) =>
      call(gy.POST("/repos/{owner}/{repo}/tasks", { params: { path: r }, body })),
    onSuccess: () => client.invalidateQueries({ queryKey: tasksKey(r) }),
  });
}

// A 422 without field errors is a policy refusal, such as agents_enabled off.
export function PolicyRefusal({ error, owner }: { error: GraftyardError; owner: string }) {
  const off = /agents_enabled/.test(error.message);
  return (
    <Alert variant="destructive" data-policy-refusal>
      <AlertTitle className="line-clamp-none">
        {off ? "Agents are off for this organization" : "The task was refused"}
      </AlertTitle>
      <AlertDescription>
        <p>{error.message}</p>
        {off && (
          <p>
            An owner of <strong>{owner}</strong> can turn on "Enable Graftyard-run agents" in the{" "}
            <Link
              to="/orgs/$org/settings/agents"
              params={{ org: owner }}
              className="font-medium underline"
            >
              organization's agent settings
            </Link>
            .
          </p>
        )}
      </AlertDescription>
    </Alert>
  );
}

export function NewTaskButton(r: Repo) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button size="sm" onClick={() => setOpen(true)} data-new-task>
        <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
        New task
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="sm:max-w-lg">
          {open && <NewTaskForm {...r} onDone={() => setOpen(false)} />}
        </DialogContent>
      </Dialog>
    </>
  );
}

function NewTaskForm({ onDone, ...r }: Repo & { onDone: () => void }) {
  const navigate = useNavigate();
  const create = useCreateTask(r);
  const issues = useQuery(issuesQuery(r));
  const open = (issues.data ?? []).filter((i) => !i.pull_request && i.state === "open");
  const [source, setSource] = useState<"issue" | "prompt">("issue");
  const [refusal, setRefusal] = useState<GraftyardError>();

  const form = useForm({
    defaultValues: { issue: null as Issue | null, issueText: "", prompt: "" },
    onSubmit: async ({ value, formApi }) => {
      setRefusal(undefined);
      const body: CreateInput =
        source === "issue" ? { issue_number: value.issue!.number } : { prompt: value.prompt };
      try {
        const task = await create.mutateAsync(body);
        toast.success("Task queued", {
          description: `Session ${shortId(task.id)} is waiting for a sandbox.`,
        });
        onDone();
        void navigate({
          to: "/$owner/$repo/_graftyard/sessions/$uuid",
          params: { ...r, uuid: task.id },
        });
      } catch (e) {
        if (!(e instanceof GraftyardError)) throw e;
        const fields = e.fieldErrors();
        if (e.status === 422 && Object.keys(fields).length) {
          if (fields.issue_number)
            formApi.setFieldMeta(source === "issue" ? "issue" : "prompt", (m) => ({
              ...m,
              errorMap: {
                onServer:
                  source === "issue"
                    ? `Not an open issue: ${fields.issue_number}`
                    : "Pick an issue or write a prompt.",
              },
            }));
          if (fields.prompt)
            formApi.setFieldMeta("prompt", (m) => ({
              ...m,
              errorMap: { onServer: "The server refused the prompt: at most 64 KiB." },
            }));
        } else if (e.status === 422 || e.status === 403) {
          setRefusal(e);
        } else {
          toast.error("Task not created", { description: e.message });
        }
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
    >
      <DialogHeader>
        <DialogTitle>New agent task</DialogTitle>
        <DialogDescription>
          graftyard-agent works on a branch under its own prefix and opens a draft pull request. You
          review it like any other.
        </DialogDescription>
      </DialogHeader>
      <Tabs value={source} onValueChange={(v) => setSource(v as typeof source)}>
        <TabsList>
          <TabsTrigger value="issue">From an issue</TabsTrigger>
          <TabsTrigger value="prompt">From a prompt</TabsTrigger>
        </TabsList>
      </Tabs>

      {source === "issue" ? (
        <form.Field
          name="issue"
          validators={{ onSubmit: ({ value }) => (value ? undefined : "Choose an open issue.") }}
        >
          {(field) => (
            <Field data-invalid={field.state.meta.errors.length > 0 || undefined}>
              <FieldLabel htmlFor="task-issue">Issue</FieldLabel>
              <form.Field name="issueText">
                {(text) => (
                  <Autocomplete
                    items={open}
                    value={text.state.value}
                    onValueChange={(v) => {
                      text.handleChange(v);
                      const match = open.find((i) => label(i) === v);
                      field.handleChange(match ?? null);
                    }}
                    itemToStringValue={(i: Issue) => label(i)}
                  >
                    <AutocompleteInput
                      id="task-issue"
                      placeholder={
                        issues.isPending
                          ? "Loading issues…"
                          : "Search open issues by number or title"
                      }
                      showClear
                      aria-invalid={field.state.meta.errors.length > 0 || undefined}
                    />
                    <AutocompleteContent>
                      <AutocompleteEmpty>No open issue matches.</AutocompleteEmpty>
                      <AutocompleteList>
                        {(i: Issue) => (
                          <AutocompleteItem key={i.number} value={i} data-issue-option={i.number}>
                            <span className="text-muted-foreground font-mono text-xs">
                              #{i.number}
                            </span>
                            <span className="truncate">{i.title}</span>
                          </AutocompleteItem>
                        )}
                      </AutocompleteList>
                    </AutocompleteContent>
                  </Autocomplete>
                )}
              </form.Field>
              {issues.isError && (
                <FieldDescription className="text-destructive">
                  Could not load issues: {issues.error.message}
                </FieldDescription>
              )}
              <FieldDescription>
                The agent sees the issue as it reads now, through the trust view.
              </FieldDescription>
              <FieldError
                errors={field.state.meta.errors.map((message) => ({ message: String(message) }))}
              />
            </Field>
          )}
        </form.Field>
      ) : (
        <form.Field
          name="prompt"
          validators={{
            onChange: ({ value }) =>
              bytes(value) > PROMPT_MAX_BYTES ? "A prompt is at most 64 KiB." : undefined,
            onSubmit: ({ value }) => (value.trim() ? undefined : "Write a prompt first."),
          }}
        >
          {(field) => {
            const used = bytes(field.state.value);
            return (
              <Field data-invalid={field.state.meta.errors.length > 0 || undefined}>
                <FieldLabel htmlFor="task-prompt">Prompt</FieldLabel>
                <Textarea
                  id="task-prompt"
                  className="max-h-72 min-h-32 font-mono text-xs"
                  placeholder="Describe the change. The agent cannot touch workflows, CODEOWNERS, or AGENTS.md."
                  value={field.state.value}
                  onBlur={field.handleBlur}
                  onChange={(e) => field.handleChange(e.target.value)}
                  aria-invalid={field.state.meta.errors.length > 0 || undefined}
                />
                <FieldDescription
                  className={
                    used > PROMPT_MAX_BYTES ? "text-destructive tabular-nums" : "tabular-nums"
                  }
                  data-prompt-counter={used}
                >
                  {used.toLocaleString()} / {PROMPT_MAX_BYTES.toLocaleString()} bytes
                </FieldDescription>
                <FieldError
                  errors={field.state.meta.errors.map((message) => ({ message: String(message) }))}
                />
              </Field>
            );
          }}
        </form.Field>
      )}

      {refusal && <PolicyRefusal error={refusal} owner={r.owner} />}

      <DialogFooter>
        <Button type="button" variant="outline" onClick={onDone}>
          Close
        </Button>
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <Button type="submit" disabled={submitting} data-submit-task>
              {submitting ? "Creating…" : "Create task"}
            </Button>
          )}
        </form.Subscribe>
      </DialogFooter>
    </form>
  );
}

const label = (i: Issue) => `#${i.number} ${i.title}`;
