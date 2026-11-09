import { useState } from "react";
import { useForm } from "@tanstack/react-form";
import { useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Add01Icon,
  ComputerIcon,
  Delete02Icon,
  PencilEdit02Icon,
} from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { Panel } from "@/components/panel";
import { Badge } from "@/components/reui/badge";
import { CodeBlock } from "@/components/reui/code-block/code-block";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Ago } from "@/components/github";
import { runnersQuery, scopeKey, type Scope } from "./scope";
import { send, useWrite } from "./write";

type Runner = Schemas["runner"];
type RunnersData = { total_count: number; runners: Runner[] };

const HOST = "https://ghes.graftyard.test";

function scopeUrl(scope: Scope) {
  return scope.kind === "repo" ? `${HOST}/${scope.owner}/${scope.repo}` : `${HOST}/${scope.org}`;
}

export function RunnerList({
  scope,
  groups,
}: {
  scope: Scope;
  groups?: { id: number; name: string }[];
}) {
  const query = runnersQuery(scope);
  const { data } = useSuspenseQuery(query);
  const queryClient = useQueryClient();
  const [adding, setAdding] = useState(false);
  const [labelling, setLabelling] = useState<Runner | null>(null);
  const [removing, setRemoving] = useState<Runner | null>(null);

  const remove = useWrite({
    run: (r: Runner) =>
      send(
        scope.kind === "repo"
          ? api.DELETE("/repos/{owner}/{repo}/actions/runners/{runner_id}", {
              params: { path: { owner: scope.owner, repo: scope.repo, runner_id: r.id } },
            })
          : api.DELETE("/orgs/{org}/actions/runners/{runner_id}", {
              params: { path: { org: scope.org, runner_id: r.id } },
            }),
      ),
    invalidates: () => [query.queryKey],
    optimistic: (r) => {
      const before = queryClient.getQueryData<RunnersData>(query.queryKey);
      queryClient.setQueryData<RunnersData>(
        query.queryKey,
        (d) =>
          d && { total_count: d.total_count - 1, runners: d.runners.filter((x) => x.id !== r.id) },
      );
      return () => queryClient.setQueryData(query.queryKey, before);
    },
    success: (r) => `Removed ${r.name}`,
    failure: (r) => `Could not remove ${r.name}`,
  });

  return (
    <Panel
      title={`Self-hosted runners (${data.total_count})`}
      flush
      actions={
        <Button size="sm" onClick={() => setAdding(true)}>
          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
          New runner
        </Button>
      }
    >
      {data.runners.length === 0 ? (
        <p
          className="text-muted-foreground px-4 py-6 text-center text-sm"
          data-testid="runners-empty"
        >
          No self-hosted runners yet. Add one to run jobs on your own machines.
        </p>
      ) : (
        <ul className="divide-y" data-testid="runners">
          {data.runners.map((r) => (
            <li
              key={r.id}
              className="flex flex-wrap items-center gap-3 px-4 py-2.5 text-sm"
              data-runner={r.name}
            >
              <HugeiconsIcon
                icon={ComputerIcon}
                strokeWidth={2}
                className="text-muted-foreground size-4"
              />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="font-medium">{r.name}</span>
                  <span className="text-muted-foreground text-xs">
                    {r.os} · v{r.version}
                  </span>
                  {groups && (
                    <Badge variant="outline" size="sm">
                      {groups.find((g) => g.id === r.runner_group_id)?.name ?? "Default"}
                    </Badge>
                  )}
                </div>
                <div className="mt-1 flex flex-wrap gap-1">
                  {r.labels.map((l) => (
                    <Badge
                      key={l.name}
                      variant={l.type === "custom" ? "info-light" : "outline"}
                      size="sm"
                      className="font-mono"
                    >
                      {l.name}
                    </Badge>
                  ))}
                </div>
              </div>
              <RunnerStatus runner={r} />
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Edit labels of ${r.name}`}
                onClick={() => setLabelling(r)}
              >
                <HugeiconsIcon icon={PencilEdit02Icon} strokeWidth={2} />
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Remove ${r.name}`}
                onClick={() => setRemoving(r)}
              >
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
              </Button>
            </li>
          ))}
        </ul>
      )}
      {adding && <NewRunner scope={scope} onClose={() => setAdding(false)} />}
      {labelling && (
        <EditLabels scope={scope} runner={labelling} onClose={() => setLabelling(null)} />
      )}
      <Dialog open={removing !== null} onOpenChange={(o) => !o && setRemoving(null)}>
        <DialogContent className="rounded-sm">
          <DialogHeader>
            <DialogTitle>Remove {removing?.name}?</DialogTitle>
            <DialogDescription>
              The runner stops taking jobs from this{" "}
              {scope.kind === "repo" ? "repository" : "organization"}. Run{" "}
              <code className="font-mono">./config.sh remove</code> on the machine to clean it up
              there too.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="rounded-b-sm">
            <Button variant="outline" onClick={() => setRemoving(null)}>
              Cancel
            </Button>
            <Button
              variant="destructive"
              onClick={() => {
                if (removing) remove.mutate(removing);
                setRemoving(null);
              }}
            >
              Remove runner
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </Panel>
  );
}

