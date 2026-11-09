import type { Schemas } from "@/api/client";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { RepoPicker } from "./token-fields";

type Repo = Schemas["repository"];

// "All repositories" or "Only select repositories", with the picker for the second.
export function RepositorySelection({
  selection,
  onSelection,
  repos,
  selected,
  onSelected,
  owner,
}: {
  selection: "all" | "selected";
  onSelection: (s: "all" | "selected") => void;
  repos: Repo[];
  selected: string[];
  onSelected: (names: string[]) => void;
  owner: string;
}) {
  return (
    <div className="flex flex-col gap-3" data-testid="repository-selection">
      <RadioGroup value={selection} onValueChange={(v) => onSelection(v as "all" | "selected")}>
        <label className="flex items-start gap-2 text-sm">
          <RadioGroupItem value="all" className="mt-0.5" />
          <span>
            <span className="font-medium">All repositories</span>
            <span className="text-muted-foreground block text-xs">
              Every current and future repository {owner} owns, {repos.length} today.
            </span>
          </span>
        </label>
        <label className="flex items-start gap-2 text-sm">
          <RadioGroupItem value="selected" className="mt-0.5" />
          <span>
            <span className="font-medium">Only select repositories</span>
            <span className="text-muted-foreground block text-xs">
              Choose which ones the App can reach.
            </span>
          </span>
        </label>
      </RadioGroup>
      {selection === "selected" && (
        <RepoPicker repos={repos} selected={selected} onChange={onSelected} />
      )}
    </div>
  );
}
