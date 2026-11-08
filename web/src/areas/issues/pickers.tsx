import { useState, type ReactNode } from "react";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { Settings02Icon, Tick02Icon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { assigneesQuery, labelsQuery, milestonesQuery } from "./queries";
import { UserAvatar } from "@/components/github";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "cn";

type Repo = { owner: string; repo: string };

// One picker: a gear button that opens a filterable list. A multi picker toggles items and
// reports the set when the popover closes, as GHES's sidebar does; a single picker reports
// the pick at once.
function Picker<T>({
  title,
  items,
  keyOf,
  text,
  render,
  selected,
  onCommit,
  multiple,
  trigger,
}: {
  title: string;
  items: T[];
  keyOf: (t: T) => string;
  text: (t: T) => string;
  render: (t: T) => ReactNode;
  selected: string[];
  onCommit: (keys: string[]) => void;
  multiple: boolean;
  trigger?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const [draft, setDraft] = useState<string[]>(selected);
  const shown = items.filter((t) => text(t).toLowerCase().includes(q.toLowerCase()));
  const same = (a: string[], b: string[]) => a.length === b.length && a.every((x) => b.includes(x));
  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        if (next) {
          setDraft(selected);
          setQ("");
        } else if (multiple && !same(draft, selected)) {
          onCommit(draft);
        }
        setOpen(next);
      }}
    >
      <PopoverTrigger
        render={
          trigger ? (
            (trigger as React.ReactElement)
          ) : (
            <Button variant="ghost" size="icon-xs" aria-label={`Edit ${title.toLowerCase()}`}>
              <HugeiconsIcon icon={Settings02Icon} strokeWidth={2} />
            </Button>
          )
        }
      />
      <PopoverContent
        align="end"
        className="w-64 gap-1 rounded-sm p-1"
        data-testid={`picker-${title.toLowerCase()}`}
      >
        <p className="px-2 pt-1 text-xs font-semibold">{title}</p>
        <Input
          autoFocus
          value={q}
          placeholder="Filter"
          aria-label={`Filter ${title.toLowerCase()}`}
          className="h-7 rounded-sm text-xs"
          onChange={(e) => setQ(e.target.value)}
        />
        <ul role="listbox" aria-multiselectable={multiple} className="max-h-64 overflow-y-auto">
          {shown.length === 0 && (
            <li className="text-muted-foreground px-2 py-3 text-xs">Nothing matches.</li>
          )}
          {shown.map((t) => {
            const k = keyOf(t);
            const on = draft.includes(k);
            return (
              <li key={k}>
                <button
                  type="button"
                  role="option"
                  aria-selected={on}
                  className="hover:bg-muted flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm"
                  onClick={() => {
                    if (multiple) {
                      setDraft(on ? draft.filter((x) => x !== k) : [...draft, k]);
                    } else {
                      onCommit(on ? [] : [k]);
                      setOpen(false);
                    }
                  }}
                >
                  <HugeiconsIcon
                    icon={Tick02Icon}
                    strokeWidth={2}
                    className={cn("size-3.5 shrink-0", !on && "invisible")}
                  />
                  {render(t)}
                </button>
              </li>
            );
          })}
        </ul>
      </PopoverContent>
    </Popover>
  );
}

export function LabelDot({ color }: { color: string }) {
  return (
    <span
      aria-hidden
      className="inline-block size-3 shrink-0 rounded-sm border border-black/10"
      style={{ backgroundColor: `#${color}` }}
    />
  );
}

export function LabelPicker({
  selected,
  onCommit,
  trigger,
  ...repo
}: Repo & {
  selected: string[];
  onCommit: (labels: Schemas["label"][]) => void;
  trigger?: ReactNode;
}) {
  return (
    <Region name="labels" fallback={null}>
      <LabelPickerBody {...repo} selected={selected} onCommit={onCommit} trigger={trigger} />
    </Region>
  );
}

