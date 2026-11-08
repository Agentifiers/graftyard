import { createFileRoute, Link } from "@tanstack/react-router";
import { useForm } from "@tanstack/react-form";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { call, GraftyardError, gy, type Policy } from "@/areas/agents/api";
import { globProblem, parseGlobs } from "@/areas/agents/globs";
import { policyQuery } from "@/areas/agents/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Textarea } from "@/components/ui/textarea";

export const Route = createFileRoute("/$owner/$repo/settings/agents")({
  loader: ({ context, params }) => void context.queryClient.prefetchQuery(policyQuery(params)),
  component: () => (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Agents"
        description="Rules the host enforces on agent Apps in this repository. People are not affected."
      />
      <Region name="agent policy" rows={6}>
        <PolicyForm />
      </Region>
    </div>
  ),
});

type Values = Omit<Policy, "labels" | "extra_paths"> & { labels: string; extra_paths: string };

const toValues = (p: Policy): Values => ({
  ...p,
  labels: p.labels.join(", "),
  extra_paths: p.extra_paths.join("\n"),
});
const toPolicy = (v: Values): Policy => ({
  ...v,
  labels: v.labels
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean),
  extra_paths: parseGlobs(v.extra_paths),
});

function PolicyForm() {
  const params = Route.useParams();
  const client = useQueryClient();
  const { data } = useSuspenseQuery(policyQuery(params));
  const key = policyQuery(params).queryKey;
  const save = useMutation({
    mutationFn: (body: Policy) =>
      call(gy.PUT("/repos/{owner}/{repo}/agents/policy", { params: { path: params }, body })),
    onMutate: async (next) => {
      await client.cancelQueries({ queryKey: key });
      const before = client.getQueryData(key);
      client.setQueryData(key, next);
      return { before };
    },
    onError: (e, _v, ctx) => {
      if (ctx?.before) client.setQueryData(key, ctx.before);
      toast.error("Policy not saved", { description: e.message });
    },
    onSuccess: () => toast.success("Agent policy saved"),
    onSettled: () => client.invalidateQueries({ queryKey: key }),
  });

  const form = useForm({
    defaultValues: toValues(data),
    onSubmit: async ({ value, formApi }) => {
      try {
        const saved = await save.mutateAsync(toPolicy(value));
        formApi.reset(toValues(saved));
      } catch (e) {
        if (!(e instanceof GraftyardError)) throw e;
        const fields = e.fieldErrors();
        for (const [f, msg] of Object.entries(fields))
          if (f in value)
            formApi.setFieldMeta(f as keyof Values, (m) => ({ ...m, errorMap: { onServer: msg } }));
        // The form keeps the edit, so the user can save again.
        formApi.setFieldValue("ci", value.ci);
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
      <Panel title="Workflows on agent pushes">
        <form.Field name="ci">
          {(field) => (
            <RadioGroup
              value={field.state.value}
              onValueChange={(v) => field.handleChange(v as Policy["ci"])}
            >
              <Choice value="approve" label="Require approval" id="ci-approve">
                Runs from agent pushes wait as <code>action_required</code> until a writer approves
                them, as on GitHub.
              </Choice>
              <Choice value="auto_read_only" label="Run read-only" id="ci-auto">
                Runs start at once with a read-only token and no secrets. Jobs that reference
                secrets wait for approval.
              </Choice>
            </RadioGroup>
          )}
        </form.Field>
      </Panel>

      <Panel title="Leases">
        <form.Field name="leases">
          {(field) => (
            <RadioGroup
              value={field.state.value}
              onValueChange={(v) => field.handleChange(v as Policy["leases"])}
            >
              <Choice value="advisory" label="Advisory" id="leases-advisory">
                Leases inform and never refuse a push.
              </Choice>
              <Choice value="agents_exclusive" label="Exclusive for agents" id="leases-exclusive">
                An agent push that touches another holder's live Exclusive lease is refused with a
                message naming the lease. People still push.
              </Choice>
            </RadioGroup>
          )}
        </form.Field>
      </Panel>

      <Panel title="Limits">
        <FieldSet>
          <FieldLegend variant="label" className="sr-only">
            Limits
          </FieldLegend>
          <form.Field
            name="labels"
            validators={{
              onChange: ({ value }) =>
                value.split(",").filter((s) => s.trim()).length > 64
                  ? "At most 64 labels."
                  : undefined,
            }}
          >
            {(field) => (
              <Field data-invalid={field.state.meta.errors.length > 0 || undefined}>
                <FieldLabel htmlFor="policy-labels">Labels agents may set</FieldLabel>
                <Input
                  id="policy-labels"
                  value={field.state.value}
                  onChange={(e) => field.handleChange(e.target.value)}
                />
                <FieldDescription>
                  Comma separated. Agents get 403 on any other label. Manage labels in{" "}
                  <Link to="/$owner/$repo/issues" params={params} className="underline">
                    Issues
                  </Link>
                  .
                </FieldDescription>
                <FieldError
                  errors={field.state.meta.errors.map((message) => ({ message: String(message) }))}
                />
              </Field>
            )}
          </form.Field>
          <form.Field
            name="extra_paths"
            validators={{
              onChange: ({ value }) => parseGlobs(value).map(globProblem).find(Boolean),
            }}
          >
            {(field) => (
              <Field data-invalid={field.state.meta.errors.length > 0 || undefined}>
                <FieldLabel htmlFor="policy-paths">Extra protected paths</FieldLabel>
                <Textarea
                  id="policy-paths"
                  className="min-h-20 font-mono text-xs"
                  value={field.state.value}
                  onChange={(e) => field.handleChange(e.target.value)}
                />
                <FieldDescription>
                  One glob per line. Added to the built-in list (workflows, CODEOWNERS, AGENTS.md,
                  and others). Agent pushes under them are refused.
                </FieldDescription>
                <FieldError
                  errors={field.state.meta.errors.map((message) => ({ message: String(message) }))}
                />
              </Field>
            )}
          </form.Field>
          <form.Field
            name="extra_human_approvals"
            validators={{
              onChange: ({ value }) =>
                Number.isInteger(value) && value >= 0 && value <= 10
                  ? undefined
                  : "A whole number from 0 to 10.",
            }}
          >
            {(field) => (
              <Field
                data-invalid={field.state.meta.errors.length > 0 || undefined}
                className="max-w-xs"
              >
                <FieldLabel htmlFor="policy-approvals">Extra human approvals</FieldLabel>
                <Input
                  id="policy-approvals"
                  type="number"
                  min={0}
                  max={10}
                  value={Number.isNaN(field.state.value) ? "" : field.state.value}
                  onChange={(e) => field.handleChange(e.target.valueAsNumber)}
                />
                <FieldDescription>
                  An agent PR merges after 1 + this many approvals from writers who are not the
                  agent, the requester, or a pusher to the PR.
                </FieldDescription>
                <FieldError
                  errors={field.state.meta.errors.map((message) => ({ message: String(message) }))}
                />
              </Field>
            )}
          </form.Field>
        </FieldSet>
      </Panel>

      <div className="flex justify-end gap-2">
        <form.Subscribe selector={(s) => [s.isDirty, s.isSubmitting] as const}>
          {([dirty, submitting]) => (
            <>
              <Button
                type="button"
                variant="outline"
                disabled={!dirty || submitting}
                onClick={() => form.reset()}
              >
                Discard
              </Button>
              <Button type="submit" disabled={!dirty || submitting} data-save-policy>
                {submitting ? "Saving…" : "Save policy"}
              </Button>
            </>
          )}
        </form.Subscribe>
      </div>
    </form>
  );
}

function Choice({
  value,
  label,
  id,
  children,
}: {
  value: string;
  label: string;
  id: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-start gap-2">
      <RadioGroupItem value={value} id={id} className="mt-0.5" />
      <div className="flex flex-col gap-0.5">
        <Label htmlFor={id}>{label}</Label>
        <p className="text-muted-foreground text-xs">{children}</p>
      </div>
    </div>
  );
}
