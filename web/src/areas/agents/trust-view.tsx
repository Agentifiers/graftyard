import { HugeiconsIcon } from "@hugeicons/react";
import {
  ArrowRight01Icon,
  Robot01Icon,
  SecurityCheckIcon,
  Alert02Icon,
  UserIcon,
  ViewOffIcon,
} from "@hugeicons/core-free-icons";
import { Badge } from "@/components/reui/badge";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "cn";
import type { TrustView } from "./api";

// Renders a `trust_view` (design/AGENT-LAYER.md, Shape, `TrustView`). `visible` is what a
// reader of the rendered page sees, shown as plain text and never as Markdown or HTML, so
// nothing in it can hide again. The counts say what the edge dropped.

const HIDDEN: { key: keyof TrustView["hidden"]; label: (n: number) => string }[] = [
  { key: "html_comments", label: (n) => `${n} HTML comment${n === 1 ? "" : "s"}` },
  { key: "invisible_chars", label: (n) => `${n} invisible character${n === 1 ? "" : "s"}` },
  { key: "link_title_text", label: (n) => `${n} link or image title${n === 1 ? "" : "s"}` },
  { key: "collapsed", label: (n) => `${n} collapsed <details> block${n === 1 ? "" : "s"}` },
];

export function hiddenTotal(view: TrustView) {
  return HIDDEN.reduce((n, h) => n + view.hidden[h.key], 0);
}

export function TrustText({
  view,
  className,
  inline,
}: {
  view: TrustView;
  className?: string;
  inline?: boolean;
}) {
  const Tag = inline ? "span" : "p";
  return (
    <Tag
      data-trust-visible
      className={cn(!inline && "text-sm break-words whitespace-pre-wrap", className)}
    >
      {view.visible || <span className="text-muted-foreground italic">Nothing visible</span>}
    </Tag>
  );
}

export function HiddenDisclosure({ view }: { view: TrustView }) {
  const total = hiddenTotal(view);
  if (!total) return null;
  return (
    <Collapsible data-trust-hidden={total}>
      <CollapsibleTrigger className="text-warning-foreground dark:text-warning group flex items-center gap-1 text-xs font-medium">
        <HugeiconsIcon
          icon={ArrowRight01Icon}
          strokeWidth={2}
          className="size-3.5 transition-transform group-data-[panel-open]:rotate-90"
        />
        <HugeiconsIcon icon={ViewOffIcon} strokeWidth={2} className="size-3.5" />
        Hidden content ({total})
      </CollapsibleTrigger>
      <CollapsibleContent>
        <ul className="text-muted-foreground mt-1 ml-5 list-disc text-xs">
          {HIDDEN.filter((h) => view.hidden[h.key] > 0).map((h) => (
            <li key={h.key} data-hidden-kind={h.key}>
              {h.label(view.hidden[h.key])} removed
            </li>
          ))}
        </ul>
        <p className="text-muted-foreground mt-1 ml-5 text-xs">
          A reader of the page does not see these. Agents get only the visible text.
        </p>
      </CollapsibleContent>
    </Collapsible>
  );
}

const AUTHOR_FLAGS: {
  key: Exclude<keyof TrustView["author"], "association" | "trusted">;
  on: string;
  off: string;
}[] = [
  { key: "can_push", on: "Can push", off: "Cannot push" },
  { key: "is_bot", on: "Bot", off: "Not a bot" },
  { key: "is_agent", on: "Agent", off: "Not an agent" },
  { key: "first_contribution", on: "First contribution", off: "Returning" },
];

export function AuthorBadges({ author }: { author: TrustView["author"] }) {
  return (
    <div className="flex flex-wrap items-center gap-1" data-trust-author>
      <Badge
        variant={author.trusted ? "success-light" : "warning-light"}
        size="sm"
        data-trusted={author.trusted}
      >
        <HugeiconsIcon icon={author.trusted ? SecurityCheckIcon : Alert02Icon} strokeWidth={2} />
        {author.trusted ? "Trusted" : "Untrusted"}
      </Badge>
      <Badge variant="outline" size="sm" className="font-mono">
        {author.association}
      </Badge>
      {AUTHOR_FLAGS.map((f) =>
        author[f.key] || f.key === "can_push" ? (
          <Badge
            key={f.key}
            variant={f.key === "can_push" && !author.can_push ? "destructive-outline" : "outline"}
            size="sm"
            data-flag={f.key}
            data-value={author[f.key]}
          >
            {f.key === "is_agent" || f.key === "is_bot" ? (
              <HugeiconsIcon icon={Robot01Icon} strokeWidth={2} />
            ) : f.key === "can_push" ? (
              <HugeiconsIcon icon={UserIcon} strokeWidth={2} />
            ) : null}
            {author[f.key] ? f.on : f.off}
          </Badge>
        ) : null,
      )}
    </div>
  );
}

// The whole view: badges, the visible text, and the hidden-content disclosure.
export function TrustViewCard({ view, className }: { view: TrustView; className?: string }) {
  return (
    <div className={cn("flex flex-col gap-2", className)} data-trust-view>
      <AuthorBadges author={view.author} />
      <TrustText view={view} />
      <HiddenDisclosure view={view} />
    </div>
  );
}

// One line of untrusted text inside a list row: the visible text and a hidden marker.
export function TrustInline({ view }: { view: TrustView }) {
  const total = hiddenTotal(view);
  return (
    <span className="inline-flex min-w-0 flex-wrap items-center gap-1" data-trust-view>
      <TrustText view={view} inline className="break-words" />
      {total > 0 && (
        <span
          className="text-warning-foreground dark:text-warning inline-flex items-center gap-0.5 text-xs"
          title={HIDDEN.filter((h) => view.hidden[h.key])
            .map((h) => h.label(view.hidden[h.key]))
            .join(", ")}
          data-trust-hidden={total}
        >
          <HugeiconsIcon icon={ViewOffIcon} strokeWidth={2} className="size-3" />
          {total} hidden
        </span>
      )}
      {!view.author.trusted && (
        <Badge variant="warning-light" size="xs">
          Untrusted
        </Badge>
      )}
    </span>
  );
}
