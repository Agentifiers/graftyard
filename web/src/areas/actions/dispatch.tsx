import { useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useForm } from "@tanstack/react-form";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { PlayIcon } from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import {
  Popover,
  PopoverContent,
  PopoverDescription,
  PopoverHeader,
  PopoverTitle,
  PopoverTrigger,
} from "@/components/ui/popover";
import { branchesQuery, environmentsQuery, workflowSourceQuery } from "./queries";
import type { DispatchInput } from "./ui-api";
import { fieldError, send, useWrite } from "./write";

type Repo = { owner: string; repo: string };
type Values = { ref: string; inputs: Record<string, string | boolean> };

// "Run workflow": a popover with the branch and one control per declared input.
export function RunWorkflow({ workflow, ...repo }: Repo & { workflow: Schemas["workflow"] }) {
  const [open, setOpen] = useState(false);
  const disabled = workflow.state !== "active";
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        render={
          <Button
            size="sm"
            disabled={disabled}
            title={disabled ? "Enable this workflow to run it" : undefined}
          >
            <HugeiconsIcon icon={PlayIcon} strokeWidth={2} data-icon="inline-start" />
            Run workflow
          </Button>
        }
      />
      <PopoverContent align="end" className="w-80">
        <PopoverHeader>
          <PopoverTitle>Run workflow</PopoverTitle>
          <PopoverDescription>
            Sends <code className="font-mono text-xs">workflow_dispatch</code> for {workflow.name}.
          </PopoverDescription>
        </PopoverHeader>
        <Region name="workflow inputs" rows={4}>
          <DispatchForm workflow={workflow} onDone={() => setOpen(false)} {...repo} />
        </Region>
      </PopoverContent>
    </Popover>
  );
}

function initial(input: DispatchInput): string | boolean {
  if (input.type === "boolean") return input.default ?? false;
  if (input.type === "choice") return input.default ?? input.options[0] ?? "";
  return input.default ?? "";
}

function DispatchForm({
  workflow,
  onDone,
  ...repo
}: Repo & { workflow: Schemas["workflow"]; onDone: () => void }) {
  const { data: source } = useSuspenseQuery(
    workflowSourceQuery({ ...repo, workflowId: workflow.id }),
  );
  const { data: branches } = useSuspenseQuery(branchesQuery(repo));
  const { data: envs } = useSuspenseQuery(environmentsQuery(repo));
  const navigate = useNavigate();
  const declared = source.dispatch;

  const dispatch = useWrite({
    run: (v: Values) =>
      send(
        api.POST("/repos/{owner}/{repo}/actions/workflows/{workflow_id}/dispatches", {
          params: { path: { ...repo, workflow_id: workflow.id } },
          body: { ref: v.ref, inputs: v.inputs, return_run_details: true },
        }),
      ),
    invalidates: () => [["repos", repo.owner, repo.repo, "actions", "runs"]],
    success: () => `${workflow.name} queued`,
    failure: `Could not run ${workflow.name}`,
  });

  const form = useForm({
    defaultValues: {
      ref: "main",
      inputs: Object.fromEntries((declared ?? []).map(({ name, input }) => [name, initial(input)])),
    } as Values,
    onSubmit: async ({ value }) => {
      const result = await dispatch.mutateAsync(value).catch(() => undefined);
      if (!result) return;
      onDone();
      if (result && "workflow_run_id" in result)
        void navigate({
          to: "/$owner/$repo/actions/runs/$runId",
          params: { ...repo, runId: result.workflow_run_id },
        });
    },
  });

  if (!declared) {
    return (
      <p className="text-muted-foreground text-sm" data-testid="no-dispatch">
        This workflow has no <code className="font-mono text-xs">workflow_dispatch</code> trigger.
      </p>
    );
  }

  const serverError = (name: string) => fieldError(dispatch.error, name);

  return (
    <form
      className="flex flex-col gap-3"
      data-testid="dispatch-form"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field name="ref">
        {(field) => (
          <Field data-invalid={Boolean(serverError("ref")) || undefined}>
            <FieldLabel htmlFor="dispatch-ref">Use workflow from</FieldLabel>
            <NativeSelect
              id="dispatch-ref"
              size="sm"
              className="w-full"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
            >
              {branches.map((b) => (
                <NativeSelectOption key={b.name} value={b.name}>
                  Branch: {b.name}
                </NativeSelectOption>
              ))}
            </NativeSelect>
            {serverError("ref") && <FieldError>{serverError("ref")}</FieldError>}
          </Field>
        )}
      </form.Field>
      {declared.map(({ name, input }) => (
        <form.Field
          key={name}
          name={`inputs.${name}`}
          validators={{
            onSubmit: ({ value }) =>
              input.required && input.type !== "boolean" && !String(value ?? "").trim()
                ? `${name} is required`
                : undefined,
          }}
        >
          {(field) => {
            const id = `dispatch-${name}`;
            const error = field.state.meta.errors[0] ?? serverError(`inputs.${name}`);
            const value = field.state.value;
            const label = (
              <FieldLabel htmlFor={id}>
                {input.description || name}
                {input.required && <span className="text-destructive">*</span>}
              </FieldLabel>
            );
            if (input.type === "boolean")
              return (
                <Field orientation="horizontal">
                  <Checkbox
                    id={id}
                    checked={value === true}
                    onCheckedChange={(on) => field.handleChange(on === true)}
                  />
                  {label}
                </Field>
              );
            return (
              <Field data-invalid={Boolean(error) || undefined}>
                {label}
                {input.type === "string" ? (
                  <Input
                    id={id}
                    className="h-7 rounded-sm"
                    value={String(value ?? "")}
                    aria-invalid={Boolean(error) || undefined}
                    onBlur={field.handleBlur}
                    onChange={(e) => field.handleChange(e.target.value)}
                  />
                ) : (
                  <NativeSelect
                    id={id}
                    size="sm"
                    className="w-full"
                    value={String(value ?? "")}
                    aria-invalid={Boolean(error) || undefined}
                    onChange={(e) => field.handleChange(e.target.value)}
                  >
                    {(input.type === "choice"
                      ? input.options
                      : (envs.environments?.map((e) => e.name) ?? [])
                    ).map((o) => (
                      <NativeSelectOption key={o} value={o}>
                        {o}
                      </NativeSelectOption>
                    ))}
                  </NativeSelect>
                )}
                <FieldDescription className="text-xs">
                  <code className="font-mono">{name}</code> · {input.type}
                </FieldDescription>
                {error && <FieldError>{String(error)}</FieldError>}
              </Field>
            );
          }}
        </form.Field>
      ))}
      {dispatch.error &&
        !fieldError(dispatch.error, "ref") &&
        !declared.some((d) => serverError(`inputs.${d.name}`)) && (
          <Alert variant="destructive">
            <AlertTitle>Not started</AlertTitle>
            <AlertDescription>{dispatch.error.message}</AlertDescription>
          </Alert>
        )}
      <form.Subscribe selector={(s) => s.isSubmitting}>
        {(submitting) => (
          <Button type="submit" size="sm" disabled={submitting}>
            {submitting ? "Starting…" : "Run workflow"}
          </Button>
        )}
      </form.Subscribe>
    </form>
  );
}
