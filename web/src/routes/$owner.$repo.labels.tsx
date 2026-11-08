import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useForm } from "@tanstack/react-form";
import { useLiveSuspenseQuery } from "@tanstack/react-db";
import { HugeiconsIcon } from "@hugeicons/react";
import { RefreshIcon, Search01Icon, Tag01Icon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { errorText, toastFailure, ValidationError } from "@/areas/issues/api";
import { draftLabel, issuesCollection, labelsCollection } from "@/areas/issues/collections";
import { LabelBadge } from "@/components/github";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { ColorPicker } from "@/components/reui/color-picker";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

export const Route = createFileRoute("/$owner/$repo/labels")({
  validateSearch: (s: Record<string, unknown>): { q?: string } => ({
    q: typeof s.q === "string" && s.q ? s.q : undefined,
  }),
  loader: ({ params }) => {
    void labelsCollection(params)
      .preload()
      .catch(() => {});
    void issuesCollection(params)
      .preload()
      .catch(() => {});
  },
  component: LabelsPage,
});

const SWATCHES = [
  "b60205",
  "d93f0b",
  "fbca04",
  "0e8a16",
  "006b75",
  "1d76db",
  "0052cc",
  "5319e7",
  "e99695",
  "c5def5",
  "bfdadc",
  "d4c5f9",
];

const randomColor = () => SWATCHES[Math.floor(Math.random() * SWATCHES.length)];

function LabelsPage() {
  const params = Route.useParams();
  const { q = "" } = Route.useSearch();
  const navigate = Route.useNavigate();
  const [creating, setCreating] = useState(false);
  return (
    <div className="flex flex-col gap-3">
      <PageHeader
        title="Labels"
        description={
          <>
            Labels sort{" "}
            <Link
              to="/$owner/$repo/issues"
              params={params}
              className="text-primary hover:underline"
            >
              issues
            </Link>{" "}
            and pull requests. Renaming or recolouring one changes it everywhere it is used.
          </>
        }
        actions={
          <Button size="sm" onClick={() => setCreating(true)} disabled={creating}>
            New label
          </Button>
        }
      />
      <label className="relative max-w-xs">
        <HugeiconsIcon
          icon={Search01Icon}
          strokeWidth={2}
          className="text-muted-foreground absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2"
        />
        <Input
          type="search"
          aria-label="Search all labels"
          placeholder="Search all labels"
          className="rounded-sm pl-8"
          value={q}
          onChange={(e) =>
            void navigate({ search: { q: e.target.value || undefined }, replace: true })
          }
        />
      </label>
      {creating && (
        <Panel title="New label">
          <LabelForm mode="create" done={() => setCreating(false)} />
        </Panel>
      )}
      <Region name="labels" rows={8} onRetry={() => labelsCollection(params).reload()}>
        <LabelList q={q} />
      </Region>
    </div>
  );
}

function LabelList({ q }: { q: string }) {
  const params = Route.useParams();
  const { data } = useLiveSuspenseQuery({
    query: (qb) => qb.from({ l: labelsCollection(params) }).orderBy(({ l }) => l.name, "asc"),
  });
  const { data: issues } = useLiveSuspenseQuery({
    query: (qb) => qb.from({ i: issuesCollection(params) }),
  });
  const shown = data.filter(
    (l) =>
      l.name.toLowerCase().includes(q.toLowerCase()) ||
      (l.description ?? "").toLowerCase().includes(q.toLowerCase()),
  );
  const openCount = (name: string) =>
    issues.filter(
      (i) =>
        i.state === "open" &&
        !i.pull_request &&
        i.labels.some((x) => (typeof x === "string" ? x : x.name) === name),
    ).length;
  return (
    <Panel title={`${shown.length} ${shown.length === 1 ? "label" : "labels"}`} flush>
      {shown.length === 0 ? (
        <p className="text-muted-foreground px-4 py-10 text-center text-sm">No labels match.</p>
      ) : (
        <ul className="divide-y" data-testid="label-list">
          {shown.map((l) => (
            <LabelRow key={l.id} label={l} open={openCount(l.name)} pending={l.$hasPendingWrites} />
          ))}
        </ul>
      )}
    </Panel>
  );
}

function LabelRow({
  label,
  open,
  pending,
}: {
  label: Schemas["label"];
  open: number;
  pending: boolean;
}) {
  const params = Route.useParams();
  const [editing, setEditing] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const remove = async () => {
    setConfirm(false);
    const tx = labelsCollection(params).delete(label.id);
    try {
      await tx.when("settled");
    } catch (e) {
      toastFailure(`Deleting “${label.name}”`, e);
    }
  };
  if (editing) {
    return (
      <li className="px-4 py-3" data-label={label.name}>
        <LabelForm mode="edit" label={label} done={() => setEditing(false)} />
      </li>
    );
  }
  return (
    <li
      className="grid grid-cols-[minmax(0,14rem)_minmax(0,1fr)_7rem_auto] items-center gap-3 px-4 py-2.5 text-sm"
      data-label={label.name}
      data-pending={pending || undefined}
    >
      <span>
        <LabelBadge label={label} />
      </span>
      <span className="text-muted-foreground truncate text-xs">{label.description}</span>
      <Link
        to="/$owner/$repo/issues"
        params={params}
        search={{ filters: [{ f: "label", op: "has_any_of", v: [label.name] }] }}
        className="text-muted-foreground hover:text-foreground text-xs"
      >
        {open} open {open === 1 ? "issue" : "issues"}
      </Link>
      <span className="flex gap-1">
        <Button variant="ghost" size="sm" onClick={() => setEditing(true)} disabled={label.id < 0}>
          Edit
        </Button>
        <Button
          variant="ghost"
          size="sm"
          className="text-destructive"
          onClick={() => setConfirm(true)}
          disabled={label.id < 0}
        >
          Delete
        </Button>
      </span>
      <AlertDialog open={confirm} onOpenChange={setConfirm}>
        <AlertDialogContent className="rounded-sm">
          <AlertDialogHeader>
            <AlertDialogTitle>Delete “{label.name}”?</AlertDialogTitle>
            <AlertDialogDescription>
              It will be removed from every issue and pull request that has it. This cannot be
              undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction variant="destructive" onClick={() => void remove()}>
              Delete label
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </li>
  );
}

type LabelValues = { name: string; color: string; description: string };

// Create and edit share one form. The preview badge follows every keystroke and colour
// drag; the write goes through the labels collection, so the list shows it at once.
function LabelForm({
  mode,
  label,
  done,
}: {
  mode: "create" | "edit";
  label?: Schemas["label"];
  done: () => void;
}) {
  const params = Route.useParams();
  const [failure, setFailure] = useState<string>();
  const form = useForm({
    defaultValues: {
      name: label?.name ?? "",
      color: label?.color ?? randomColor(),
      description: label?.description ?? "",
    } as LabelValues,
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const clean = {
        name: value.name.trim(),
        color: value.color.replace(/^#/, "").toLowerCase(),
        description: value.description.trim(),
      };
      const labels = labelsCollection(params);
      const tx =
        mode === "create"
          ? labels.insert(draftLabel(clean))
          : labels.update(label!.id, (d) => {
              d.name = clean.name;
              d.color = clean.color;
              d.description = clean.description || null;
            });
      try {
        await tx.when("settled");
        done();
      } catch (e) {
        if (e instanceof ValidationError) {
          for (const [field, message] of Object.entries(e.fields)) {
            if (field === "name" || field === "color")
              formApi.setFieldMeta(field, (m) => ({ ...m, errorMap: { onServer: message } }));
          }
        }
        setFailure(errorText(e));
        toastFailure(mode === "create" ? `Creating “${clean.name}”` : `Saving “${label!.name}”`, e);
      }
    },
  });
  return (
    <form
      className="flex flex-col gap-3"
      data-testid={`label-form-${mode}`}
      noValidate
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Subscribe selector={(s) => s.values}>
        {(v) => (
          <div data-testid="label-preview">
            <LabelBadge
              label={{
                name: v.name.trim() || "Label preview",
                color: v.color.replace(/^#/, ""),
                description: v.description,
              }}
            />
          </div>
        )}
      </form.Subscribe>
      <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.5fr)_12rem_auto] sm:items-start">
        <form.Field
          name="name"
          validators={{
            onSubmit: ({ value }) => (value.trim() ? undefined : "Can't be blank."),
            onChange: ({ value }) =>
              value.length > 50 ? "Keep it under 50 characters." : undefined,
          }}
        >
          {(field) => (
            <label className="flex flex-col gap-1 text-sm">
              <span className="font-medium">Label name</span>
              <Input
                aria-label="Label name"
                className="rounded-sm"
                placeholder="Label name"
                value={field.state.value}
                aria-invalid={field.state.meta.errors.length > 0 || undefined}
                onBlur={field.handleBlur}
                onChange={(e) => field.handleChange(e.target.value)}
              />
              {field.state.meta.errors.length > 0 && (
                <span className="text-destructive text-xs" data-field-error="name">
                  Name {String(field.state.meta.errors.join(", ")).toLowerCase()}
                </span>
              )}
            </label>
          )}
        </form.Field>
        <form.Field name="description">
          {(field) => (
            <label className="flex flex-col gap-1 text-sm">
              <span className="font-medium">Description</span>
              <Input
                aria-label="Description"
                className="rounded-sm"
                placeholder="Description (optional)"
                value={field.state.value}
                onChange={(e) => field.handleChange(e.target.value)}
              />
            </label>
          )}
        </form.Field>
        <form.Field
          name="color"
          validators={{
            onChange: ({ value }) =>
              /^#?[0-9a-f]{6}$/i.test(value) ? undefined : "Use six hex digits.",
          }}
        >
          {(field) => (
            <div className="flex flex-col gap-1 text-sm">
              <span className="font-medium">Color</span>
              <div className="flex gap-1">
                <Button
                  type="button"
                  variant="outline"
                  size="icon"
                  aria-label="Pick a random colour"
                  onClick={() => field.handleChange(randomColor())}
                  style={{ backgroundColor: `#${field.state.value.replace(/^#/, "")}` }}
                >
                  <HugeiconsIcon
                    icon={RefreshIcon}
                    strokeWidth={2}
                    className="mix-blend-difference text-white"
                  />
                </Button>
                <ColorPicker
                  aria-label="Label colour"
                  className="min-w-0 flex-1 rounded-sm"
                  format="hex"
                  swatches={SWATCHES.map((c) => `#${c}`)}
                  value={
                    /^#?[0-9a-f]{6}$/i.test(field.state.value)
                      ? `#${field.state.value.replace(/^#/, "")}`
                      : null
                  }
                  onValueChange={(v) =>
                    v && field.handleChange(v.replace(/^#/, "").slice(0, 6).toLowerCase())
                  }
                  invalid={field.state.meta.errors.length > 0}
                />
              </div>
              <Input
                aria-label="Colour hex"
                className="h-7 rounded-sm font-mono text-xs"
                value={field.state.value}
                onChange={(e) => field.handleChange(e.target.value.replace(/^#/, ""))}
              />
              {field.state.meta.errors.length > 0 && (
                <span className="text-destructive text-xs" data-field-error="color">
                  {field.state.meta.errors.join(", ")}
                </span>
              )}
            </div>
          )}
        </form.Field>
        <div className="flex gap-2 sm:pt-6">
          <Button type="button" variant="outline" size="sm" onClick={done}>
            Cancel
          </Button>
          <form.Subscribe selector={(s) => s.isSubmitting}>
            {(submitting) => (
              <Button type="submit" size="sm" disabled={submitting}>
                <HugeiconsIcon icon={Tag01Icon} strokeWidth={2} data-icon="inline-start" />
                {mode === "create" ? "Create label" : "Save changes"}
              </Button>
            )}
          </form.Subscribe>
        </div>
      </div>
      {failure && <p className="text-destructive text-xs">Not saved: {failure}.</p>}
    </form>
  );
}
