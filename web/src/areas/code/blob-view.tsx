import { useEffect, useMemo, useState } from "react";
import { Link, useLocation, useNavigate } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Delete02Icon,
  Download04Icon,
  Edit02Icon,
  File02Icon,
  HardDriveIcon,
  Image01Icon,
} from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { MarkdownBody } from "@/components/markdown";
import { Badge } from "@/components/reui/badge";
import {
  CodeBlock,
  CodeBlockCopyButton,
  CodeBlockHeader,
  CodeBlockLanguage,
  CodeBlockTitle,
  CodeBlockWrapToggle,
} from "@/components/reui/code-block/code-block";
import { Button } from "@/components/ui/button";
import { ButtonGroup } from "@/components/ui/button-group";
import { base64ToBytes, decodeText } from "./base64";
import { contentsQuery } from "./queries";
import { joinPath, type Repo } from "./refs";

// One file at a ref: highlighted code with line anchors, rendered Markdown with a Code
// toggle, an image, or a Git LFS pointer.

const LANG_BY_EXT: Record<string, string> = {
  rs: "rust",
  ts: "typescript",
  tsx: "tsx",
  js: "javascript",
  json: "json",
  md: "markdown",
  toml: "toml",
  yml: "yaml",
  yaml: "yaml",
  sh: "shell",
  css: "css",
  html: "html",
  py: "python",
  go: "go",
};

export function languageOf(path: string) {
  const name = path.split("/").at(-1)!;
  if (name === "Dockerfile") return "docker";
  return LANG_BY_EXT[name.split(".").at(-1)!.toLowerCase()];
}

const IMAGE = /\.(png|jpe?g|gif|webp|svg)$/i;
const LFS =
  /^version https:\/\/git-lfs\.github\.com\/spec\/v1\noid sha256:([0-9a-f]{64})\nsize (\d+)\n?$/;

export type FileKind =
  | { kind: "text"; text: string }
  | { kind: "markdown"; text: string }
  | { kind: "image"; src: string }
  | { kind: "lfs"; oid: string; size: number }
  | { kind: "binary" };

export function classify(file: Schemas["content-file"]): FileKind {
  if (IMAGE.test(file.path)) {
    const mime = file.path.endsWith(".svg")
      ? "image/svg+xml"
      : `image/${file.path.split(".").at(-1)!.replace("jpg", "jpeg")}`;
    return { kind: "image", src: `data:${mime};base64,${file.content.replace(/\s+/g, "")}` };
  }
  const text = decodeText(file.content);
  if (text === null) return { kind: "binary" };
  const lfs = LFS.exec(text);
  if (lfs) return { kind: "lfs", oid: lfs[1], size: Number(lfs[2]) };
  if (/\.(md|markdown)$/i.test(file.path)) return { kind: "markdown", text };
  return { kind: "text", text };
}