function RunnerStatus({ runner }: { runner: Runner }) {
  if (runner.status !== "online")
    return (
      <Badge variant="outline" data-runner-status="offline">
        Offline
      </Badge>
    );
  return runner.busy ? (
    <Badge variant="warning-light" data-runner-status="active">
      Active
    </Badge>
  ) : (
    <Badge variant="success-light" data-runner-status="idle">
      Idle
    </Badge>
  );
}

const OSES = [
  ["linux", "x64"],
  ["linux", "arm64"],
  ["osx", "arm64"],
  ["win", "x64"],
] as const;

// GHES's "New self-hosted runner": a registration token from the API and the commands to
// download, configure, and start the runner with it.
function NewRunner({ scope, onClose }: { scope: Scope; onClose: () => void }) {
  const [os, setOs] = useState<(typeof OSES)[number]>(OSES[0]);
  const token = useWrite({
    run: () =>
      send(
        scope.kind === "repo"
          ? api.POST("/repos/{owner}/{repo}/actions/runners/registration-token", {
              params: { path: { owner: scope.owner, repo: scope.repo } },
            })
          : api.POST("/orgs/{org}/actions/runners/registration-token", {
              params: { path: { org: scope.org } },
            }),
      ),
    invalidates: () => [],
    failure: "Could not create a registration token",
  });
  const file = `actions-runner-${os[0]}-${os[1]}-2.329.0.${os[0] === "win" ? "zip" : "tar.gz"}`;
  const commands = token.data
    ? os[0] === "win"
      ? `mkdir actions-runner; cd actions-runner
Invoke-WebRequest -Uri ${HOST}/_runner/v2.329.0/${file} -OutFile ${file}
Add-Type -AssemblyName System.IO.Compression.FileSystem ; [System.IO.Compression.ZipFile]::ExtractToDirectory("$PWD/${file}", "$PWD")
./config.cmd --url ${scopeUrl(scope)} --token ${token.data.token}
./run.cmd`
      : `mkdir actions-runner && cd actions-runner
curl -o ${file} -L ${HOST}/_runner/v2.329.0/${file}
tar xzf ./${file}
./config.sh --url ${scopeUrl(scope)} --token ${token.data.token}
./run.sh`
    : "";
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="rounded-sm sm:max-w-2xl [&>*]:min-w-0">
        <DialogHeader>
          <DialogTitle>New self-hosted runner</DialogTitle>
          <DialogDescription>
            Pick the machine's platform, then create a registration token. It is valid for one hour.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-wrap items-end gap-3">
          <Field className="w-48">
            <FieldLabel htmlFor="runner-os">Runner image</FieldLabel>
            <NativeSelect
              id="runner-os"
              size="sm"
              className="w-full"
              value={os.join("-")}
              onChange={(e) => setOs(OSES.find((o) => o.join("-") === e.target.value) ?? OSES[0])}
            >
              {OSES.map((o) => (
                <NativeSelectOption key={o.join("-")} value={o.join("-")}>
                  {o[0] === "osx" ? "macOS" : o[0] === "win" ? "Windows" : "Linux"} {o[1]}
                </NativeSelectOption>
              ))}
            </NativeSelect>
          </Field>
          <Button size="sm" onClick={() => token.mutate(undefined)} disabled={token.isPending}>
            {token.data ? "Create a new token" : "Create registration token"}
          </Button>
        </div>
        {token.data && (
          <div className="flex min-w-0 flex-col gap-2" data-testid="runner-token">
            <p className="text-sm">
              Token{" "}
              <code
                className="bg-muted rounded-sm px-1 font-mono text-xs"
                data-testid="token-value"
              >
                {token.data.token}
              </code>{" "}
              <span className="text-muted-foreground text-xs">
                expires <Ago at={token.data.expires_at} />
              </span>
            </p>
            <CodeBlock
              code={commands}
              language={os[0] === "win" ? "powershell" : "bash"}
              label="Download, configure, and run"
            />
            <FieldDescription className="text-xs">
              The runner shows up in the list once <code>config.sh</code> finishes.
            </FieldDescription>
          </div>
        )}
        <DialogFooter className="rounded-b-sm" showCloseButton />
      </DialogContent>
    </Dialog>
  );
}

