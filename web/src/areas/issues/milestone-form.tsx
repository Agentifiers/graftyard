import { useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useForm } from "@tanstack/react-form";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { Calendar03Icon, Cancel01Icon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { userQuery } from "@/areas/core/queries";
import { errorText, toastFailure, ValidationError } from "./api";
import { createdNumberFor, draftMilestone, milestonesCollection } from "./collections";
import { formatDue } from "./pickers";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Button } from "@/components/ui/button";
import { Calendar } from "@/components/ui/calendar";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Textarea } from "@/components/ui/textarea";

type Repo = { owner: string; repo: string };
type Values = { title: string; description: string; due: Date | undefined };

// GHES stores a due date as that day at 07:00 UTC; the calendar works in local days.
const toDueOn = (d: Date | undefined) =>
  d
    ? new Date(Date.UTC(d.getFullYear(), d.getMonth(), d.getDate(), 7))
        .toISOString()
        .replace(".000", "")
    : null;
const fromDueOn = (s: string | null) => {
  if (!s) return undefined;
  const d = new Date(s);
  return new Date(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate());
};

// Create and edit a milestone. Writes go through the milestones collection, so the list
// shows the change at once; a 422 (blank or duplicate title) shows on the field.
export function MilestoneForm({ milestone, ...repo }: Repo & { milestone?: Schemas["milestone"] }) {
  const navigate = useNavigate();
  const { data: viewer } = useSuspenseQuery(userQuery());
  const [failure, setFailure] = useState<string>();
  const [open, setOpen] = useState(false);
  const form = useForm({
    defaultValues: {
      title: milestone?.title ?? "",
      description: milestone?.description ?? "",
      due: fromDueOn(milestone?.due_on ?? null),
    } as Values,
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const input = {
        title: value.title.trim(),
        description: value.description.trim(),
        due_on: toDueOn(value.due),
      };
      const c = milestonesCollection(repo);
      const draft = milestone ? undefined : draftMilestone(viewer, input);
      const tx = draft
        ? c.insert(draft)
        : c.update(milestone!.number, (d) => {
            d.title = input.title;
            d.description = input.description || null;
            d.due_on = input.due_on;
          });
      try {
        await tx.when("settled");
        const number = draft ? createdNumberFor(draft.number) : milestone!.number;
        if (number)
          await navigate({ to: "/$owner/$repo/milestones/$number", params: { ...repo, number } });
      } catch (e) {
        if (e instanceof ValidationError && e.fields.title) {
          formApi.setFieldMeta("title", (m) => ({ ...m, errorMap: { onServer: e.fields.title } }));
        }
        setFailure(errorText(e));
        toastFailure(milestone ? "Saving the milestone" : "Creating the milestone", e);
      }
    },
  });
  return (
    <form
      className="flex max-w-2xl flex-col gap-4"
      noValidate
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field
        name="title"
        validators={{
          // Any validator makes a change clear the server's 422; a blank title is left to the server.
          onChange: ({ value }) =>
            value.length > 256 ? "Keep the title under 256 characters." : undefined,
        }}
      >
        {(field) => (
          <label className="flex flex-col gap-1 text-sm">
            <span className="font-semibold">Title</span>
            <Input
              aria-label="Title"
              autoFocus
              className="rounded-sm"
              placeholder="Title"
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
      <form.Field name="due">
        {(field) => (
          <div className="flex flex-col gap-1 text-sm">
            <span className="font-semibold">Due date (optional)</span>
            <div className="flex items-center gap-2">
              <Popover open={open} onOpenChange={setOpen}>
                <PopoverTrigger
                  render={
                    <Button
                      type="button"
                      variant="outline"
                      className="w-56 justify-start rounded-sm"
                      aria-label="Due date"
                    >
                      <HugeiconsIcon
                        icon={Calendar03Icon}
                        strokeWidth={2}
                        data-icon="inline-start"
                      />
                      <span data-testid="due-value">
                        {field.state.value ? formatDue(toDueOn(field.state.value)!) : "No due date"}
                      </span>
                    </Button>
                  }
                />
                <PopoverContent align="start" className="w-auto rounded-sm p-0">
                  <Calendar
                    mode="single"
                    selected={field.state.value}
                    defaultMonth={field.state.value ?? new Date()}
                    onSelect={(d) => {
                      field.handleChange(d);
                      setOpen(false);
                    }}
                  />
                </PopoverContent>
              </Popover>
              {field.state.value && (
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Clear due date"
                  onClick={() => field.handleChange(undefined)}
                >
                  <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} />
                </Button>
              )}
            </div>
          </div>
        )}
      </form.Field>
      <form.Field name="description">
        {(field) => (
          <label className="flex flex-col gap-1 text-sm">
            <span className="font-semibold">Description</span>
            <Textarea
              aria-label="Description"
              className="min-h-28 rounded-sm"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
            />
          </label>
        )}
      </form.Field>
      {failure && (
        <Alert variant="destructive">
          <AlertTitle>{milestone ? "Not saved" : "Not created"}</AlertTitle>
          <AlertDescription>{failure}.</AlertDescription>
        </Alert>
      )}
      <div className="flex gap-2">
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <Button type="submit" size="sm" disabled={submitting}>
              {milestone ? "Save changes" : "Create milestone"}
            </Button>
          )}
        </form.Subscribe>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() =>
            void (milestone
              ? navigate({
                  to: "/$owner/$repo/milestones/$number",
                  params: { ...repo, number: milestone.number },
                })
              : navigate({ to: "/$owner/$repo/milestones", params: repo }))
          }
        >
          Cancel
        </Button>
      </div>
    </form>
  );
}
