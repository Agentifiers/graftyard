import { useMemo, useState } from "react";
import { useForm, useStore } from "@tanstack/react-form";
import { useMutation } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { GitBranchIcon, GitCommitIcon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { queryClient } from "@/api/query-client";
import { FileDiff } from "@/components/diff";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
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
import { Input } from "@/components/ui/input";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Textarea } from "@/components/ui/textarea";
import { encodeText } from "./base64";
import {
  createBranch,
  deleteFile,
  errorText,
  invalidateCode,
  putFile,
  WriteError,
  type FieldErrors,
} from "./mutations";
import { contentsQuery } from "./queries";
import { joinPath, type Repo } from "./refs";
import { unifiedDiff } from "./text-diff";

// Edit, create, and delete a file, each committed through the contents API. "Commit
// to a new branch" first creates the branch from the current one, then commits there.

type Target = "direct" | "branch";

const VALID_BRANCH = /^(?!\/|.*(?:\/\.|\/\/|\.\.|@\{|\\))[^\0-\x20~^:?*[\x7f]+(?<![./]|\.lock)$/;

type CommitValues = {
  message: string;
  description: string;
  target: Target;
  newBranch: string;
  path: string;
  text: string;
};

// The one form both the editor and the delete dialog use, so CommitFields can type it.
function useCommitForm(
  defaults: Partial<CommitValues>,
  submit: (v: CommitValues) => Promise<unknown>,
) {
  return useForm({
    defaultValues: {
      message: "",
      description: "",
      target: "direct",
      newBranch: "",
      path: "",
      text: "",
      ...defaults,
    } as CommitValues,
    onSubmit: ({ value }) => submit(value),
  });
}

type CommitForm = ReturnType<typeof useCommitForm>;

function CommitFields({
  form,
  branch,
  serverErrors,
}: {
  form: CommitForm;
  branch: string;
  serverErrors: FieldErrors;
}) {
  return (
    <div className="flex flex-col gap-3">
      <form.Field
        name="message"
        validators={{
          onSubmit: ({ value }) => (value.trim() ? undefined : "Write a commit message."),
        }}
      >
        {(field) => (
          <Field>
            <FieldLabel htmlFor="commit-message">Commit message</FieldLabel>
            <Input
              id="commit-message"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
              onBlur={field.handleBlur}
              aria-invalid={
                field.state.meta.errors.length > 0 || Boolean(serverErrors.message) || undefined
              }
            />
            <FieldError
              errors={[...field.state.meta.errors, serverErrors.message]
                .filter(Boolean)
                .map((m) => ({ message: String(m) }))}
            />
          </Field>
        )}
      </form.Field>
      <form.Field name="description">
        {(field) => (
          <Field>
            <FieldLabel htmlFor="commit-description">Extended description</FieldLabel>
            <Textarea
              id="commit-description"
              className="min-h-16"
              placeholder="Add an optional extended description"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
            />
          </Field>
        )}
      </form.Field>
      <form.Field name="target">
        {(field) => (
          <RadioGroup
            value={field.state.value}
            onValueChange={(v) => field.handleChange(v as Target)}
            aria-label="Where to commit"
          >
            <label className="flex items-start gap-2 text-sm">
              <RadioGroupItem value="direct" className="mt-0.5" />
              <span>
                <HugeiconsIcon
                  icon={GitCommitIcon}
                  strokeWidth={2}
                  className="mr-1 inline size-3.5"
                />
                Commit directly to the{" "}
                <code className="bg-muted rounded-sm px-1 font-mono text-xs">{branch}</code> branch
              </span>
            </label>
            <label className="flex items-start gap-2 text-sm">
              <RadioGroupItem value="branch" className="mt-0.5" />
              <span>
                <HugeiconsIcon
                  icon={GitBranchIcon}
                  strokeWidth={2}
                  className="mr-1 inline size-3.5"
                />
                Create a new branch for this commit
              </span>
            </label>
          </RadioGroup>
        )}
      </form.Field>
      <form.Subscribe selector={(s) => s.values.target}>
        {(target) =>
          target === "branch" && (
            <form.Field
              name="newBranch"
              validators={{
                onSubmit: ({ value }) =>
                  !value.trim()
                    ? "Name the new branch."
                    : VALID_BRANCH.test(value.trim())
                      ? undefined
                      : "That is not a valid branch name.",
              }}
            >
              {(field) => (
                <Field>
                  <FieldLabel htmlFor="new-branch">New branch name</FieldLabel>
                  <Input
                    id="new-branch"
                    className="font-mono"
                    value={field.state.value}
                    onChange={(e) => field.handleChange(e.target.value)}
                    aria-invalid={
                      field.state.meta.errors.length > 0 || Boolean(serverErrors.ref) || undefined
                    }
                  />
                  <FieldDescription>Branches from {branch}.</FieldDescription>
                  <FieldError
                    errors={[
                      ...field.state.meta.errors,
                      serverErrors.ref && `Branch ${serverErrors.ref.replace(/^ref /, "")}`,
                    ]
                      .filter(Boolean)
                      .map((m) => ({ message: String(m) }))}
                  />
                </Field>
              )}
            </form.Field>
          )
        }
      </form.Subscribe>
    </div>
  );
}

function fullMessage(v: { message: string; description: string }) {
  return v.description.trim() ? `${v.message.trim()}\n\n${v.description.trim()}` : v.message.trim();
}

// Creates the branch when asked to, then runs the commit there.
async function onBranch(r: Repo, from: string, v: CommitValues) {
  if (v.target !== "branch") return from;
  const name = v.newBranch.trim();
  const head = await queryClient.fetchQuery({
    queryKey: ["repos", r.owner, r.repo, "branches", from, "sha"],
    queryFn: async () => {
      const { api } = await import("@/api/client");
      const { data, response } = await api.GET("/repos/{owner}/{repo}/branches/{branch}", {
        params: { path: { ...r, branch: from } },
      });
      if (!response.ok || !data) throw new Error(`Could not read ${from}`);
      return data.commit.sha;
    },
    staleTime: 0,
  });
  await createBranch(r, { name, sha: head });
  return name;
}

// ---- Edit and create ----

export function FileEditor({
  owner,
  repo,
  branch,
  file,
  dir = "",
  onClose,
}: Repo & {
  branch: string;
  // The file being edited, or none to add one under `dir`.
  file?: Schemas["content-file"] & { text: string };
  dir?: string;
  onClose: () => void;
}) {
  const r = { owner, repo };
  const navigate = useNavigate();
  const [serverErrors, setServerErrors] = useState<FieldErrors>({});
  const [failure, setFailure] = useState<string>();
  const [tab, setTab] = useState<"edit" | "preview">("edit");
  const creating = !file;
  const save = useMutation({
    mutationFn: async (v: CommitValues) => {
      const target = await onBranch(r, branch, v);
      const path = creating ? joinPath(dir, v.path.trim()) : file.path;
      return {
        target,
        path,
        result: await putFile(r, {
          path,
          text: v.text,
          message: fullMessage(v),
          branch: target,
          sha: file?.sha,
        }),
      };
    },
    onMutate: (v) => {
      // The edit shows at once in the file view; a failure puts the old content back.
      if (!file) return undefined;
      const key = contentsQuery({ owner, repo, ref: branch, path: file.path }).queryKey;
      const previous = queryClient.getQueryData(key);
      if (v.target === "direct") {
        const { text: _text, ...wire } = file;
        const optimistic: Schemas["content-file"] = { ...wire, content: encodeText(v.text) };
        queryClient.setQueryData(key, optimistic);
      }
      return { key, previous };
    },
    onError: (e, _v, ctx) => {
      if (ctx?.previous) queryClient.setQueryData(ctx.key, ctx.previous);
      setServerErrors(e instanceof WriteError ? e.fields : {});
      setFailure(errorText(e));
      toast.error(`Could not commit: ${errorText(e)}`);
    },
    onSuccess: async ({ target, path, result }) => {
      toast.success(`Committed ${result.commit.sha?.slice(0, 7)} to ${target}`);
      await invalidateCode(r);
      onClose();
      void navigate({
        to: "/$owner/$repo/blob/$",
        params: { owner, repo, _splat: joinPath(target, path) },
      });
    },
  });
  const form = useCommitForm(
    { message: creating ? "" : `Update ${file.name}`, text: file?.text ?? "" },
    (value) => {
      setServerErrors({});
      setFailure(undefined);
      return save.mutateAsync(value).catch(() => {});
    },
  );
  const text = useStore(form.store, (s) => s.values.text);
  const patch = useMemo(() => (file ? unifiedDiff(file.text, text) : undefined), [file, text]);
  const draftDiff: Schemas["diff-entry"] | undefined =
    patch && file
      ? {
          sha: null,
          filename: file.path,
          status: "modified",
          additions: patch.additions,
          deletions: patch.deletions,
          changes: patch.additions + patch.deletions,
          blob_url: "",
          raw_url: "",
          contents_url: "",
          ...(patch.patch ? { patch: patch.patch } : {}),
        }
      : undefined;

  return (
    <form
      className="flex flex-col gap-4"
      data-testid="file-editor"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <div className="flex flex-wrap items-center gap-2">
        {creating ? (
          <form.Field
            name="path"
            validators={{
              onSubmit: ({ value }) =>
                !value?.trim()
                  ? "Name the file."
                  : value.split("/").some((s) => !s || s === "." || s === "..")
                    ? "A path cannot have empty, . or .. parts."
                    : undefined,
            }}
          >
            {(field) => (
              <Field className="min-w-64 flex-1">
                <FieldLabel htmlFor="file-path">
                  Name your file in{" "}
                  <code className="font-mono">{dir ? `${repo}/${dir}/` : `${repo}/`}</code>
                </FieldLabel>
                <Input
                  id="file-path"
                  className="font-mono"
                  placeholder="docs/notes.md"
                  value={field.state.value}
                  onChange={(e) => field.handleChange(e.target.value)}
                  aria-invalid={
                    field.state.meta.errors.length > 0 || Boolean(serverErrors.path) || undefined
                  }
                />
                <FieldError
                  errors={[...field.state.meta.errors, serverErrors.path]
                    .filter(Boolean)
                    .map((m) => ({ message: String(m) }))}
                />
              </Field>
            )}
          </form.Field>
        ) : (
          <p className="text-sm">
            Editing <code className="font-mono font-semibold">{file.path}</code> on{" "}
            <code className="bg-muted rounded-sm px-1 font-mono text-xs">{branch}</code>
          </p>
        )}
        {!creating && (
          <div className="ml-auto flex border-b text-xs" role="tablist" aria-label="Editor view">
            {(["edit", "preview"] as const).map((t) => (
              <button
                key={t}
                type="button"
                role="tab"
                aria-selected={tab === t}
                onClick={() => setTab(t)}
                className="text-muted-foreground aria-selected:border-primary aria-selected:text-foreground -mb-px border-b-2 border-transparent px-2 py-1 font-medium"
              >
                {t === "edit" ? "Edit" : "Preview changes"}
              </button>
            ))}
          </div>
        )}
      </div>
      {tab === "edit" ? (
        <form.Field name="text">
          {(field) => (
            <Textarea
              aria-label="File contents"
              spellCheck={false}
              className="min-h-80 font-mono text-xs leading-5"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Tab") {
                  e.preventDefault();
                  const el = e.currentTarget;
                  const { selectionStart: s, selectionEnd: end } = el;
                  const v = el.value;
                  field.handleChange(`${v.slice(0, s)}    ${v.slice(end)}`);
                  requestAnimationFrame(() => el.setSelectionRange(s + 4, s + 4));
                }
              }}
            />
          )}
        </form.Field>
      ) : draftDiff && draftDiff.changes > 0 ? (
        <FileDiff file={draftDiff} />
      ) : (
        <p className="text-muted-foreground border px-3 py-6 text-center text-sm">
          No changes yet.
        </p>
      )}
      <fieldset className="bg-card flex flex-col gap-3 rounded-sm border p-3">
        <legend className="px-1 text-sm font-semibold">Commit changes</legend>
        <CommitFields form={form} branch={branch} serverErrors={serverErrors} />
        {serverErrors.content && (
          <p className="text-destructive text-xs">
            File {serverErrors.content.replace(/^content /, "")}.
          </p>
        )}
        {serverErrors.sha && (
          <p className="text-destructive text-xs">
            The file changed on the branch since you opened it. Reload and try again.
          </p>
        )}
        {failure && Object.keys(serverErrors).length === 0 && (
          <Alert variant="destructive">
            <AlertTitle>Commit failed</AlertTitle>
            <AlertDescription>{failure}. Your changes are still here.</AlertDescription>
          </Alert>
        )}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="outline" size="sm" onClick={onClose}>
            Cancel
          </Button>
          <form.Subscribe selector={(s) => s.isSubmitting}>
            {(submitting) => (
              <Button type="submit" size="sm" disabled={submitting}>
                {submitting ? "Committing" : "Commit changes"}
              </Button>
            )}
          </form.Subscribe>
        </div>
      </fieldset>
    </form>
  );
}

