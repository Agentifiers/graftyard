import { useEffect, useSyncExternalStore } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { ComputerIcon, Moon02Icon, Sun03Icon } from "@hugeicons/core-free-icons";
import { Section } from "@/areas/account/kit";
import { PageHeader } from "@/components/panel";
import { cn } from "cn";

// The same switch the top bar's toggle flips: `localStorage.theme` ("light" or "dark",
// absent for the system setting) and the `dark` class on <html>, which the root layout's
// pre-paint script reads (web/src/routes/__root.tsx).
export const Route = createFileRoute("/settings/appearance")({
  component: AppearancePage,
});

type Theme = "light" | "dark" | "system";

const THEMES: { value: Theme; label: string; icon: typeof Sun03Icon; hint: string }[] = [
  { value: "light", label: "Light", icon: Sun03Icon, hint: "Always light" },
  { value: "dark", label: "Dark", icon: Moon02Icon, hint: "Always dark" },
  { value: "system", label: "System", icon: ComputerIcon, hint: "Follow your operating system" },
];

const media = () => matchMedia("(prefers-color-scheme: dark)");

function stored(): Theme {
  const t = localStorage.getItem("theme");
  return t === "light" || t === "dark" ? t : "system";
}

function apply(theme: Theme) {
  if (theme === "system") localStorage.removeItem("theme");
  else localStorage.setItem("theme", theme);
  const dark = theme === "dark" || (theme === "system" && media().matches);
  document.documentElement.classList.toggle("dark", dark);
  // A same-tab storage write fires no "storage" event, and the class may not change.
  window.dispatchEvent(new Event(THEME_EVENT));
}

const THEME_EVENT = "graftyard:theme";

// Re-renders when the top bar's toggle changes the class or another tab changes storage.
function subscribe(onChange: () => void) {
  const observer = new MutationObserver(onChange);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
  window.addEventListener("storage", onChange);
  window.addEventListener(THEME_EVENT, onChange);
  const mq = media();
  mq.addEventListener("change", onChange);
  return () => {
    observer.disconnect();
    window.removeEventListener("storage", onChange);
    window.removeEventListener(THEME_EVENT, onChange);
    mq.removeEventListener("change", onChange);
  };
}

function AppearancePage() {
  const theme = useSyncExternalStore(subscribe, stored, () => "system" as Theme);
  const dark = useSyncExternalStore(
    subscribe,
    () => document.documentElement.classList.contains("dark"),
    () => false,
  );

  // On "system", follow the OS while the page is open.
  useEffect(() => {
    if (theme !== "system") return;
    const mq = media();
    const follow = () => apply("system");
    mq.addEventListener("change", follow);
    return () => mq.removeEventListener("change", follow);
  }, [theme]);

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Appearance"
        description="How Graftyard looks to you. Saved in this browser."
      />
      <Section title="Theme mode" description={`Showing the ${dark ? "dark" : "light"} theme now.`}>
        <div
          role="radiogroup"
          aria-label="Theme"
          className="grid gap-3 sm:grid-cols-3"
          data-testid="theme-picker"
        >
          {THEMES.map((t) => {
            const selected = theme === t.value;
            return (
              <button
                key={t.value}
                type="button"
                role="radio"
                aria-checked={selected}
                data-theme-option={t.value}
                onClick={() => apply(t.value)}
                className={cn(
                  "hover:border-primary/60 flex flex-col overflow-hidden rounded-sm border text-left transition-colors",
                  selected && "border-primary ring-primary/30 ring-2",
                )}
              >
                <Preview variant={t.value} />
                <span className="flex items-center gap-2 border-t px-3 py-2 text-sm font-medium">
                  <HugeiconsIcon icon={t.icon} strokeWidth={2} className="size-4" />
                  {t.label}
                  <span className="text-muted-foreground ml-auto text-xs font-normal">
                    {t.hint}
                  </span>
                </span>
              </button>
            );
          })}
        </div>
      </Section>
    </div>
  );
}

// A small drawing of a page in each theme. Fixed colours, so a preview never follows the
// page's own theme.
function Preview({ variant }: { variant: Theme }) {
  const half = (bg: string, bar: string, line: string, accent: string) => (
    <div className="flex flex-1 flex-col gap-1.5 p-2" style={{ background: bg }}>
      <div className="h-2 w-2/3" style={{ background: bar }} />
      <div className="h-1.5 w-full" style={{ background: line }} />
      <div className="h-1.5 w-5/6" style={{ background: line }} />
      <div className="mt-auto h-2.5 w-1/3" style={{ background: accent }} />
    </div>
  );
  const light = half("#ffffff", "#d0d7de", "#eaeef2", "#1f883d");
  const dark = half("#0d1117", "#30363d", "#21262d", "#2ea043");
  return (
    <div className="flex h-20" aria-hidden>
      {variant === "light" ? (
        light
      ) : variant === "dark" ? (
        dark
      ) : (
        <>
          {light}
          {dark}
        </>
      )}
    </div>
  );
}
