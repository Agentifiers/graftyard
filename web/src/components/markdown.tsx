import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { cn } from "cn";

// GHES bodies (issues, comments, releases, reviews) are GitHub Flavored Markdown.
// react-markdown escapes raw HTML, so a body can never inject markup.
export function MarkdownBody({ text, className }: { text: string; className?: string }) {
  return (
    <div
      className={cn(
        "prose prose-sm dark:prose-invert prose-p:my-2 prose-pre:bg-muted prose-pre:text-foreground prose-code:before:content-none prose-code:after:content-none prose-code:font-normal [&_:not(pre)>code]:bg-muted max-w-none leading-relaxed [&_:not(pre)>code]:rounded-sm [&_:not(pre)>code]:px-1 [&_:not(pre)>code]:py-0.5",
        className,
      )}
    >
      <Markdown remarkPlugins={[remarkGfm]}>{text}</Markdown>
    </div>
  );
}
