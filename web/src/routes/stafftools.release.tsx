import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { ArrowTurnBackwardIcon, InformationCircleIcon } from "@hugeicons/core-free-icons";
import { releaseEntriesQuery } from "@/areas/admin/queries";
import { decodeEntry, type Decoded } from "@/areas/admin/release-codec";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge, type BadgeProps } from "@/components/reui/badge";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";

export const Route = createFileRoute("/stafftools/release")({
  validateSearch: (s: Record<string, unknown>): { entry?: number } => ({
    entry: Number(s.entry) > 0 ? Number(s.entry) : undefined,
  }),
  loader: ({ context: { queryClient } }) => void queryClient.prefetchQuery(releaseEntriesQuery()),
  component: () => (
    <>
      <PageHeader
        title="Release log"
        description="The install's signed Release group entries that the agents account reads: genesis, keys, controller rolls, their steps, and pre-signed pointer undos."
      />
      <Alert variant="info" className="mb-4">
        <HugeiconsIcon icon={InformationCircleIcon} strokeWidth={2} />
        <AlertTitle>Read only</AlertTitle>
        <AlertDescription>
          Entries are accepted only in index order and only with valid signatures. A signer submits
          one through POST /api/graftyard/v1/release/entries; this page shows GET.
        </AlertDescription>
      </Alert>
      <Region name="release log" rows={8}>
        <Entries />
      </Region>
    </>
  ),
});

// Each kind's family, for its badge colour.
const FAMILY: Record<string, BadgeProps["variant"]> = {
  Genesis: "primary-light",
  Keys: "primary-light",
  ReleaseUploaded: "outline",
  Intent: "info-light",
  IntentStep: "outline",
  IntentDone: "success-light",
  ControllerPointer: "warning-light",
  Halt: "destructive-light",
  Resume: "info-light",
  Abort: "destructive-light",
};

const short = (hex: string | null | undefined) =>
  hex ? `${hex.slice(0, 8)}…${hex.slice(-4)}` : "none";

function Entries() {
  const { data } = useSuspenseQuery(releaseEntriesQuery());
  const { entry } = Route.useSearch();
  const decoded = data.map((e) => ({ ...e, d: decodeEntry(e.entry) }));
  return (
    <Panel title={`${data.length} entries`} flush>
      <ol className="divide-y" data-testid="release-entries">
        {decoded.map(({ index, entry: raw, d }) => (
          <li key={index} id={`entry-${index}`} data-index={index}>
            <Collapsible key={entry === index ? "open" : "closed"} defaultOpen={entry === index}>
              <CollapsibleTrigger className="hover:bg-muted/40 flex w-full items-center gap-3 px-4 py-2 text-left text-sm">
                <span className="text-muted-foreground w-8 text-right font-mono text-xs tabular-nums">
                  {index}
                </span>
                <Summary d={d} />
              </CollapsibleTrigger>
              <CollapsibleContent className="bg-muted/30 border-t px-4 py-3 text-xs">
                <Detail d={d} raw={raw} parentIndex={parentOf(decoded, d)} />
              </CollapsibleContent>
            </Collapsible>
          </li>
        ))}
      </ol>
    </Panel>
  );
}

function Summary({ d }: { d: Decoded }) {
  if (!d.ok) {
    return (
      <>
        <Badge variant="outline" size="sm">
          opaque
        </Badge>
        <span className="text-muted-foreground text-xs" data-kind="opaque">
          {d.reason}, {d.bytes} bytes
        </span>
      </>
    );
  }
  const { kind, body, bound, signatures } = d.entry;
  const intent =
    typeof body.intent === "string"
      ? body.intent
      : typeof body.id === "string"
        ? body.id
        : undefined;
  return (
    <>
      <Badge variant={FAMILY[kind] ?? "outline"} size="sm" className="font-mono" data-kind={kind}>
        {kind}
      </Badge>
      <span className="min-w-0 flex-1 truncate">{describe(kind, body)}</span>
      {bound.parent && (
        <Badge
          variant="warning-outline"
          size="sm"
          title="A pre-signed undo binds its parent intent, not prev"
        >
          <HugeiconsIcon icon={ArrowTurnBackwardIcon} strokeWidth={2} />
          undo
        </Badge>
      )}
      {intent && (
        <code className="text-muted-foreground hidden text-xs md:inline">{short(intent)}</code>
      )}
      <span className="text-muted-foreground text-xs">{signatures.length} sig</span>
    </>
  );
}

