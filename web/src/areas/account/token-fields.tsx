import { useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import { Calendar03Icon, Cancel01Icon, Search01Icon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import {
  Autocomplete,
  AutocompleteContent,
  AutocompleteEmpty,
  AutocompleteInput,
  AutocompleteItem,
  AutocompleteList,
} from "@/components/reui/autocomplete";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Calendar } from "@/components/ui/calendar";
import { Checkbox } from "@/components/ui/checkbox";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  GROUP_LABELS,
  metadataLocked,
  PERMISSIONS,
  SCOPES,
  setPermission,
  type PermissionGroup,
} from "./permissions";
import type { Access, AppPermissions } from "./ui-api";

// The pieces of the token forms that the App form reuses: expiry, the permission matrix,
// the repository picker, and the classic scope tree.

export const EXPIRY_DAYS = [7, 30, 60, 90] as const;
export type ExpiryChoice = `${(typeof EXPIRY_DAYS)[number]}` | "custom" | "none";

// The ISO expiry for a choice: midnight UTC at the end of the chosen day, as GitHub sets it.
export function expiryFor(choice: ExpiryChoice, custom: Date | undefined): string | null {
  if (choice === "none") return null;
  const day = choice === "custom" ? custom : new Date(Date.now() + Number(choice) * 86_400_000);
  if (!day) return null;
  return new Date(
    Date.UTC(day.getFullYear(), day.getMonth(), day.getDate(), 23, 59, 59),
  ).toISOString();
}

export const expiryText = (iso: string | null) =>
  iso
    ? `Expires on ${new Date(iso).toLocaleDateString("en", { weekday: "short", year: "numeric", month: "short", day: "numeric" })}`
    : "Never expires";

