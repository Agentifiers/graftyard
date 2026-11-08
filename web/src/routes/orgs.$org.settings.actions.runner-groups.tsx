import { useState } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useForm } from "@tanstack/react-form";
import { useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Add01Icon,
  Delete02Icon,
  PencilEdit02Icon,
  ServerStack01Icon,
} from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { runnerGroupReposQuery, runnerGroupsQuery } from "@/areas/actions/queries";
import { runnersQuery } from "@/areas/actions/scope";
import { RepoPicker } from "@/areas/actions/secrets";
import { fieldError, send, useWrite } from "@/areas/actions/write";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
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
import { Input } from "@/components/ui/input";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";

export const Route = createFileRoute("/orgs/$org/settings/actions/runner-groups")({
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(runnerGroupsQuery(params.org));
    void context.queryClient.prefetchQuery(runnersQuery({ kind: "org", org: params.org }));
  },
  component: RunnerGroupsPage,
});

type Group = Schemas["runner-groups-org"];

function RunnerGroupsPage() {
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Runner groups"
        description="Groups decide which repositories may send jobs to which organization runners."
      />
      <Region name="runner groups" rows={4}>
        <Groups />
      </Region>
    </div>
  );
}

function Groups() {
  const { org } = Route.useParams();
  const { data } = useSuspenseQuery(runnerGroupsQuery(org));
  const { data: runners } = useSuspenseQuery(runnersQuery({ kind: "org", org }));
  const [editing, setEditing] = useState<Group | "new" | null>(null);
  const queryClient = useQueryClient();
  const key = runnerGroupsQuery(org).queryKey;
  const remove = useWrite({
    run: (g: Group) =>
      send(
        api.DELETE("/orgs/{org}/actions/runner-groups/{runner_group_id}", {
          params: { path: { org, runner_group_id: g.id } },
        }),
      ),
    invalidates: () => [key, runnersQuery({ kind: "org", org }).queryKey],
    optimistic: (g) => {
      const before = queryClient.getQueryData(key);
      queryClient.setQueryData(
        key,
        (d) =>
          d && {
            total_count: d.total_count - 1,
            runner_groups: d.runner_groups.filter((x) => x.id !== g.id),
          },
      );
      return () => queryClient.setQueryData(key, before);
    },
    success: (g) => `Deleted ${g.name}; its runners moved to Default`,
    failure: (g) => `Could not delete ${g.name}`,
  });
  return (
    <Panel
      title={`${data.total_count} runner groups`}
      flush
      actions={
        <Button size="sm" onClick={() => setEditing("new")}>
          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
          New runner group
        </Button>
      }
    >
      <ul className="divide-y" data-testid="runner-groups">
        {data.runner_groups.map((g) => {
          const members = runners.runners.filter((r) => r.runner_group_id === g.id);
          return (
            <li
              key={g.id}
              className="flex items-center gap-3 px-4 py-2.5 text-sm"
              data-group={g.name}
            >
              <HugeiconsIcon
                icon={ServerStack01Icon}
                strokeWidth={2}
                className="text-muted-foreground size-4"
              />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="font-medium">{g.name}</span>
                  {g.default && (
                    <Badge variant="outline" size="sm">
                      Default
                    </Badge>
                  )}
                </div>
                <p className="text-muted-foreground text-xs">
                  {g.visibility === "all"
                    ? "All repositories"
                    : g.visibility === "private"
                      ? "Private repositories"
                      : "Selected repositories"}
                  {" · "}
                  {members.length} runner{members.length === 1 ? "" : "s"}
                  {members.length > 0 && `: ${members.map((r) => r.name).join(", ")}`}
                </p>
              </div>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Edit ${g.name}`}
                onClick={() => setEditing(g)}
              >
                <HugeiconsIcon icon={PencilEdit02Icon} strokeWidth={2} />
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Delete ${g.name}`}
                disabled={g.default}
                title={g.default ? "The default group cannot be deleted" : undefined}
                onClick={() => remove.mutate(g)}
              >
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
              </Button>
            </li>
          );
        })}
      </ul>
      {editing && (
        <Region name="runner group" fallback={null}>
          <GroupDialog
            group={editing === "new" ? undefined : editing}
            onClose={() => setEditing(null)}
          />
        </Region>
      )}
    </Panel>
  );
}

type Values = {
  name: string;
  visibility: "all" | "private" | "selected";
  repos: number[];
  runners: number[];
  allowsPublic: boolean;
};

