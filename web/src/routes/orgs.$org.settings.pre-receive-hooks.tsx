import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useSuspenseQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { api, type Schemas } from "@/api/client";
import { send } from "@/areas/orgs/api";
import { EmptyRow, failed, optimistic, refetch } from "@/areas/orgs/kit";
import { preReceiveQuery } from "@/areas/orgs/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Switch } from "@/components/ui/switch";

// The pre-receive hooks the site admin made available: set each one's enforcement for the
// org, let repositories override it, or reset to the instance default.
export const Route = createFileRoute("/orgs/$org/settings/pre-receive-hooks")({
  loader: ({ context: { queryClient }, params }) =>
    void queryClient.prefetchQuery(preReceiveQuery(params.org)),
  component: () => {
    const { org } = Route.useParams();
    return (
      <div className="flex flex-col gap-4">
        <PageHeader
          title="Pre-receive hooks"
          description="Scripts the site admin installed that accept or reject every push. Testing runs the script and only warns."
        />
        <Panel flush>
          <Region name="pre-receive hooks" rows={4}>
            <Hooks org={org} />
          </Region>
        </Panel>
      </div>
    );
  },
});

type Hook = Schemas["org-pre-receive-hook"];

function Hooks({ org }: { org: string }) {
  const { data } = useSuspenseQuery(preReceiveQuery(org));
  if (!data.length)
    return <EmptyRow>The site admin hasn't installed any pre-receive hooks.</EmptyRow>;
  return (
    <ul className="divide-y" data-testid="pre-receive-hooks">
      {data.map((h) => (
        <HookRow key={h.id} org={org} hook={h} />
      ))}
    </ul>
  );
}

function HookRow({ org, hook }: { org: string; hook: Hook }) {
  const key = preReceiveQuery(org).queryKey;
  // The org has set its own enforcement once the configuration URL points at the org.
  const own = hook.configuration_url?.includes(`/orgs/${org}/`) ?? false;
  const patch = useMutation({
    mutationFn: (body: { enforcement?: string; allow_downstream_configuration?: boolean }) =>
      send(
        api.PATCH("/orgs/{org}/pre-receive-hooks/{pre_receive_hook_id}", {
          params: { path: { org, pre_receive_hook_id: hook.id! } },
          body,
        }),
      ),
    onMutate: (body) =>
      optimistic<Hook[]>(key, (old) => old.map((h) => (h.id === hook.id ? { ...h, ...body } : h))),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed(`${hook.name} not changed`)(e);
    },
    onSuccess: (h) => toast.success(`${h.name}: ${h.enforcement}`),
    onSettled: () => refetch(key),
  });
  const reset = useMutation({
    mutationFn: () =>
      send(
        api.DELETE("/orgs/{org}/pre-receive-hooks/{pre_receive_hook_id}", {
          params: { path: { org, pre_receive_hook_id: hook.id! } },
        }),
      ),
    onError: failed(`${hook.name} not reset`),
    onSuccess: (h) => toast.success(`${h.name} reset to the instance default: ${h.enforcement}`),
    onSettled: () => refetch(key),
  });
  // block-secrets is enforced instance-wide, so the server refuses a change with a 422.
  const locked = hook.name === "block-secrets";
  return (
    <li className="flex flex-wrap items-center gap-3 px-4 py-3" data-pre-receive={hook.name}>
      <div className="min-w-0 flex-1">
        <p className="font-mono text-sm font-semibold">{hook.name}</p>
        <p className="text-muted-foreground text-xs">
          {locked
            ? "Enforced by the site admin for every organization."
            : own
              ? "Set by this organization."
              : "Using the instance default."}
        </p>
      </div>
      <Badge
        variant={
          hook.enforcement === "enabled"
            ? "success-light"
            : hook.enforcement === "testing"
              ? "warning-light"
              : "outline"
        }
        size="sm"
        className="capitalize"
        data-enforcement={hook.enforcement}
      >
        {hook.enforcement}
      </Badge>
      <NativeSelect
        size="sm"
        aria-label={`Enforcement for ${hook.name}`}
        value={hook.enforcement}
        disabled={patch.isPending}
        onChange={(e) => patch.mutate({ enforcement: e.target.value })}
      >
        <NativeSelectOption value="enabled">Enabled</NativeSelectOption>
        <NativeSelectOption value="testing">Testing</NativeSelectOption>
        <NativeSelectOption value="disabled">Disabled</NativeSelectOption>
      </NativeSelect>
      <label className="flex items-center gap-2 text-xs">
        <Switch
          size="sm"
          aria-label={`Repositories can override ${hook.name}`}
          checked={hook.allow_downstream_configuration ?? false}
          disabled={patch.isPending}
          onCheckedChange={(on) => patch.mutate({ allow_downstream_configuration: on })}
        />
        Repositories can override
      </label>
      <Button
        variant="ghost"
        size="sm"
        disabled={!own || reset.isPending}
        onClick={() => reset.mutate()}
      >
        Reset
      </Button>
    </li>
  );
}
