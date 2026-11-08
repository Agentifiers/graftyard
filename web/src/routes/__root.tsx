import type { QueryClient } from "@tanstack/react-query";
import { useSuspenseQuery } from "@tanstack/react-query";
import {
  ClientOnly,
  HeadContent,
  Link,
  Outlet,
  Scripts,
  ScriptOnce,
  createRootRouteWithContext,
} from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { Moon02Icon, Sun03Icon } from "@hugeicons/core-free-icons";
import { userQuery } from "@/areas/core/queries";
import { UserAvatar } from "@/components/github";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import appCss from "../styles.css?url";

export const Route = createRootRouteWithContext<{ queryClient: QueryClient }>()({
  head: () => ({
    meta: [
      { charSet: "utf-8" },
      { name: "viewport", content: "width=device-width, initial-scale=1" },
      { title: "Graftyard" },
    ],
    links: [
      { rel: "stylesheet", href: appCss },
      { rel: "icon", href: "/favicon.ico" },
    ],
  }),
  notFoundComponent: () => (
    <main className="mx-auto max-w-6xl p-6">
      <h1 className="text-lg font-semibold">Not found</h1>
      <p className="text-muted-foreground text-sm">No page lives at this address.</p>
    </main>
  ),
  shellComponent: RootDocument,
  component: AppShell,
});

// Runs before paint so the saved theme never flashes.
const THEME_SCRIPT = `try{var t=localStorage.getItem("theme");if(t==="dark"||(!t&&matchMedia("(prefers-color-scheme: dark)").matches))document.documentElement.classList.add("dark")}catch(e){}`;

function RootDocument({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <HeadContent />
      </head>
      <body className="bg-background text-foreground min-h-svh antialiased">
        <ScriptOnce>{THEME_SCRIPT}</ScriptOnce>
        {children}
        <Scripts />
      </body>
    </html>
  );
}

function AppShell() {
  return (
    <>
      <header className="bg-card border-b">
        <div className="mx-auto flex h-12 max-w-6xl items-center gap-3 px-4">
          <Link to="/" className="flex items-center gap-2 text-sm font-semibold tracking-tight">
            <Logo />
            Graftyard
          </Link>
          <div className="ml-auto flex items-center gap-1">
            <ThemeToggle />
            {/* The shell prerender has no API, so the viewer renders only in the browser. */}
            <ClientOnly fallback={<Skeleton className="size-6 rounded-sm" />}>
              <Region name="user" fallback={<Skeleton className="size-6 rounded-sm" />}>
                <Viewer />
              </Region>
            </ClientOnly>
          </div>
        </div>
      </header>
      <Outlet />
    </>
  );
}

function Viewer() {
  const { data: user } = useSuspenseQuery(userQuery());
  return (
    <span className="flex items-center gap-2 pl-1 text-sm" title={user.name ?? user.login}>
      <UserAvatar user={user} />
      <span className="text-muted-foreground hidden sm:inline">{user.login}</span>
    </span>
  );
}

function ThemeToggle() {
  const toggle = () => {
    const dark = document.documentElement.classList.toggle("dark");
    localStorage.setItem("theme", dark ? "dark" : "light");
  };
  return (
    <Button variant="ghost" size="icon-sm" onClick={toggle} aria-label="Toggle light or dark theme">
      <HugeiconsIcon icon={Sun03Icon} strokeWidth={2} className="hidden dark:block" />
      <HugeiconsIcon icon={Moon02Icon} strokeWidth={2} className="dark:hidden" />
    </Button>
  );
}

function Logo() {
  return (
    <svg
      viewBox="0 0 24 24"
      className="text-primary size-5"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      aria-hidden
    >
      <circle cx="6" cy="6" r="2.5" />
      <circle cx="6" cy="18" r="2.5" />
      <circle cx="18" cy="9" r="2.5" />
      <path d="M6 8.5v7M18 11.5c0 4-6 3-10.5 5" strokeLinecap="round" />
    </svg>
  );
}