function describe(kind: string, body: Record<string, unknown>): string {
  const intent = body.intent;
  switch (kind) {
    case "Genesis":
      return `Install ${String(body.install)} on ${(body.sites as string[] | undefined)?.join(", ")}`;
    case "Keys":
      return `Key set generation ${String(body.generation)}${body.agents_executor ? ", adds agents_executor" : ""}`;
    case "ReleaseUploaded":
      return `Release ${short(String(body.release))} uploaded for ${(body.versions as [string, string][] | undefined)?.map(([s]) => s).join(" and ")}`;
    case "Intent": {
      const i =
        intent && typeof intent === "object"
          ? Object.entries(intent as Record<string, unknown>)[0]
          : undefined;
      return i ? `${i[0]} ${JSON.stringify(i[1])}` : "Intent";
    }
    case "IntentStep":
      return `Step ${String(body.step)} of ${short(String(intent))}`;
    case "IntentDone":
      return `Intent ${short(String(intent))} done`;
    case "ControllerPointer":
      return `Pin graft-controller to ${short(String(body.pin))}${body.reason ? `: ${String(body.reason)}` : ""}`;
    case "Halt":
      return `Halt ${short(String(intent))}`;
    case "Resume":
      return `Resume ${short(String(intent))}`;
    default:
      return kind;
  }
}

// The index of the Intent entry an undo binds.
function parentOf(all: { index: number; d: Decoded }[], d: Decoded) {
  if (!d.ok || !d.entry.bound.parent) return undefined;
  const parent = d.entry.bound.parent;
  return all.find((e) => e.d.ok && e.d.entry.kind === "Intent" && e.d.entry.body.id === parent)
    ?.index;
}

function Detail({ d, raw, parentIndex }: { d: Decoded; raw: string; parentIndex?: number }) {
  return (
    <div className="grid gap-3 lg:grid-cols-2">
      {d.ok ? (
        <div className="flex flex-col gap-2">
          <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5">
            <dt className="text-muted-foreground">install</dt>
            <dd className="font-mono">{d.entry.bound.install}</dd>
            <dt className="text-muted-foreground">group</dt>
            <dd className="font-mono">{d.entry.bound.group}</dd>
            <dt className="text-muted-foreground">prev</dt>
            <dd className="font-mono break-all">
              {d.entry.bound.prev ?? "none (first entry, or an undo)"}
            </dd>
            {d.entry.bound.parent && (
              <>
                <dt className="text-muted-foreground">parent</dt>
                <dd className="font-mono">
                  {parentIndex ? (
                    <Link
                      from={Route.fullPath}
                      search={{ entry: parentIndex }}
                      hash={`entry-${parentIndex}`}
                      className="underline"
                    >
                      {d.entry.bound.parent} (entry {parentIndex})
                    </Link>
                  ) : (
                    d.entry.bound.parent
                  )}
                </dd>
              </>
            )}
          </dl>
          <pre
            className="bg-background max-h-72 overflow-auto rounded-sm border p-2"
            data-testid="entry-body"
          >
            {JSON.stringify(d.entry.body, null, 2)}
          </pre>
          <div>
            <h4 className="mb-1 font-semibold">Signatures</h4>
            <ul className="flex flex-col gap-1">
              {d.entry.signatures.map((s, i) => (
                <li key={i} className="font-mono break-all">
                  key {short(s.key)} · sig {short(s.sig)}
                </li>
              ))}
            </ul>
          </div>
        </div>
      ) : (
        <p>
          This console cannot decode this entry ({d.reason}). The first bytes are{" "}
          <code>{d.head}</code>. A newer console reads it.
        </p>
      )}
      <div>
        <h4 className="mb-1 font-semibold">Entry, base64</h4>
        <pre className="bg-background max-h-72 overflow-auto rounded-sm border p-2 break-all whitespace-pre-wrap">
          {raw}
        </pre>
      </div>
    </div>
  );
}
