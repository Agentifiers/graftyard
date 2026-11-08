import { useCallback, useMemo, useState, type ReactNode } from "react";
import { useForm } from "@tanstack/react-form";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { ArrowDown01Icon, File01Icon, Folder01Icon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { FileDiff, type Anchor, type DiffView, type Selection } from "@/components/diff";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Textarea } from "@/components/ui/textarea";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { cn } from "cn";
import {
  useAddDraftComment,
  useLineComment,
  useStartReview,
  useViewed,
  WriteError,
} from "./mutations";
import { sameAnchor } from "./patch";
import { pullFilesQuery, pullQuery, viewedQuery, type Pull } from "./queries";
import type { ReviewModel } from "./review-data";
import { threadAnchor, ThreadView, type Thread } from "./threads";

type File = Schemas["diff-entry"];
type Draft = { path: string; sel: Selection };

// The Files changed tab: a file tree beside the diffs, viewed checkboxes, split and
// unified views, line and range comments, and the threads already on each line.
export function FilesTab({
  pull,
  model,
  view,
  onView,
}: {
  pull: Pull;
  model: ReviewModel;
  view: DiffView;
  onView: (v: DiffView) => void;
}) {
  const { data: files } = useSuspenseQuery(pullFilesQuery(pull));
  const { data: pr } = useSuspenseQuery(pullQuery(pull));
  const { data: viewed } = useSuspenseQuery(viewedQuery(pull));
  const markViewed = useViewed(pull);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const seen = new Set(viewed.paths);
  const open = pr.state === "open";

  const threadsByFile = useMemo(() => {
    const m = new Map<string, Thread[]>();
    for (const t of model.threads) m.set(t.root.path, [...(m.get(t.root.path) ?? []), t]);
    return m;
  }, [model.threads]);

  const jump = (path: string) => {
    setExpanded((s) => new Set(s).add(path));
    document
      .querySelector(`[data-file="${CSS.escape(path)}"]`)
      ?.scrollIntoView({ behavior: "smooth", block: "start" });
  };

  return (
    <div className="flex flex-col gap-3" data-testid="files-tab">
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <span className="text-muted-foreground" data-testid="viewed-count">
          {seen.size} / {files.length} files viewed
        </span>
        {model.pending && (
          <Badge variant="warning-light" size="sm">
            Review in progress: {model.pendingComments.length} pending
          </Badge>
        )}
        <ToggleGroup
          className="ml-auto"
          size="sm"
          variant="outline"
          spacing={0}
          value={[view]}
          onValueChange={(v) => v[0] && onView(v[0] as DiffView)}
          aria-label="Diff view"
        >
          <ToggleGroupItem value="unified">Unified</ToggleGroupItem>
          <ToggleGroupItem value="split">Split</ToggleGroupItem>
        </ToggleGroup>
      </div>
      <div className="grid items-start gap-3 md:grid-cols-[14rem_minmax(0,1fr)]">
        <nav
          className="bg-card sticky top-2 max-h-[80vh] overflow-auto rounded-sm border p-1 text-sm"
          aria-label="Changed files"
        >
          <FileTree files={files} viewed={seen} threads={threadsByFile} onPick={jump} />
        </nav>
        <div className="flex min-w-0 flex-col gap-3">
          {files.map((f) => {
            const isViewed = seen.has(f.filename);
            const fileThreads = threadsByFile.get(f.filename) ?? [];
            const sel = draft?.path === f.filename ? draft.sel : null;
            return (
              <FileBlock
                key={f.filename}
                pull={pull}
                file={f}
                view={view}
                headSha={pr.head.sha}
                model={model}
                threads={fileThreads}
                collapsed={isViewed && !expanded.has(f.filename)}
                selection={open ? sel : null}
                onSelect={open ? (s) => setDraft({ path: f.filename, sel: s }) : undefined}
                onCancel={() => setDraft(null)}
                header={
                  <label className="ml-2 flex items-center gap-1.5 border-l pl-2 font-sans">
                    <Checkbox
                      checked={isViewed}
                      aria-label={`Viewed ${f.filename}`}
                      onCheckedChange={(on) => {
                        markViewed.mutate({ path: f.filename, viewed: on });
                        setExpanded((s) => {
                          const n = new Set(s);
                          n.delete(f.filename);
                          return n;
                        });
                      }}
                    />
                    Viewed
                  </label>
                }
                onExpand={() => setExpanded((s) => new Set(s).add(f.filename))}
              />
            );
          })}
        </div>
      </div>
    </div>
  );
}