function LabelPickerBody({
  selected,
  onCommit,
  trigger,
  ...repo
}: Repo & {
  selected: string[];
  onCommit: (labels: Schemas["label"][]) => void;
  trigger?: ReactNode;
}) {
  const { data } = useSuspenseQuery(labelsQuery(repo));
  return (
    <Picker
      title="Labels"
      items={data}
      keyOf={(l) => l.name}
      text={(l) => `${l.name} ${l.description ?? ""}`}
      render={(l) => (
        <span className="flex min-w-0 items-start gap-2">
          <span className="mt-0.5">
            <LabelDot color={l.color} />
          </span>
          <span className="min-w-0">
            <span className="block truncate">{l.name}</span>
            {l.description && (
              <span className="text-muted-foreground block truncate text-xs">{l.description}</span>
            )}
          </span>
        </span>
      )}
      selected={selected}
      onCommit={(names) => onCommit(data.filter((l) => names.includes(l.name)))}
      multiple
      trigger={trigger}
    />
  );
}

export function AssigneePicker({
  selected,
  onCommit,
  trigger,
  ...repo
}: Repo & {
  selected: string[];
  onCommit: (users: Schemas["simple-user"][]) => void;
  trigger?: ReactNode;
}) {
  return (
    <Region name="assignees" fallback={null}>
      <AssigneePickerBody {...repo} selected={selected} onCommit={onCommit} trigger={trigger} />
    </Region>
  );
}

function AssigneePickerBody({
  selected,
  onCommit,
  trigger,
  ...repo
}: Repo & {
  selected: string[];
  onCommit: (users: Schemas["simple-user"][]) => void;
  trigger?: ReactNode;
}) {
  const { data } = useSuspenseQuery(assigneesQuery(repo));
  return (
    <Picker
      title="Assignees"
      items={data}
      keyOf={(u) => u.login}
      text={(u) => `${u.login} ${u.name ?? ""}`}
      render={(u) => (
        <span className="flex items-center gap-2">
          <UserAvatar user={u} />
          <span className="font-medium">{u.login}</span>
          <span className="text-muted-foreground truncate text-xs">{u.name}</span>
        </span>
      )}
      selected={selected}
      onCommit={(logins) => onCommit(data.filter((u) => logins.includes(u.login)))}
      multiple
      trigger={trigger}
    />
  );
}

export function MilestonePicker({
  selected,
  onCommit,
  trigger,
  ...repo
}: Repo & {
  selected: number | null;
  onCommit: (m: Schemas["milestone"] | null) => void;
  trigger?: ReactNode;
}) {
  return (
    <Region name="milestones" fallback={null}>
      <MilestonePickerBody {...repo} selected={selected} onCommit={onCommit} trigger={trigger} />
    </Region>
  );
}

function MilestonePickerBody({
  selected,
  onCommit,
  trigger,
  ...repo
}: Repo & {
  selected: number | null;
  onCommit: (m: Schemas["milestone"] | null) => void;
  trigger?: ReactNode;
}) {
  const { data } = useSuspenseQuery(milestonesQuery(repo));
  const open = data.filter((m) => m.state === "open" || m.number === selected);
  return (
    <Picker
      title="Milestone"
      items={open}
      keyOf={(m) => String(m.number)}
      text={(m) => m.title}
      render={(m) => (
        <span className="min-w-0">
          <span className="block truncate">{m.title}</span>
          {m.due_on && (
            <span className="text-muted-foreground block text-xs">Due {formatDue(m.due_on)}</span>
          )}
        </span>
      )}
      selected={selected === null ? [] : [String(selected)]}
      onCommit={(keys) => onCommit(open.find((m) => String(m.number) === keys[0]) ?? null)}
      multiple={false}
      trigger={trigger}
    />
  );
}

// GHES stores a due date as 07:00 UTC on that day; show the day in UTC so it never drifts.
export function formatDue(dueOn: string) {
  return new Date(dueOn).toLocaleDateString("en", {
    timeZone: "UTC",
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}
