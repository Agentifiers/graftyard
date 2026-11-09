import { Suspense, useState } from "react";
import { useSuspenseQuery } from "@tanstack/react-query";
import { useDebouncedValue } from "@tanstack/react-pacer";
import { markdownQuery } from "./queries";
import { Region, RegionSkeleton } from "@/components/region";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "cn";

// A markdown body with Write and Preview tabs. Preview renders through POST /markdown,
// as GHES's own editor does, debounced so typing does not send a request per key.
export function MarkdownEditor({
  value,
  onChange,
  onBlur,
  context,
  label,
  placeholder,
  invalid,
  className,
  onSubmitShortcut,
}: {
  value: string;
  onChange: (v: string) => void;
  onBlur?: () => void;
  context: string;
  label: string;
  placeholder?: string;
  invalid?: boolean;
  className?: string;
  onSubmitShortcut?: () => void;
}) {
  const [tab, setTab] = useState("write");
  return (
    <Tabs value={tab} onValueChange={(v) => setTab(String(v))} className="gap-0">
      <TabsList variant="line" className="h-8 border-b px-1 group-data-horizontal/tabs:h-8">
        <TabsTrigger value="write" className="rounded-sm">
          Write
        </TabsTrigger>
        <TabsTrigger value="preview" className="rounded-sm">
          Preview
        </TabsTrigger>
      </TabsList>
      <TabsContent value="write" className="pt-2">
        <Textarea
          aria-label={label}
          aria-invalid={invalid || undefined}
          placeholder={placeholder}
          className={cn("min-h-32 rounded-sm font-mono text-sm", className)}
          value={value}
          onBlur={onBlur}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) onSubmitShortcut?.();
          }}
        />
        <p className="text-muted-foreground mt-1 text-xs">Markdown is supported.</p>
      </TabsContent>
      <TabsContent value="preview" className="pt-2">
        {tab === "preview" && <Preview text={value} context={context} />}
      </TabsContent>
    </Tabs>
  );
}

function Preview({ text, context }: { text: string; context: string }) {
  const [debounced] = useDebouncedValue(text, { wait: 250 });
  if (!debounced.trim()) {
    return <p className="text-muted-foreground min-h-32 border p-3 text-sm">Nothing to preview.</p>;
  }
  return (
    <div className="min-h-32 border p-3" data-testid="markdown-preview">
      <Region name="preview" rows={3}>
        <Suspense fallback={<RegionSkeleton rows={3} />}>
          <Rendered text={debounced} context={context} />
        </Suspense>
      </Region>
    </div>
  );
}

function Rendered({ text, context }: { text: string; context: string }) {
  const { data } = useSuspenseQuery(markdownQuery(text, context));
  return (
    <div
      className="prose prose-sm dark:prose-invert prose-p:my-2 prose-pre:bg-muted prose-pre:text-foreground prose-code:before:content-none prose-code:after:content-none [&_:not(pre)>code]:bg-muted max-w-none [&_:not(pre)>code]:rounded-sm [&_:not(pre)>code]:px-1"
      // The mock renders with react-markdown, which escapes raw HTML in the source.
      dangerouslySetInnerHTML={{ __html: data }}
    />
  );
}
