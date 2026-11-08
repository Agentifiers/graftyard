import { Suspense, type ReactNode } from "react";
import { QueryErrorResetBoundary } from "@tanstack/react-query";
import { ErrorBoundary } from "react-error-boundary";
import { HugeiconsIcon } from "@hugeicons/react";
import { AlertCircleIcon, Refresh01Icon } from "@hugeicons/core-free-icons";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";

// One page region: its own error boundary, its own Suspense, its own queries.
// "Try again" resets the failed queries inside this region only; a region that reads a
// TanStack DB collection passes onRetry to reload it.
export function Region({
  name,
  children,
  rows = 3,
  fallback,
  onRetry,
}: {
  name: string;
  children: ReactNode;
  rows?: number;
  fallback?: ReactNode;
  onRetry?: () => unknown;
}) {
  return (
    <QueryErrorResetBoundary>
      {({ reset }) => (
        <ErrorBoundary
          onReset={() => {
            reset();
            onRetry?.();
          }}
          fallbackRender={({ error, resetErrorBoundary }) => (
            <Alert variant="destructive" data-region-error={name}>
              <HugeiconsIcon icon={AlertCircleIcon} strokeWidth={2} />
              <AlertTitle className="line-clamp-none">Could not load {name}</AlertTitle>
              <AlertDescription>
                {error instanceof Error ? error.message : String(error)}
                <Button size="sm" variant="outline" className="mt-1" onClick={resetErrorBoundary}>
                  <HugeiconsIcon icon={Refresh01Icon} strokeWidth={2} data-icon="inline-start" />
                  Try again
                </Button>
              </AlertDescription>
            </Alert>
          )}
        >
          <Suspense fallback={fallback ?? <RegionSkeleton rows={rows} />}>{children}</Suspense>
        </ErrorBoundary>
      )}
    </QueryErrorResetBoundary>
  );
}

export function RegionSkeleton({ rows = 3 }: { rows?: number }) {
  return (
    <div className="flex flex-col gap-2" aria-busy="true">
      {Array.from({ length: rows }, (_, i) => (
        <Skeleton key={i} className="h-5" style={{ width: `${90 - ((i * 17) % 40)}%` }} />
      ))}
    </div>
  );
}