export function ExpiryPicker({
  choice,
  custom,
  onChoice,
  onCustom,
  allowNone = true,
}: {
  choice: ExpiryChoice;
  custom: Date | undefined;
  onChoice: (c: ExpiryChoice) => void;
  onCustom: (d: Date | undefined) => void;
  allowNone?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const tomorrow = new Date(Date.now() + 86_400_000);
  const max = new Date(Date.now() + 366 * 86_400_000);
  return (
    <div className="flex flex-wrap items-center gap-2" data-testid="expiry">
      <NativeSelect
        aria-label="Expiration"
        value={choice}
        onChange={(e) => {
          const c = e.target.value as ExpiryChoice;
          onChoice(c);
          if (c === "custom") setOpen(true);
        }}
      >
        {EXPIRY_DAYS.map((d) => (
          <NativeSelectOption key={d} value={String(d)}>
            {d} days
          </NativeSelectOption>
        ))}
        <NativeSelectOption value="custom">Custom…</NativeSelectOption>
        {allowNone && <NativeSelectOption value="none">No expiration</NativeSelectOption>}
      </NativeSelect>
      {choice === "custom" && (
        <Popover open={open} onOpenChange={setOpen}>
          <PopoverTrigger
            render={
              <Button
                type="button"
                variant="outline"
                size="sm"
                aria-label="Pick an expiration date"
              >
                <HugeiconsIcon icon={Calendar03Icon} strokeWidth={2} data-icon="inline-start" />
                {custom
                  ? custom.toLocaleDateString("en", {
                      year: "numeric",
                      month: "short",
                      day: "numeric",
                    })
                  : "Pick a date"}
              </Button>
            }
          />
          <PopoverContent className="w-auto p-0" align="start">
            <Calendar
              mode="single"
              selected={custom}
              onSelect={(d) => {
                onCustom(d);
                setOpen(false);
              }}
              disabled={[{ before: tomorrow }, { after: max }]}
              startMonth={tomorrow}
              endMonth={max}
            />
          </PopoverContent>
        </Popover>
      )}
      <span className="text-muted-foreground text-xs" data-testid="expiry-text">
        {choice === "custom" && !custom
          ? "Pick a date within a year"
          : expiryText(expiryFor(choice, custom))}
      </span>
      {choice === "none" && (
        <Badge variant="warning-light">Tokens that never expire are a risk if they leak</Badge>
      )}
    </div>
  );
}

export function PermissionMatrix({
  value,
  onChange,
  groups,
}: {
  value: AppPermissions;
  onChange: (p: AppPermissions) => void;
  groups: PermissionGroup[];
}) {
  const locked = metadataLocked(value);
  return (
    <div className="flex flex-col gap-3" data-testid="permission-matrix">
      {groups.map((g) => {
        const rows = PERMISSIONS.filter((p) => p.group === g);
        const set = rows.filter((p) => value[p.key]).length;
        return (
          <details key={g} open={g === "repository"} className="border">
            <summary className="bg-muted/40 flex cursor-pointer items-center gap-2 px-3 py-2 text-sm font-medium">
              {GROUP_LABELS[g]}
              <Badge variant="outline" size="sm" className="ml-auto">
                {set} selected
              </Badge>
            </summary>
            <ul className="divide-y border-t">
              {rows.map((p) => {
                const level = value[p.key] ?? "none";
                const fixed = p.key === "metadata" && locked;
                return (
                  <li
                    key={p.key}
                    className="flex items-center gap-3 px-3 py-2"
                    data-permission={p.key}
                  >
                    <div className="min-w-0 flex-1">
                      <p className="text-sm font-medium">{p.label}</p>
                      <p className="text-muted-foreground text-xs">{p.hint}</p>
                    </div>
                    <NativeSelect
                      size="sm"
                      aria-label={`${GROUP_LABELS[g]}: ${p.label}`}
                      value={level}
                      disabled={fixed}
                      onChange={(e) =>
                        onChange(setPermission(value, p.key, e.target.value as Access | "none"))
                      }
                    >
                      <NativeSelectOption value="none">No access</NativeSelectOption>
                      {p.levels.map((l) => (
                        <NativeSelectOption key={l} value={l}>
                          {l === "read" ? "Read-only" : l === "write" ? "Read and write" : "Admin"}
                        </NativeSelectOption>
                      ))}
                    </NativeSelect>
                  </li>
                );
              })}
            </ul>
          </details>
        );
      })}
      {locked && (
        <p className="text-muted-foreground text-xs">
          Metadata read-only is required by every other repository permission.
        </p>
      )}
    </div>
  );
}

type Repo = Pick<
  Schemas["repository"],
  "id" | "name" | "full_name" | "description" | "private" | "visibility"
>;

// Selected repositories as chips, and an autocomplete over the rest.
export function RepoPicker({
  repos,
  selected,
  onChange,
  label = "Select repositories",
}: {
  repos: Repo[];
  selected: string[];
  onChange: (names: string[]) => void;
  label?: string;
}) {
  const [query, setQuery] = useState("");
  const needle = query.trim().toLowerCase();
  const left = repos.filter(
    (r) => !selected.includes(r.full_name) && (!needle || r.name.toLowerCase().includes(needle)),
  );
  return (
    <div className="flex flex-col gap-2" data-testid="repo-picker">
      <Autocomplete
        items={left}
        filter={null}
        value={query}
        onValueChange={(v, details) => {
          if (details.reason === "item-press") {
            const repo = repos.find((r) => r.full_name === v);
            if (repo) onChange([...selected, repo.full_name]);
            setQuery("");
            return;
          }
          setQuery(v);
        }}
        itemToStringValue={(r: Repo) => r.full_name}
        openOnInputClick
      >
        <div className="relative max-w-md">
          <HugeiconsIcon
            icon={Search01Icon}
            strokeWidth={2}
            className="text-muted-foreground pointer-events-none absolute top-1/2 left-2.5 z-10 size-3.5 -translate-y-1/2"
          />
          <AutocompleteInput placeholder={label} aria-label={label} className="pl-8" />
        </div>
        <AutocompleteContent>
          <AutocompleteEmpty>No repositories match.</AutocompleteEmpty>
          <AutocompleteList>
            {(r: Repo) => (
              <AutocompleteItem
                key={r.id}
                value={r}
                className="rounded-sm data-highlighted:before:rounded-sm"
              >
                <span className="flex min-w-0 flex-col">
                  <span className="font-medium">{r.full_name}</span>
                  {r.description && (
                    <span className="text-muted-foreground truncate text-xs">{r.description}</span>
                  )}
                </span>
                <Badge variant="outline" size="sm" className="ml-auto">
                  {r.visibility ?? (r.private ? "private" : "public")}
                </Badge>
              </AutocompleteItem>
            )}
          </AutocompleteList>
        </AutocompleteContent>
      </Autocomplete>
      {selected.length > 0 ? (
        <ul className="flex flex-wrap gap-1.5" data-testid="selected-repos">
          {selected.map((name) => (
            <li key={name}>
              <Badge variant="secondary" size="lg" className="gap-1">
                {name}
                <button
                  type="button"
                  aria-label={`Remove ${name}`}
                  onClick={() => onChange(selected.filter((n) => n !== name))}
                >
                  <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} className="size-3" />
                </button>
              </Badge>
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-muted-foreground text-xs">No repositories selected.</p>
      )}
    </div>
  );
}

// Checking a parent selects every child; a child stays editable only while its parent is off.
export function ScopeTree({
  value,
  onChange,
}: {
  value: string[];
  onChange: (s: string[]) => void;
}) {
  const has = new Set(value);
  const toggle = (name: string, on: boolean, children: string[] = []) => {
    const next = new Set(value);
    for (const n of [name, ...children]) {
      if (on) next.add(n);
      else next.delete(n);
    }
    onChange([...next]);
  };
  return (
    <ul className="divide-y border" data-testid="scope-tree">
      {SCOPES.map((s) => {
        const kids = (s.children ?? []).map((c) => c.name);
        return (
          <li key={s.name} className="px-3 py-1.5">
            <label className="flex items-center gap-2 text-sm">
              <Checkbox
                checked={has.has(s.name)}
                onCheckedChange={(on) => toggle(s.name, on, kids)}
                aria-label={s.name}
              />
              <code className="font-mono text-xs font-semibold">{s.name}</code>
              <span className="text-muted-foreground text-xs">{s.description}</span>
            </label>
            {s.children && (
              <ul className="mt-1 ml-6 flex flex-col gap-1">
                {s.children.map((c) => (
                  <li key={c.name}>
                    <label className="flex items-center gap-2 text-sm">
                      <Checkbox
                        checked={has.has(c.name) || has.has(s.name)}
                        disabled={has.has(s.name)}
                        onCheckedChange={(on) => toggle(c.name, on)}
                        aria-label={c.name}
                      />
                      <code className="font-mono text-xs">{c.name}</code>
                      <span className="text-muted-foreground text-xs">{c.description}</span>
                    </label>
                  </li>
                ))}
              </ul>
            )}
          </li>
        );
      })}
    </ul>
  );
}

// What a classic token stores: a parent scope stands for its children.
export function normalizeScopes(scopes: string[]) {
  const set = new Set(scopes);
  for (const s of SCOPES) if (set.has(s.name)) for (const c of s.children ?? []) set.delete(c.name);
  return [...set].sort();
}
