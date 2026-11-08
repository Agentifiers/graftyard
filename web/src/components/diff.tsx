import type { Schemas } from "@/api/client";
import { Badge } from "@/components/reui/badge";

type Line =
  | { kind: "hunk"; text: string }
  | { kind: "context" | "add" | "del"; text: string; old?: number; new?: number };

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

// Parses a GitHub `patch` (hunks only, no file headers) into numbered lines.
export function parsePatch(patch: string): Line[] {
  const lines: Line[] = [];
  let oldNo = 0;
  let newNo = 0;
  for (const raw of patch.split("\n")) {
    const hunk = HUNK.exec(raw);
    if (hunk) {
      oldNo = Number(hunk[1]);
      newNo = Number(hunk[2]);
      lines.push({ kind: "hunk", text: raw });
    } else if (raw.startsWith("+")) {
      lines.push({ kind: "add", text: raw.slice(1), new: newNo++ });
    } else if (raw.startsWith("-")) {
      lines.push({ kind: "del", text: raw.slice(1), old: oldNo++ });
    } else if (!raw.startsWith("\\")) {
      lines.push({ kind: "context", text: raw.slice(1), old: oldNo++, new: newNo++ });
    }
  }
  return lines;
}

const ROW = {
  hunk: "bg-info/10 text-muted-foreground",
  context: "",
  add: "bg-success/12",
  del: "bg-destructive/12",
} as const;
const SIGN = { hunk: "", context: " ", add: "+", del: "-" } as const;

export function FileDiff({ file }: { file: Schemas["diff-entry"] }) {
  return (
    <section className="bg-card overflow-hidden rounded-lg border" data-file={file.filename}>
      <header className="bg-muted/50 flex items-center gap-2 border-b px-3 py-1.5 text-xs">
        <code className="truncate font-mono font-medium">{file.filename}</code>
        <Badge variant="outline" size="sm" className="capitalize">
          {file.status}
        </Badge>
        <span className="ml-auto font-mono">
          <span className="text-success">+{file.additions}</span>{" "}
          <span className="text-destructive">-{file.deletions}</span>
        </span>
      </header>
      {file.patch ? (
        <div className="overflow-x-auto">
          <table className="w-full border-collapse font-mono text-xs leading-5">
            <tbody>
              {parsePatch(file.patch).map((l, i) =>
                l.kind === "hunk" ? (
                  <tr key={i} className={ROW.hunk}>
                    <td colSpan={4} className="px-3 py-0.5">
                      {l.text}
                    </td>
                  </tr>
                ) : (
                  <tr key={i} className={ROW[l.kind]}>
                    <td className="text-muted-foreground w-10 border-r px-2 text-right select-none">
                      {l.old}
                    </td>
                    <td className="text-muted-foreground w-10 border-r px-2 text-right select-none">
                      {l.new}
                    </td>
                    <td className="w-4 pl-2 select-none">{SIGN[l.kind]}</td>
                    <td className="pr-3 whitespace-pre">{l.text}</td>
                  </tr>
                ),
              )}
            </tbody>
          </table>
        </div>
      ) : (
        <p className="text-muted-foreground px-3 py-2 text-xs">
          Binary file or diff too large to show.
        </p>
      )}
    </section>
  );
}