function GroupDialog({ group, onClose }: { group?: Group; onClose: () => void }) {
  const { org } = Route.useParams();
  const { data: runners } = useSuspenseQuery(runnersQuery({ kind: "org", org }));
  const { data: repos } = useSuspenseQuery(
    group
      ? runnerGroupReposQuery(org, group.id)
      : { queryKey: ["no-group"], queryFn: () => ({ total_count: 0, repositories: [] }) },
  );
  const save = useWrite({
    run: async (v: Values) => {
      const selected = v.visibility === "selected" ? v.repos : undefined;
      if (!group)
        return send(
          api.POST("/orgs/{org}/actions/runner-groups", {
            params: { path: { org } },
            body: {
              name: v.name.trim(),
              visibility: v.visibility,
              selected_repository_ids: selected,
              runners: v.runners,
              allows_public_repositories: v.allowsPublic,
            },
          }),
        );
      const path = { org, runner_group_id: group.id };
      const updated = await send(
        api.PATCH("/orgs/{org}/actions/runner-groups/{runner_group_id}", {
          params: { path },
          body: {
            name: v.name.trim(),
            visibility: v.visibility,
            allows_public_repositories: v.allowsPublic,
          },
        }),
      );
      if (selected)
        await send(
          api.PUT("/orgs/{org}/actions/runner-groups/{runner_group_id}/repositories", {
            params: { path },
            body: { selected_repository_ids: selected },
          }),
        );
      await send(
        api.PUT("/orgs/{org}/actions/runner-groups/{runner_group_id}/runners", {
          params: { path },
          body: { runners: v.runners },
        }),
      );
      return updated;
    },
    invalidates: () => [
      runnerGroupsQuery(org).queryKey,
      runnersQuery({ kind: "org", org }).queryKey,
      ["orgs", org, "actions", "runner-groups"],
    ],
    success: (v) => `Runner group ${v.name.trim()} saved`,
    failure: (v) => `Could not save ${v.name.trim() || "the runner group"}`,
  });
  const form = useForm({
    defaultValues: {
      name: group?.name ?? "",
      visibility: (group?.visibility ?? "selected") as Values["visibility"],
      repos: repos.repositories.map((r) => r.id),
      runners: group
        ? runners.runners.filter((r) => r.runner_group_id === group.id).map((r) => r.id)
        : [],
      allowsPublic: group?.allows_public_repositories ?? false,
    } as Values,
    onSubmit: async ({ value }) => {
      await save.mutateAsync(value).then(onClose, () => undefined);
    },
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="rounded-sm sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{group ? `Edit ${group.name}` : "New runner group"}</DialogTitle>
          <DialogDescription>
            Runners belong to one group. Moving a runner here takes it out of its old group.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          data-testid="group-form"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <form.Field
            name="name"
            validators={{
              onSubmit: ({ value }) => (value.trim() ? undefined : "Name is required"),
            }}
          >
            {(f) => {
              const error = f.state.meta.errors[0] ?? fieldError(save.error, "name");
              return (
                <Field data-invalid={Boolean(error) || undefined}>
                  <FieldLabel htmlFor="group-name">Name</FieldLabel>
                  <Input
                    id="group-name"
                    className="rounded-sm"
                    value={f.state.value}
                    aria-invalid={Boolean(error) || undefined}
                    onChange={(e) => f.handleChange(e.target.value)}
                  />
                  {error && <FieldError>{String(error)}</FieldError>}
                </Field>
              );
            }}
          </form.Field>
          <form.Field name="visibility">
            {(f) => (
              <Field data-invalid={Boolean(fieldError(save.error, "visibility")) || undefined}>
                <FieldLabel>Repository access</FieldLabel>
                <RadioGroup
                  value={f.state.value}
                  onValueChange={(v) => f.handleChange(v as Values["visibility"])}
                >
                  {(["all", "private", "selected"] as const).map((v) => (
                    <Field key={v} orientation="horizontal">
                      <RadioGroupItem
                        value={v}
                        id={`gvis-${v}`}
                        disabled={group?.default && v !== "all"}
                      />
                      <FieldLabel htmlFor={`gvis-${v}`} className="font-normal">
                        {v === "all"
                          ? "All repositories"
                          : v === "private"
                            ? "Private repositories"
                            : "Selected repositories"}
                      </FieldLabel>
                    </Field>
                  ))}
                </RadioGroup>
                {fieldError(save.error, "visibility") && (
                  <FieldError>{fieldError(save.error, "visibility")}</FieldError>
                )}
              </Field>
            )}
          </form.Field>
          <form.Subscribe selector={(s) => s.values.visibility}>
            {(visibility) =>
              visibility === "selected" && (
                <form.Field name="repos">
                  {(f) => <RepoPicker org={org} value={f.state.value} onChange={f.handleChange} />}
                </form.Field>
              )
            }
          </form.Subscribe>
          <form.Field name="allowsPublic">
            {(f) => (
              <Field orientation="horizontal">
                <Checkbox
                  id="allow-public"
                  checked={f.state.value}
                  onCheckedChange={(on) => f.handleChange(on === true)}
                />
                <FieldLabel htmlFor="allow-public" className="font-normal">
                  Allow public repositories
                </FieldLabel>
              </Field>
            )}
          </form.Field>
          <form.Field name="runners">
            {(f) => (
              <Field>
                <FieldLabel>Runners</FieldLabel>
                <div
                  className="flex flex-col gap-1.5 rounded-sm border p-2"
                  data-testid="group-runners"
                >
                  {runners.runners.map((r) => (
                    <Field key={r.id} orientation="horizontal">
                      <Checkbox
                        id={`grunner-${r.id}`}
                        checked={f.state.value.includes(r.id)}
                        onCheckedChange={(on) =>
                          f.handleChange(
                            on ? [...f.state.value, r.id] : f.state.value.filter((x) => x !== r.id),
                          )
                        }
                      />
                      <FieldLabel htmlFor={`grunner-${r.id}`} className="font-normal">
                        {r.name}
                        <span className="text-muted-foreground text-xs">{r.status}</span>
                      </FieldLabel>
                    </Field>
                  ))}
                </div>
              </Field>
            )}
          </form.Field>
          <DialogFooter className="rounded-b-sm">
            <Button type="button" variant="outline" onClick={onClose}>
              Cancel
            </Button>
            <form.Subscribe selector={(s) => s.isSubmitting}>
              {(submitting) => (
                <Button type="submit" disabled={submitting}>
                  {group ? "Save group" : "Create group"}
                </Button>
              )}
            </form.Subscribe>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
