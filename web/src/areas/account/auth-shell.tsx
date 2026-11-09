import type { ReactNode } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import type { IconSvgElement } from "@hugeicons/react";

// The narrow centred card the sign-in, device, and consent pages share.
export function AuthCard({
  icon,
  title,
  subtitle,
  children,
  footer,
}: {
  icon: IconSvgElement;
  title: ReactNode;
  subtitle?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
}) {
  return (
    <main className="mx-auto flex w-full max-w-sm flex-col items-center gap-4 px-4 py-10">
      <HugeiconsIcon icon={icon} strokeWidth={1.5} className="text-muted-foreground size-10" />
      <div className="text-center">
        <h1 className="text-xl font-semibold">{title}</h1>
        {subtitle && <p className="text-muted-foreground mt-1 text-sm">{subtitle}</p>}
      </div>
      <div className="bg-card w-full rounded-sm border p-4">{children}</div>
      {footer && (
        <div className="text-muted-foreground w-full border p-3 text-center text-sm">{footer}</div>
      )}
    </main>
  );
}
