import { useSyncExternalStore } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { TestTube01Icon } from "@hugeicons/core-free-icons";
import { MOCK_API } from "@/mocks/browser";
import { failingRegions, gatedRegions, onFaultsChange, setFailing } from "@/mocks/faults";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverDescription,
  PopoverHeader,
  PopoverTitle,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";

// The mock's control panel: every region a handler has gated so far, each with a switch
// that makes it answer 503. Same effect as ?fail=, without editing the URL. Hidden when
// the build talks to a real API.
export function MockPanel() {
  if (!MOCK_API) return null;
  return <Panel />;
}

const snapshot = () => `${gatedRegions().join(",")}|${[...failingRegions()].sort().join(",")}`;

function Panel() {
  const state = useSyncExternalStore(onFaultsChange, snapshot, () => "|");
  const [gatedRaw, failingRaw] = state.split("|");
  const gated = gatedRaw ? gatedRaw.split(",") : [];
  const failing = new Set(failingRaw ? failingRaw.split(",") : []);
  const queryClient = useQueryClient();
  const toggle = (region: string, on: boolean) => {
    const next = new Set(failing);
    if (on) next.add(region);
    else next.delete(region);
    setFailing(next);
    void queryClient.invalidateQueries();
  };
  return (
    <Popover>
      <PopoverTrigger
        render={
          <Button variant="ghost" size="sm" aria-label="Mock API faults">
            <HugeiconsIcon icon={TestTube01Icon} strokeWidth={2} data-icon="inline-start" />
            Mock
            {failing.size > 0 && (
              <Badge variant="destructive" size="xs">
                {failing.size}
              </Badge>
            )}
          </Button>
        }
      />
      <PopoverContent align="end" className="w-72">
        <PopoverHeader>
          <PopoverTitle>Mock API faults</PopoverTitle>
          <PopoverDescription>
            A failing region answers 503. Regions appear here once a page reads them.
          </PopoverDescription>
        </PopoverHeader>
        <ul className="-mx-1 flex max-h-80 flex-col overflow-y-auto" data-testid="mock-regions">
          {gated.map((region) => (
            <li key={region}>
              <label className="hover:bg-muted flex items-center justify-between gap-2 rounded-sm px-1 py-1 text-sm">
                <code className="font-mono text-xs">{region}</code>
                <Switch
                  size="sm"
                  checked={failing.has(region)}
                  onCheckedChange={(on) => toggle(region, on)}
                />
              </label>
            </li>
          ))}
        </ul>
        {failing.size > 0 && (
          <Button
            variant="outline"
            size="sm"
            onClick={() => (setFailing([]), void queryClient.invalidateQueries())}
          >
            Clear all faults
          </Button>
        )}
      </PopoverContent>
    </Popover>
  );
}