function FileBlock({
  pull,
  file,
  view,
  headSha,
  model,
  threads,
  collapsed,
  selection,
  onSelect,
  onCancel,
  header,
  onExpand,
}: {
  pull: Pull;
  file: File;
  view: DiffView;
  headSha: string;
  model: ReviewModel;
  threads: Thread[];
  collapsed: boolean;
  selection: Selection | null;
  onSelect?: (s: Selection) => void;
  onCancel: () => void;
  header: ReactNode;
  onExpand: () => void;
}) {
  const open = Boolean(onSelect);
  const after = useCallback(
    (a: Anchor) => {
      const here = threads.filter((t) => sameAnchor(threadAnchor(t), a));
      const composing = selection && sameAnchor(selection.end, a);
      if (!here.length && !composing) return null;
      return (
        <div className="flex max-w-3xl flex-col gap-2">
          {here.map((t) => (
            <ThreadView
              key={t.root.id}
              pull={pull}
              thread={t}
              viewer={model.viewer.login}
              open={open}
            />
          ))}
          {composing && (
            <LineCommentForm
              pull={pull}
              path={file.filename}
              selection={selection}
              headSha={headSha}
              pendingId={model.pending?.id}
              onDone={onCancel}
            />
          )}
        </div>
      );
    },
    [
      threads,
      selection,
      pull,
      model.viewer.login,
      model.pending?.id,
      file.filename,
      headSha,
      onCancel,
      open,
    ],
  );
  return (
    <div className="flex flex-col">
      <FileDiff
        file={file}
        view={view}
        selection={selection}
        onSelect={onSelect}
        after={after}
        collapsed={collapsed}
        header={
          <>
            {threads.length > 0 && (
              <span className="text-muted-foreground ml-2 font-sans">
                {threads.length} {threads.length === 1 ? "thread" : "threads"}
              </span>
            )}
            {header}
          </>
        }
      />
      {collapsed && (
        <button
          type="button"
          className="text-muted-foreground hover:text-foreground -mt-px rounded-b-sm border px-3 py-1 text-left text-xs"
          onClick={onExpand}
        >
          Viewed. Show the diff again.
        </button>
      )}
    </div>
  );
}

function LineCommentForm({
  pull,
  path,
  selection,
  headSha,
  pendingId,
  onDone,
}: {
  pull: Pull;
  path: string;
  selection: Selection;
  headSha: string;
  pendingId?: number;
  onDone: () => void;
}) {
  const single = useLineComment(pull);
  const start = useStartReview(pull);
  const add = useAddDraftComment(pull);
  const [lo, hi] = [selection.start.line, selection.end.line].sort((a, b) => a - b);
  const comment = (body: string) => ({
    path,
    body,
    side: selection.end.side,
    line: hi,
    ...(lo !== hi ? { start_line: lo, start_side: selection.start.side } : {}),
  });
  const form = useForm({
    defaultValues: { body: "", mode: "single" as "single" | "review" },
    onSubmit: async ({ value, formApi }) => {
      const c = comment(value.body.trim());
      try {
        if (value.mode === "single") await single.mutateAsync({ ...c, commit_id: headSha });
        else if (pendingId) await add.mutateAsync({ reviewId: pendingId, comment: c });
        else await start.mutateAsync(c);
        onDone();
      } catch (e) {
        const msg =
          e instanceof WriteError ? (e.fields.body ?? e.fields.line ?? e.details[0]) : undefined;
        if (msg)
          formApi.setFieldMeta("body", (m) => ({
            ...m,
            errorMap: { ...m.errorMap, onServer: msg },
          }));
      }
    },
  });
  return (
    <form
      className="bg-card flex flex-col gap-2 rounded-sm border p-2"
      data-testid="line-comment-form"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <p className="text-muted-foreground text-xs">
        Commenting on {lo === hi ? `line ${hi}` : `lines ${lo} to ${hi}`} (
        {selection.end.side === "LEFT" ? "old" : "new"} side). Shift-click another line number to
        extend the range.
      </p>
      <form.Field
        name="body"
        validators={{
          onSubmit: ({ value }) => (value.trim() ? undefined : "Write a comment first."),
        }}
      >
        {(field) => (
          <>
            <Textarea
              autoFocus
              aria-label="Line comment"
              className="min-h-20 rounded-sm"
              placeholder="Leave a comment"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
            />
            {field.state.meta.errors.length > 0 && (
              <p className="text-destructive text-xs" role="alert">
                {field.state.meta.errors.join(", ")}
              </p>
            )}
          </>
        )}
      </form.Field>
      <div className="flex flex-wrap justify-end gap-1.5">
        <Button type="button" variant="ghost" size="sm" onClick={onDone}>
          Cancel
        </Button>
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(busy) => (
            <>
              {!pendingId && (
                <Button
                  type="submit"
                  variant="outline"
                  size="sm"
                  disabled={busy}
                  onClick={() => form.setFieldValue("mode", "single")}
                >
                  Add single comment
                </Button>
              )}
              <Button
                type="submit"
                size="sm"
                disabled={busy}
                onClick={() => form.setFieldValue("mode", "review")}
              >
                {pendingId ? "Add review comment" : "Start a review"}
              </Button>
            </>
          )}
        </form.Subscribe>
      </div>
    </form>
  );
}

