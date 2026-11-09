// Mocking is on unless the build sets VITE_MOCK_API=false. The deployed app will set it.
export const MOCK_API = import.meta.env.VITE_MOCK_API !== "false";

let started: Promise<unknown> | undefined;

// Resolves once the service worker intercepts /api/v3. Queries wait on this, so none
// reaches the network first. On the server (the shell prerender) it is a no-op.
export function mocksReady(): Promise<unknown> {
  if (!MOCK_API || typeof window === "undefined") return Promise.resolve();
  started ??= Promise.all([import("msw/browser"), import("./handlers")]).then(
    ([{ setupWorker }, { handlers }]) =>
      setupWorker(...handlers).start({
        onUnhandledRequest: "bypass",
        quiet: true,
        serviceWorker: { url: `${import.meta.env.BASE_URL}mockServiceWorker.js` },
      }),
  );
  return started;
}
