import { useState } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { useDebouncedCallback } from "@tanstack/react-pacer";
import { HugeiconsIcon } from "@hugeicons/react";
import { Search01Icon } from "@hugeicons/core-free-icons";
import { AuditLog, phraseOf, type AuditSearch } from "@/areas/orgs/audit";
import { membersQuery } from "@/areas/orgs/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";

// GET /orgs/{org}/audit-log. Each filter is a typed search param, and together they build
// GHES's search phrase, so the server does the filtering.

const ACTIONS = [
  ["", "Every action"],
  ["git", "Git: clone, fetch, push"],
  ["team", "Teams"],
  ["org", "Organization"],
  ["hook", "Webhooks"],
  ["repo", "Repositories"],
  ["repository_ruleset", "Rulesets"],
  ["protected_branch", "Branch protection"],
  ["pre_receive_hook", "Pre-receive hooks"],
] as const;

const DAY = /^\d{4}-\d{2}-\d{2}$/;
const str = (v: unknown) => (typeof v === "string" && v ? v : undefined);

export const Route = createFileRoute("/orgs/$org/settings/audit-log")({
  validateSearch: (s: Record<string, unknown>): AuditSearch => ({
    actor: str(s.actor),
    action: str(s.action),
    since: str(s.since) && DAY.test(String(s.since)) ? String(s.since) : undefined,
    until: str(s.until) && DAY.test(String(s.until)) ? String(s.until) : undefined,
    q: str(s.q),
    include: s.include === "git" || s.include === "all" ? s.include : undefined,
  }),
  component: AuditPage,
});

function AuditPage() {
  const { org } = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const set = (patch: Partial<AuditSearch>) =>
    void navigate({ search: (prev) => ({ ...prev, ...patch }), replace: true });
  const [text, setText] = useState(search.q ?? "");
  const setQuery = useDebouncedCallback((q: string) => set({ q: q || undefined }), { wait: 300 });
  // git.* events are only in the git or all sources.
  const include = search.include ?? (search.action === "git" ? "all" : "web");
  const filters = { phrase: phraseOf(search), include };
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Audit log"
        description="Every action members took in the organization: who, what, when, and from where."
      />
      <div className="flex flex-wrap items-end gap-2" data-testid="audit-filters">
        <ActorSelect org={org} value={search.actor} onChange={(actor) => set({ actor })} />
        <NativeSelect
          size="sm"
          aria-label="Action"
          value={search.action ?? ""}
          onChange={(e) => set({ action: e.target.value || undefined })}
        >
          {ACTIONS.map(([v, label]) => (
            <NativeSelectOption key={v} value={v}>
              {label}
            </NativeSelectOption>
          ))}
        </NativeSelect>
        <NativeSelect
          size="sm"
          aria-label="Source"
          value={include}
          onChange={(e) =>
            set({
              include: e.target.value === "web" ? undefined : (e.target.value as "git" | "all"),
            })
          }
        >
          <NativeSelectOption value="web">Web events</NativeSelectOption>
          <NativeSelectOption value="git">Git events</NativeSelectOption>
          <NativeSelectOption value="all">All events</NativeSelectOption>
        </NativeSelect>
        <label className="flex flex-col gap-0.5 text-xs">
          From
          <Input
            type="date"
            className="h-7 w-36 text-xs"
            aria-label="From date"
            value={search.since ?? ""}
            onChange={(e) => set({ since: e.target.value || undefined })}
          />
        </label>
        <label className="flex flex-col gap-0.5 text-xs">
          To
          <Input
            type="date"
            className="h-7 w-36 text-xs"
            aria-label="To date"
            value={search.until ?? ""}
            onChange={(e) => set({ until: e.target.value || undefined })}
          />
        </label>
        <InputGroup className="max-w-xs flex-1">
          <InputGroupAddon>
            <HugeiconsIcon icon={Search01Icon} strokeWidth={2} />
          </InputGroupAddon>
          <InputGroupInput
            placeholder="repo:edge or a word"
            aria-label="Search the audit log"
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              setQuery(e.target.value);
            }}
          />
        </InputGroup>
        {Object.values(search).some(Boolean) && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              setText("");
              void navigate({ search: {}, replace: true });
            }}
          >
            Clear filters
          </Button>
        )}
      </div>
      <p className="text-muted-foreground font-mono text-xs" data-testid="audit-phrase">
        phrase: {filters.phrase || "(none)"} · include: {include}
      </p>
      <Panel flush>
        <Region name="audit log" rows={10}>
          <AuditLog org={org} filters={filters} />
        </Region>
      </Panel>
    </div>
  );
}

function ActorSelect({
  org,
  value,
  onChange,
}: {
  org: string;
  value: string | undefined;
  onChange: (v: string | undefined) => void;
}) {
  // The actor list is a convenience; if it fails, the select falls back to the current value.
  const { data } = useQuery(membersQuery(org));
  const logins = data?.map((m) => m.login) ?? (value ? [value] : []);
  return (
    <NativeSelect
      size="sm"
      aria-label="Actor"
      value={value ?? ""}
      onChange={(e) => onChange(e.target.value || undefined)}
    >
      <NativeSelectOption value="">Every actor</NativeSelectOption>
      {logins.map((l) => (
        <NativeSelectOption key={l} value={l}>
          {l}
        </NativeSelectOption>
      ))}
    </NativeSelect>
  );
}