// ---- File tree ----

type Dir = { name: string; path: string; dirs: Map<string, Dir>; files: File[] };

function buildTree(files: File[]): Dir {
  const root: Dir = { name: "", path: "", dirs: new Map(), files: [] };
  for (const f of files) {
    const parts = f.filename.split("/");
    let d = root;
    for (const part of parts.slice(0, -1)) {
      const path = d.path ? `${d.path}/${part}` : part;
      if (!d.dirs.has(part)) d.dirs.set(part, { name: part, path, dirs: new Map(), files: [] });
      d = d.dirs.get(part)!;
    }
    d.files.push(f);
  }
  // Collapse single-child folders into one row, as "crates/wire/src".
  const squash = (d: Dir): Dir => {
    const dirs = new Map([...d.dirs].map(([k, v]) => [k, squash(v)]));
    if (d.path && dirs.size === 1 && d.files.length === 0) {
      const [only] = dirs.values();
      return { ...only, name: `${d.name}/${only.name}` };
    }
    return { ...d, dirs };
  };
  return squash(root);
}

function FileTree({
  files,
  viewed,
  threads,
  onPick,
}: {
  files: File[];
  viewed: Set<string>;
  threads: Map<string, Thread[]>;
  onPick: (path: string) => void;
}) {
  const tree = useMemo(() => buildTree(files), [files]);
  const [closed, setClosed] = useState<Set<string>>(new Set());
  const toggle = (path: string) =>
    setClosed((s) => {
      const n = new Set(s);
      if (n.has(path)) n.delete(path);
      else n.add(path);
      return n;
    });
  const renderDir = (d: Dir, level: number): ReactNode => (
    <>
      {[...d.dirs.values()].map((sub) => (
        <li key={sub.path} role="treeitem" aria-expanded={!closed.has(sub.path)}>
          <button
            type="button"
            className="hover:bg-muted flex w-full items-center gap-1 rounded-sm py-1 pr-1 text-left"
            style={{ paddingLeft: level * 12 + 4 }}
            onClick={() => toggle(sub.path)}
          >
            <HugeiconsIcon
              icon={ArrowDown01Icon}
              strokeWidth={2}
              className={cn("text-muted-foreground size-3.5", closed.has(sub.path) && "-rotate-90")}
            />
            <HugeiconsIcon icon={Folder01Icon} strokeWidth={2} className="text-info size-3.5" />
            <span className="truncate">{sub.name}</span>
          </button>
          {!closed.has(sub.path) && <ul role="group">{renderDir(sub, level + 1)}</ul>}
        </li>
      ))}
      {d.files.map((f) => {
        const n = threads.get(f.filename)?.length ?? 0;
        return (
          <li key={f.filename} role="treeitem">
            <button
              type="button"
              className={cn(
                "hover:bg-muted flex w-full items-center gap-1 rounded-sm py-1 pr-1 text-left",
                viewed.has(f.filename) && "text-muted-foreground",
              )}
              style={{ paddingLeft: level * 12 + 22 }}
              onClick={() => onPick(f.filename)}
              data-tree-file={f.filename}
            >
              <HugeiconsIcon icon={File01Icon} strokeWidth={2} className="size-3.5 shrink-0" />
              <span className={cn("truncate", viewed.has(f.filename) && "line-through")}>
                {f.filename.split("/").pop()}
              </span>
              {n > 0 && <span className="text-muted-foreground ml-auto text-xs">{n}</span>}
              <span
                className={cn(
                  "size-2 shrink-0 rounded-sm",
                  n > 0 ? "" : "ml-auto",
                  f.status === "added"
                    ? "bg-success"
                    : f.status === "removed"
                      ? "bg-destructive"
                      : "bg-warning",
                )}
                title={f.status}
              />
            </button>
          </li>
        );
      })}
    </>
  );
  return (
    <ul role="tree" aria-label="Changed files" data-testid="file-tree">
      {renderDir(tree, 0)}
    </ul>
  );
}
