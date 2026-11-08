import type { ReactNode } from "react";
import { Frame, FrameHeader, FramePanel, FrameTitle } from "@/components/reui/frame";
import { cn } from "cn";

// The bordered box most pages are made of: a title bar, optional actions at its right,
// and one body. `flush` drops the body padding for lists and tables that draw their own
// rows. Use it instead of composing Frame by hand so every box has the same single border.
export function Panel({
  title,
  actions,
  flush,
  className,
  children,
}: {
  title?: ReactNode;
  actions?: ReactNode;
  flush?: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <Frame spacing="sm" dense className={className}>
      {(title || actions) && (
        <FrameHeader className="min-h-10 flex-row flex-wrap items-center gap-2 py-1.5">
          {title && <FrameTitle className="text-sm">{title}</FrameTitle>}
          {actions && <div className="ml-auto flex flex-wrap items-center gap-2">{actions}</div>}
        </FrameHeader>
      )}
      <FramePanel className={cn((title || actions) && "rounded-t-none", flush && "p-0")}>
        {children}
      </FramePanel>
    </Frame>
  );
}

// A page's heading row: the title, a line under it, and actions at the right.
export function PageHeader({
  title,
  description,
  actions,
}: {
  title: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="mb-4 flex flex-wrap items-start gap-3">
      <div className="min-w-0 flex-1">
        <h1 className="text-xl font-semibold">{title}</h1>
        {description && <p className="text-muted-foreground mt-1 text-sm">{description}</p>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </div>
  );
}
