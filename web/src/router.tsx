import { createRouter } from "@tanstack/react-router";
import { QueryClientProvider } from "@tanstack/react-query";
import { queryClient } from "./api/query-client";
import { RegionSkeleton } from "./components/region";
import { routeTree } from "./routeTree.gen";

export function getRouter() {
  return createRouter({
    routeTree,
    context: { queryClient },
    scrollRestoration: true,
    defaultPreload: "intent",
    // Loaders only prefetch; regions suspend on their own queries.
    defaultPreloadStaleTime: 0,
    defaultPendingComponent: () => (
      <div className="mx-auto max-w-6xl p-6">
        <RegionSkeleton rows={4} />
      </div>
    ),
    Wrap: ({ children }) => (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    ),
  });
}

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof getRouter>;
  }
}