// ---- Delete ----

export function DeleteFileDialog({
  owner,
  repo,
  branch,
  file,
  open,
  onOpenChange,
}: Repo & {
  branch: string;
  file: { path: string; sha: string; name: string };
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const r = { owner, repo };
  const navigate = useNavigate();
  const [serverErrors, setServerErrors] = useState<FieldErrors>({});
  const remove = useMutation({
    mutationFn: async (v: CommitValues) => {
      const target = await onBranch(r, branch, v);
      await deleteFile(r, {
        path: file.path,
        sha: file.sha,
        message: fullMessage(v),
        branch: target,
      });
      return target;
    },
    onError: (e) => {
      setServerErrors(e instanceof WriteError ? e.fields : {});
      toast.error(`Could not delete ${file.name}: ${errorText(e)}`);
    },
    onSuccess: async (target) => {
      toast.success(`Deleted ${file.path} on ${target}`);
      onOpenChange(false);
      await invalidateCode(r);
      const dir = file.path.includes("/") ? file.path.slice(0, file.path.lastIndexOf("/")) : "";
      void navigate({
        to: "/$owner/$repo/tree/$",
        params: { owner, repo, _splat: joinPath(target, dir) },
      });
    },
  });
  const form = useCommitForm({ message: `Delete ${file.name}` }, (value) => {
    setServerErrors({});
    return remove.mutateAsync(value).catch(() => {});
  });
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg" data-testid="delete-dialog">
        <form
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <DialogHeader>
            <DialogTitle>Delete {file.name}</DialogTitle>
            <DialogDescription>
              This commits the removal of <code className="font-mono">{file.path}</code>. Its
              history stays.
            </DialogDescription>
          </DialogHeader>
          <CommitFields form={form} branch={branch} serverErrors={serverErrors} />
          <DialogFooter>
            <Button type="button" variant="outline" size="sm" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <form.Subscribe selector={(s) => s.isSubmitting}>
              {(submitting) => (
                <Button type="submit" variant="destructive" size="sm" disabled={submitting}>
                  {submitting ? "Deleting" : "Commit deletion"}
                </Button>
              )}
            </form.Subscribe>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
