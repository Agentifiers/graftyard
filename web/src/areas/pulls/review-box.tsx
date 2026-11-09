import { useState } from "react";
import { useForm } from "@tanstack/react-form";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { ArrowDown01Icon } from "@hugeicons/core-free-icons";
import { Alert, AlertDescription } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Textarea } from "@/components/ui/textarea";
import { useDiscardReview, useSubmitReview, WriteError, type ReviewEvent } from "./mutations";
import { pullQuery, type Pull } from "./queries";
import type { ReviewModel } from "./review-data";

const EVENTS: { value: ReviewEvent; label: string; hint: string }[] = [
  {
    value: "COMMENT",
    label: "Comment",
    hint: "Submit general feedback without explicit approval.",
  },
  { value: "APPROVE", label: "Approve", hint: "Give your approval to merge these changes." },
  {
    value: "REQUEST_CHANGES",
    label: "Request changes",
    hint: "Submit feedback that must be addressed before merging.",
  },
];

// "Review changes": submits the viewer's pending review (or a fresh one) as a comment,
// an approval, or a change request. GHES refuses self-approval and an empty change
// request with a 422; those messages show in the form.
export function ReviewBox({ pull, model }: { pull: Pull; model: ReviewModel }) {
  const { data: pr } = useSuspenseQuery(pullQuery(pull));
  const submit = useSubmitReview(pull);
  const discard = useDiscardReview(pull);
  const [open, setOpen] = useState(false);
  const [failure, setFailure] = useState<string>();
  const own = pr.user.login === model.viewer.login;
  const form = useForm({
    defaultValues: { body: "", event: "COMMENT" as ReviewEvent },
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      try {
        await submit.mutateAsync({
          reviewId: model.pending?.id,
          event: value.event,
          body: value.body.trim(),
        });
        formApi.reset();
        setOpen(false);
      } catch (e) {
        setFailure(e instanceof WriteError ? e.message : String(e));
      }
    },
  });
  if (pr.state !== "open" && !model.pending) return null;
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        render={
          <Button
            size="sm"
            variant={model.pending ? "default" : "outline"}
            data-testid="review-changes"
          >
            {model.pending ? "Finish your review" : "Review changes"}
            {model.pending && (
              <Badge variant="secondary" size="sm">
                {model.pendingComments.length}
              </Badge>
            )}
            <HugeiconsIcon icon={ArrowDown01Icon} strokeWidth={2} data-icon="inline-end" />
          </Button>
        }
      />
      <PopoverContent align="end" className="w-96 rounded-sm">
        <form
          className="flex flex-col gap-2"
          data-testid="review-form"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <p className="text-sm font-semibold">Finish your review</p>
          <form.Field name="body">
            {(field) => (
              <Textarea
                aria-label="Review summary"
                className="min-h-24 rounded-sm"
                placeholder="Leave a comment"
                value={field.state.value}
                onChange={(e) => field.handleChange(e.target.value)}
              />
            )}
          </form.Field>
          <form.Field name="event">
            {(field) => (
              <RadioGroup
                value={field.state.value}
                onValueChange={(v) => field.handleChange(v as ReviewEvent)}
                aria-label="Review outcome"
              >
                {EVENTS.map((ev) => (
                  <label key={ev.value} className="flex items-start gap-2 text-sm">
                    <RadioGroupItem value={ev.value} className="mt-0.5" />
                    <span>
                      <span className="font-medium">{ev.label}</span>
                      {own && ev.value !== "COMMENT" && (
                        <span className="text-muted-foreground"> (your own pull request)</span>
                      )}
                      <span className="text-muted-foreground block text-xs">{ev.hint}</span>
                    </span>
                  </label>
                ))}
              </RadioGroup>
            )}
          </form.Field>
          {failure && (
            <Alert variant="destructive" data-testid="review-error">
              <AlertDescription>{failure}</AlertDescription>
            </Alert>
          )}
          <div className="flex items-center justify-between gap-2">
            {model.pending ? (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={discard.isPending}
                onClick={() =>
                  discard.mutate(model.pending!.id, { onSuccess: () => setOpen(false) })
                }
              >
                Discard review
              </Button>
            ) : (
              <span />
            )}
            <form.Subscribe selector={(s) => s.isSubmitting}>
              {(busy) => (
                <Button type="submit" size="sm" disabled={busy}>
                  Submit review
                </Button>
              )}
            </form.Subscribe>
          </div>
        </form>
      </PopoverContent>
    </Popover>
  );
}
