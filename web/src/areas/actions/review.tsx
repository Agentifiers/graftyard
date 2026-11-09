import { useState } from "react";
import { useForm } from "@tanstack/react-form";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { HourglassIcon } from "@hugeicons/core-free-icons";
import { api } from "@/api/client";
import { Ago, UserAvatar } from "@/components/github";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldError, FieldLabel } from "@/components/ui/field";
import { Textarea } from "@/components/ui/textarea";
import { approvalsQuery, pendingDeploymentsQuery } from "./queries";
import { fieldError, send, useWrite } from "./write";

type Ref = { owner: string; repo: string; runId: number };

// A job waiting on an environment's required reviewers: "Review deployments".
export function PendingDeployments(ref: Ref) {
  const { data: pending } = useSuspenseQuery(pendingDeploymentsQuery(ref));
  const [open, setOpen] = useState(false);
  if (pending.length === 0) return <ReviewHistory {...ref} />;
  const canApprove = pending.some((p) => p.current_user_can_approve);
  return (
    <>
      <Alert variant="warning" data-testid="pending-deployments">
        <HugeiconsIcon icon={HourglassIcon} strokeWidth={2} />
        <AlertTitle>
          Waiting for review: {pending.map((p) => p.environment.name).join(", ")} needs approval to
          start deploying changes
        </AlertTitle>
        <AlertDescription>
          <span>
            Reviewers:{" "}
            {[
              ...new Set(
                pending.flatMap((p) =>
                  p.reviewers.map((r) =>
                    r.reviewer && "login" in r.reviewer ? r.reviewer.login : r.reviewer?.name,
                  ),
                ),
              ),
            ].join(", ")}
            . Waiting since{" "}
            <Ago at={pending[0].wait_timer_started_at ?? new Date().toISOString()} />.
          </span>
          <Button
            size="sm"
            className="mt-1 w-fit"
            disabled={!canApprove}
            onClick={() => setOpen(true)}
          >
            Review deployments
          </Button>
        </AlertDescription>
      </Alert>
      <ReviewDialog open={open} onOpenChange={setOpen} {...ref} />
      <ReviewHistory {...ref} />
    </>
  );
}

function ReviewDialog({
  open,
  onOpenChange,
  ...ref
}: Ref & { open: boolean; onOpenChange: (o: boolean) => void }) {
  const { data: pending } = useSuspenseQuery(pendingDeploymentsQuery(ref));
  const key = pendingDeploymentsQuery(ref).queryKey;
  const review = useWrite({
    run: (v: { environment_ids: number[]; state: "approved" | "rejected"; comment: string }) =>
      send(
        api.POST("/repos/{owner}/{repo}/actions/runs/{run_id}/pending_deployments", {
          params: { path: { owner: ref.owner, repo: ref.repo, run_id: ref.runId } },
          body: v,
        }),
      ),
    invalidates: () => [
      key,
      ["repos", ref.owner, ref.repo, "actions", "runs"],
      approvalsQuery(ref).queryKey,
    ],
    // No optimistic removal: emptying the pending list unmounts this dialog, and a 422's
    // field errors would go with it.
    success: (v) => (v.state === "approved" ? "Deployment approved" : "Deployment rejected"),
    failure: "Could not review the deployment",
  });
  const form = useForm({
    defaultValues: {
      environment_ids: pending
        .filter((p) => p.current_user_can_approve)
        .map((p) => p.environment.id ?? 0),
      comment: "",
      state: "approved" as "approved" | "rejected",
    },
    onSubmit: async ({ value }) => {
      await review.mutateAsync(value).then(
        () => onOpenChange(false),
        () => undefined,
      );
    },
  });
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="rounded-sm sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Review pending deployments</DialogTitle>
          <DialogDescription>
            Approving starts the waiting jobs. Rejecting fails them.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          data-testid="review-form"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <form.Field
            name="environment_ids"
            validators={{
              onSubmit: ({ value }) => (value.length ? undefined : "Choose an environment"),
            }}
          >
            {(field) => (
              <div className="flex flex-col gap-2">
                {pending.map((p) => {
                  const id = p.environment.id ?? 0;
                  return (
                    <Field key={id} orientation="horizontal">
                      <Checkbox
                        id={`env-${id}`}
                        disabled={!p.current_user_can_approve}
                        checked={field.state.value.includes(id)}
                        onCheckedChange={(on) =>
                          field.handleChange(
                            on
                              ? [...field.state.value, id]
                              : field.state.value.filter((x) => x !== id),
                          )
                        }
                      />
                      <FieldLabel htmlFor={`env-${id}`} className="font-mono">
                        {p.environment.name}
                      </FieldLabel>
                    </Field>
                  );
                })}
                {(field.state.meta.errors[0] ?? fieldError(review.error, "environment_ids")) && (
                  <FieldError>
                    {String(
                      field.state.meta.errors[0] ?? fieldError(review.error, "environment_ids"),
                    )}
                  </FieldError>
                )}
              </div>
            )}
          </form.Field>
          <form.Field name="comment">
            {(field) => (
              <Field data-invalid={Boolean(fieldError(review.error, "comment")) || undefined}>
                <FieldLabel htmlFor="review-comment">Comment</FieldLabel>
                <Textarea
                  id="review-comment"
                  className="rounded-sm"
                  placeholder="Leave a comment (required to reject)"
                  value={field.state.value}
                  onChange={(e) => field.handleChange(e.target.value)}
                />
                {fieldError(review.error, "comment") && (
                  <FieldError>{fieldError(review.error, "comment")}</FieldError>
                )}
              </Field>
            )}
          </form.Field>
          <DialogFooter className="rounded-b-sm">
            <form.Subscribe selector={(s) => s.isSubmitting}>
              {(submitting) => (
                <>
                  <Button
                    type="submit"
                    variant="outline"
                    disabled={submitting}
                    onClick={() => form.setFieldValue("state", "rejected")}
                  >
                    Reject
                  </Button>
                  <Button
                    type="submit"
                    disabled={submitting}
                    onClick={() => form.setFieldValue("state", "approved")}
                  >
                    Approve and deploy
                  </Button>
                </>
              )}
            </form.Subscribe>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function ReviewHistory(ref: Ref) {
  const { data } = useSuspenseQuery(approvalsQuery(ref));
  if (data.length === 0) return null;
  return (
    <ul className="flex flex-col gap-1 text-sm" data-testid="review-history">
      {data.map((a, i) => (
        <li key={i} className="flex flex-wrap items-center gap-2">
          <UserAvatar user={a.user} />
          <span className="font-medium">{a.user.login}</span>
          <Badge variant={a.state === "approved" ? "success-light" : "destructive-light"} size="sm">
            {a.state}
          </Badge>
          <span className="text-muted-foreground">
            {a.environments.map((e) => e.name).join(", ")}
          </span>
          {a.comment && <span className="text-muted-foreground truncate">“{a.comment}”</span>}
        </li>
      ))}
    </ul>
  );
}
