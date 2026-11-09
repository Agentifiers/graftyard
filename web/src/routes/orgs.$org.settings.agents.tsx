import { useState } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { call, GraftyardError, gy, type OrgAgents } from "@/areas/agents/api";
import { orgAgentsQuery } from "@/areas/agents/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Switch } from "@/components/ui/switch";

export const Route = createFileRoute("/orgs/$org/settings/agents")({
  loader: ({ context, params }) =>
    void context.queryClient.prefetchQuery(orgAgentsQuery(params.org)),
  component: () => (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Agents"
        description="Graftyard-run agents for every repository in this organization. Off until an owner turns them on."
      />
      <Region name="organization agent settings" rows={5}>
        <OrgAgentsForm />
      </Region>
    </div>
  ),
});

type Patch = Partial<Omit<OrgAgents, "workspace_org">>;

// Each control saves on change with PATCH, shows the new value at once, and rolls back
// visibly if the server refuses.
function OrgAgentsForm() {
  const { org } = Route.useParams();
  const client = useQueryClient();
  const key = orgAgentsQuery(org).queryKey;
  const { data } = useSuspenseQuery(orgAgentsQuery(org));
  const [refusal, setRefusal] = useState<string>();
  const patch = useMutation({
    mutationFn: (body: Patch) =>
      call(gy.PATCH("/orgs/{org}/agents", { params: { path: { org } }, body })),
    onMutate: async (body) => {
      setRefusal(undefined);
      await client.cancelQueries({ queryKey: key });
      const before = client.getQueryData(key);
      client.setQueryData<OrgAgents>(key, (old) => old && { ...old, ...body });
      return { before };
    },
    onError: (e, _v, ctx) => {
      if (ctx?.before) client.setQueryData(key, ctx.before);
      if (e instanceof GraftyardError && e.status === 422) setRefusal(e.message);
      else toast.error("Setting not saved", { description: e.message });
    },
    onSuccess: (_d, body) => toast.success(describe(body)),
    onSettled: () => client.invalidateQueries({ queryKey: key }),
  });

  return (
    <div className="flex flex-col gap-4">
      <Panel>
        <div className="flex items-start gap-3">
          <Switch
            id="agents-enabled"
            checked={data.agents_enabled}
            disabled={patch.isPending}
            onCheckedChange={(v) => patch.mutate({ agents_enabled: v })}
            data-agents-enabled={data.agents_enabled}
          />
          <div className="flex flex-col gap-0.5">
            <Label htmlFor="agents-enabled">Enable Graftyard-run agents</Label>
            <p className="text-muted-foreground text-xs">
              Lets writers create agent tasks from issues and prompts. While off, creating a task
              answers 422. Third-party agent Apps report their own work either way.
            </p>
          </div>
        </div>
      </Panel>

      <Panel title="Workspace">
        <RadioGroup
          value={data.workspace}
          onValueChange={(v) => patch.mutate({ workspace: v as OrgAgents["workspace"] })}
          disabled={patch.isPending}
        >
          <div className="flex items-start gap-2">
            <RadioGroupItem value="fork" id="ws-fork" className="mt-0.5" />
            <div className="flex flex-col gap-0.5">
              <Label htmlFor="ws-fork">Fork into the workspace organization</Label>
              <p className="text-muted-foreground text-xs">
                Agents push to a private fork in <code>{data.workspace_org}</code>. The App
                permission ceiling confines them.
              </p>
            </div>
          </div>
          <div className="flex items-start gap-2">
            <RadioGroupItem value="in_repo" id="ws-in-repo" className="mt-0.5" />
            <div className="flex flex-col gap-0.5">
              <Label htmlFor="ws-in-repo">In the repository</Label>
              <p className="text-muted-foreground text-xs">
                For organizations that forbid private forks. Agents push under their task prefix
                only.
              </p>
            </div>
          </div>
        </RadioGroup>
        <p className="text-muted-foreground mt-3 flex items-center gap-2 text-xs">
          Workspace organization
          <Badge variant="outline" className="font-mono" data-workspace-org>
            {data.workspace_org}
          </Badge>
          (read only)
        </p>
        {refusal && (
          <Alert variant="destructive" className="mt-3" data-org-refusal>
            <AlertTitle className="line-clamp-none">
              The workspace setting was not changed
            </AlertTitle>
            <AlertDescription>
              <p>{refusal}</p>
              <p>
                Create the <code>{data.workspace_org}</code> organization, allow private forks into
                it, and install graftyard-agent and graftyard-workspace there. Then choose fork mode
                again.
              </p>
            </AlertDescription>
          </Alert>
        )}
      </Panel>

      <Panel>
        <div className="flex items-start gap-3">
          <Switch
            id="code-mode"
            checked={data.code_mode}
            disabled={patch.isPending}
            onCheckedChange={(v) => patch.mutate({ code_mode: v })}
          />
          <div className="flex flex-col gap-0.5">
            <Label htmlFor="code-mode">Code Mode for MCP</Label>
            <p className="text-muted-foreground text-xs">
              Lets MCP clients run code against this organization's repositories at{" "}
              <code>mcp.&lt;host&gt;/codemode</code>. Turning it off refuses new calls within a
              minute.
            </p>
          </div>
        </div>
      </Panel>
    </div>
  );
}

function describe(body: Patch) {
  if (body.agents_enabled !== undefined)
    return body.agents_enabled ? "Agents enabled" : "Agents turned off";
  if (body.workspace !== undefined)
    return body.workspace === "fork" ? "Agents now work in forks" : "Agents now work in-repo";
  return body.code_mode ? "Code Mode on" : "Code Mode off";
}