export function formatBytes(n: number) {
  if (n < 1024) return `${n} Bytes`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

// "#L4" or "#L4-L9" to the lines it names, and back.
export function parseHash(hash: string): number[] {
  const m = /^L(\d+)(?:-L(\d+))?$/.exec(hash.replace(/^#/, ""));
  if (!m) return [];
  const a = Number(m[1]);
  const b = Number(m[2] ?? m[1]);
  const [lo, hi] = a <= b ? [a, b] : [b, a];
  return Array.from({ length: hi - lo + 1 }, (_, i) => lo + i);
}

export function toHash(lines: number[]) {
  if (lines.length === 0) return "";
  const lo = Math.min(...lines);
  const hi = Math.max(...lines);
  return lo === hi ? `L${lo}` : `L${lo}-L${hi}`;
}

export function BlobView({
  owner,
  repo,
  refName,
  path,
  onEdit,
  onDelete,
  editable,
}: Repo & {
  refName: string;
  path: string;
  onEdit: () => void;
  onDelete: () => void;
  editable: boolean;
}) {
  const { data } = useSuspenseQuery(contentsQuery({ owner, repo, ref: refName, path }));
  if (Array.isArray(data) || data.type !== "file") throw new Error(`${path} is not a file`);
  const file = data as Schemas["content-file"];
  const content = useMemo(() => classify(file), [file]);
  const [mode, setMode] = useState<"preview" | "code">("preview");
  const lines =
    content.kind === "text" || content.kind === "markdown"
      ? content.text.split("\n").length - (content.text.endsWith("\n") ? 1 : 0)
      : 0;
  const raw = () => {
    const bytes = base64ToBytes(file.content);
    const url = URL.createObjectURL(
      new Blob([bytes as BlobPart], { type: "application/octet-stream" }),
    );
    const a = document.createElement("a");
    a.href = url;
    a.download = file.name;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <section
      className="bg-card overflow-hidden rounded-sm border"
      data-testid="blob"
      data-kind={content.kind}
    >
      <header className="bg-muted/40 flex flex-wrap items-center gap-2 border-b px-3 py-1.5 text-xs">
        {content.kind === "markdown" && (
          <ButtonGroup aria-label="View">
            <Button
              size="xs"
              variant={mode === "preview" ? "secondary" : "outline"}
              onClick={() => setMode("preview")}
              aria-pressed={mode === "preview"}
            >
              Preview
            </Button>
            <Button
              size="xs"
              variant={mode === "code" ? "secondary" : "outline"}
              onClick={() => setMode("code")}
              aria-pressed={mode === "code"}
            >
              Code
            </Button>
          </ButtonGroup>
        )}
        <span className="text-muted-foreground">
          {lines > 0 && `${lines} lines · `}
          {formatBytes(file.size)}
        </span>
        <div className="ml-auto flex flex-wrap items-center gap-1">
          <ButtonGroup>
            <Button size="xs" variant="outline" onClick={raw} aria-label="Download raw file">
              <HugeiconsIcon icon={Download04Icon} strokeWidth={2} data-icon="inline-start" />
              Raw
            </Button>
            {(content.kind === "text" || content.kind === "markdown") && (
              <Button
                size="xs"
                variant="outline"
                render={
                  <Link
                    to="/$owner/$repo/blame/$"
                    params={{ owner, repo, _splat: joinPath(refName, path) }}
                  />
                }
              >
                Blame
              </Button>
            )}
            <Button
              size="xs"
              variant="outline"
              render={
                <Link
                  to="/$owner/$repo/commits/$"
                  params={{ owner, repo, _splat: joinPath(refName, path) }}
                />
              }
            >
              History
            </Button>
          </ButtonGroup>
          {(content.kind === "text" || content.kind === "markdown") && (
            <Button
              size="icon-xs"
              variant="ghost"
              onClick={onEdit}
              disabled={!editable}
              aria-label="Edit this file"
              title={editable ? "Edit this file" : "Switch to a branch to edit"}
            >
              <HugeiconsIcon icon={Edit02Icon} strokeWidth={2} />
            </Button>
          )}
          <Button
            size="icon-xs"
            variant="ghost"
            onClick={onDelete}
            disabled={!editable}
            aria-label="Delete this file"
            title={editable ? "Delete this file" : "Switch to a branch to delete"}
            className="text-destructive"
          >
            <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
          </Button>
        </div>
      </header>
      <FileBody file={file} content={content} mode={mode} />
    </section>
  );
}

function FileBody({
  file,
  content,
  mode,
}: {
  file: Schemas["content-file"];
  content: FileKind;
  mode: "preview" | "code";
}) {
  switch (content.kind) {
    case "markdown":
      return mode === "preview" ? (
        <div className="px-6 py-4" data-testid="markdown-preview">
          <MarkdownBody text={content.text} />
        </div>
      ) : (
        <Code path={file.path} text={content.text} />
      );
    case "text":
      return <Code path={file.path} text={content.text} />;
    case "image":
      return (
        <div className="bg-muted/30 flex flex-col items-center gap-2 p-6" data-testid="image">
          <img
            src={content.src}
            alt={file.name}
            className="max-h-[32rem] max-w-full border"
            style={{ imageRendering: "pixelated" }}
          />
          <span className="text-muted-foreground flex items-center gap-1 text-xs">
            <HugeiconsIcon icon={Image01Icon} strokeWidth={2} className="size-3.5" />
            {file.name}
          </span>
        </div>
      );
    case "lfs":
      return (
        <div className="flex flex-col items-center gap-2 px-6 py-10 text-center" data-testid="lfs">
          <HugeiconsIcon
            icon={HardDriveIcon}
            strokeWidth={2}
            className="text-muted-foreground size-8"
          />
          <p className="text-sm font-semibold">Stored with Git LFS</p>
          <p className="text-muted-foreground text-sm">
            {formatBytes(content.size)} ·{" "}
            <code className="font-mono text-xs">sha256:{content.oid.slice(0, 12)}</code>
          </p>
          <Badge variant="outline">This pointer is all the git object holds</Badge>
        </div>
      );
    case "binary":
      return (
        <div className="text-muted-foreground flex items-center justify-center gap-2 px-6 py-10 text-sm">
          <HugeiconsIcon icon={File02Icon} strokeWidth={2} className="size-5" />
          Binary file not shown.
        </div>
      );
  }
}

// Highlighted lines. Clicking a line number selects it and shift-click a range; the
// selection lives in the URL hash, so a link to #L4-L9 opens with those lines lit.
function Code({ path, text }: { path: string; text: string }) {
  const location = useLocation();
  const navigate = useNavigate();
  const selected = parseHash(location.hash);
  const first = selected[0];
  useEffect(() => {
    if (first === undefined) return;
    const el = document.querySelector(`[data-code-line="${first}"]`);
    el?.scrollIntoView({ block: "center" });
  }, [first]);
  const source = text.endsWith("\n") ? text.slice(0, -1) : text;
  return (
    <CodeBlock
      code={source}
      language={languageOf(path)}
      showLineNumbers
      selectable
      selectedLines={selected}
      highlightedLines={selected}
      onSelectedLinesChange={(next) => {
        const range = next.length ? [Math.min(...next), Math.max(...next)] : [];
        void navigate({
          to: ".",
          hash: toHash(
            range.length
              ? Array.from({ length: range[1] - range[0] + 1 }, (_, i) => range[0] + i)
              : [],
          ),
          replace: true,
          resetScroll: false,
          hashScrollIntoView: false,
        });
      }}
      variant="ghost"
      className="rounded-none border-0"
      data-testid="code"
    >
      <CodeBlockHeader className="bg-muted/20">
        <CodeBlockTitle>{path.split("/").at(-1)}</CodeBlockTitle>
        <CodeBlockLanguage />
        <span className="text-muted-foreground text-xs">
          {selected.length > 0
            ? `Lines ${toHash(selected).replace(/L/g, "")} selected`
            : "Click a line to link to it, shift-click for a range"}
        </span>
        <div className="ml-auto flex items-center gap-1">
          <CodeBlockWrapToggle />
          <CodeBlockCopyButton />
        </div>
      </CodeBlockHeader>
    </CodeBlock>
  );
}