function EditLabels({
  scope,
  runner,
  onClose,
}: {
  scope: Scope;
  runner: Runner;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const query = runnersQuery(scope);
  const readOnly = runner.labels.filter((l) => l.type !== "custom");
  const save = useWrite({
    run: (labels: string[]) =>
      send(
        scope.kind === "repo"
          ? api.PUT("/repos/{owner}/{repo}/actions/runners/{runner_id}/labels", {
              params: { path: { owner: scope.owner, repo: scope.repo, runner_id: runner.id } },
              body: { labels },
            })
          : api.PUT("/orgs/{org}/actions/runners/{runner_id}/labels", {
              params: { path: { org: scope.org, runner_id: runner.id } },
              body: { labels },
            }),
      ),
    invalidates: () => [query.queryKey, [...scopeKey(scope)]],
    optimistic: (labels) => {
      const before = queryClient.getQueryData<RunnersData>(query.queryKey);
      queryClient.setQueryData<RunnersData>(
        query.queryKey,
        (d) =>
          d && {
            ...d,
            runners: d.runners.map((r) =>
              r.id === runner.id
                ? {
                    ...r,
                    labels: [
                      ...readOnly,
                      ...labels.map((name) => ({ name, type: "custom" as const })),
                    ],
                  }
                : r,
            ),
          },
      );
      return () => queryClient.setQueryData(query.queryKey, before);
    },
    success: () => `Labels of ${runner.name} saved`,
    failure: `Could not save the labels of ${runner.name}`,
  });
  const form = useForm({
    defaultValues: {
      labels: runner.labels
        .filter((l) => l.type === "custom")
        .map((l) => l.name)
        .join(", "),
    },
    onSubmit: async ({ value }) => {
      const labels = value.labels
        .split(",")
        .map((l) => l.trim())
        .filter(Boolean);
      await save.mutateAsync(labels).then(onClose, () => undefined);
    },
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="rounded-sm">
        <DialogHeader>
          <DialogTitle>Labels of {runner.name}</DialogTitle>
          <DialogDescription>
            A job runs here when every label in its <code>runs-on</code> is on this runner. These
            are set by the runner: {readOnly.map((l) => l.name).join(", ")}.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <form.Field
            name="labels"
            validators={{
              onChange: ({ value }) =>
                value.split(",").some((l) => /\s/.test(l.trim()))
                  ? "Labels may not contain spaces"
                  : undefined,
            }}
          >
            {(f) => {
              const error =
                f.state.meta.errors[0] ??
                (save.error && "fields" in save.error ? save.error.message : undefined);
              return (
                <Field data-invalid={Boolean(error) || undefined}>
                  <FieldLabel htmlFor="labels">Custom labels</FieldLabel>
                  <Input
                    id="labels"
                    className="rounded-sm font-mono"
                    value={f.state.value}
                    onChange={(e) => f.handleChange(e.target.value)}
                  />
                  <FieldDescription className="text-xs">
                    Comma separated, such as linux, x64, gpu.
                  </FieldDescription>
                  {error && <FieldError>{String(error)}</FieldError>}
                </Field>
              );
            }}
          </form.Field>
          <DialogFooter className="rounded-b-sm">
            <Button type="button" variant="outline" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" disabled={save.isPending}>
              Save labels
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
