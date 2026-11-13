import { useState } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  ArrowDown01Icon,
  BookOpen01Icon,
  CodeIcon,
  GitBranchIcon,
  Tag01Icon,
  TerminalIcon,
} from "@hugeicons/core-free-icons";
import { repoQuery } from "@/areas/core/queries";
import type { SlotEntry } from "@/areas/slots";
import { MarkdownBody } from "@/components/markdown";
import { Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { UserAvatar } from "@/components/github";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Directory } from "./directory";
import {
  branchesQuery,
  contributorsQuery,
  languagesQuery,
  readmeQuery,
  tagsQuery,
} from "./queries";
import { CopyButton, RefPicker, type Repo } from "./refs";

// What the code area adds to the repository overview: the file list with a branch
// picker, the README, languages, contributors, and the Clone menu.

const LANGUAGE_COLORS: Record<string, string> = {
  Rust: "#dea584",
  TypeScript: "#3178c6",
  CSS: "#663399",
  Shell: "#89e051",
  Dockerfile: "#384d54",
};
const colorOf = (lang: string) => LANGUAGE_COLORS[lang] ?? "#8b949e";

function Overview({ owner, repo }: Repo) {
  const { data } = useSuspenseQuery(repoQuery({ owner, repo }));
  const ref = data.default_branch;
  const navigate = useNavigate();
  return (
    <>
      <div className="flex flex-wrap items-center gap-2">
        <RefPicker
          owner={owner}
          repo={repo}
          current={ref}
          onPick={(r) =>
            void navigate({ to: "/$owner/$repo/tree/$", params: { owner, repo, _splat: r } })
          }
        />
        <Region name="branch counts" fallback={null} hideNotFound>
          <RefCounts owner={owner} repo={repo} />
        </Region>
        <Link
          to="/$owner/$repo/tree/$"
          params={{ owner, repo, _splat: ref }}
          className="text-muted-foreground hover:text-foreground ml-auto text-xs hover:underline"
        >
          Browse files
        </Link>
      </div>
      <Panel flush>
        <Region name="files" rows={8}>
          <Directory owner={owner} repo={repo} ref={ref} path="" />
        </Region>
      </Panel>
      <Region name="README" rows={6}>
        <Readme owner={owner} repo={repo} />
      </Region>
    </>
  );
}

function RefCounts({ owner, repo }: Repo) {
  const { data: branches } = useSuspenseQuery(branchesQuery({ owner, repo }));
  const { data: tags } = useSuspenseQuery(tagsQuery({ owner, repo }));
  return (
    <span className="flex items-center gap-3 text-xs">
      <Link
        to="/$owner/$repo/branches"
        params={{ owner, repo }}
        className="flex items-center gap-1 hover:underline"
      >
        <HugeiconsIcon icon={GitBranchIcon} strokeWidth={2} className="size-3.5" />
        <strong>{branches.length}</strong> branches
      </Link>
      <Link
        to="/$owner/$repo/tags"
        params={{ owner, repo }}
        className="flex items-center gap-1 hover:underline"
      >
        <HugeiconsIcon icon={Tag01Icon} strokeWidth={2} className="size-3.5" />
        <strong>{tags.length}</strong> tags
      </Link>
    </span>
  );
}

function Readme({ owner, repo }: Repo) {
  const { data } = useSuspenseQuery(readmeQuery({ owner, repo }));
  return (
    <Panel
      title={
        <span className="flex items-center gap-1.5">
          <HugeiconsIcon icon={BookOpen01Icon} strokeWidth={2} className="size-4" />
          {data.name}
        </span>
      }
    >
      <div data-testid="readme">
        <MarkdownBody text={data.text} />
      </div>
    </Panel>
  );
}

function Languages({ owner, repo }: Repo) {
  const { data } = useSuspenseQuery(languagesQuery({ owner, repo }));
  const entries = Object.entries(data);
  const total = entries.reduce((n, [, b]) => n + b, 0);
  return (
    <Panel title="Languages">
      {total === 0 ? (
        <p className="text-muted-foreground text-sm">No code yet.</p>
      ) : (
        <div className="flex flex-col gap-2" data-testid="languages">
          <div
            className="flex h-2 overflow-hidden rounded-sm"
            role="img"
            aria-label="Language share"
          >
            {entries.map(([lang, bytes]) => (
              <span
                key={lang}
                style={{ width: `${(bytes / total) * 100}%`, backgroundColor: colorOf(lang) }}
              />
            ))}
          </div>
          <ul className="flex flex-wrap gap-x-4 gap-y-1 text-xs">
            {entries.map(([lang, bytes]) => (
              <li key={lang} className="flex items-center gap-1.5">
                <span className="size-2 rounded-sm" style={{ backgroundColor: colorOf(lang) }} />
                <span className="font-semibold">{lang}</span>
                <span className="text-muted-foreground">{((bytes / total) * 100).toFixed(1)}%</span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </Panel>
  );
}

function Contributors({ owner, repo }: Repo) {
  const { data } = useSuspenseQuery(contributorsQuery({ owner, repo }));
  return (
    <Panel
      title={
        <span>
          Contributors <span className="text-muted-foreground font-normal">{data.length}</span>
        </span>
      }
    >
      {data.length === 0 ? (
        <p className="text-muted-foreground text-sm">No commits yet.</p>
      ) : (
        <ul className="flex flex-col gap-1.5" data-testid="contributors">
          {data.map((c) => (
            <li key={c.login} className="flex items-center gap-2 text-sm">
              <UserAvatar
                user={c.login && c.avatar_url ? { login: c.login, avatar_url: c.avatar_url } : null}
              />
              <span className="font-medium">{c.login}</span>
              <span className="text-muted-foreground ml-auto text-xs">
                {c.contributions} commits
              </span>
            </li>
          ))}
          <li>
            <Link
              to="/$owner/$repo/pulse"
              params={{ owner, repo }}
              search={{ tab: "contributors" }}
              className="text-primary text-xs hover:underline"
            >
              See contributor graphs
            </Link>
          </li>
        </ul>
      )}
    </Panel>
  );
}

function CloneMenu({ owner, repo }: Repo) {
  const { data } = useSuspenseQuery(repoQuery({ owner, repo }));
  const [proto, setProto] = useState<"https" | "ssh" | "cli">("https");
  const urls = {
    https: data.clone_url,
    ssh: data.ssh_url,
    cli: `gh repo clone ${data.full_name}`,
  };
  return (
    <Popover>
      <PopoverTrigger
        render={
          <Button size="sm" aria-label="Clone this repository">
            <HugeiconsIcon icon={CodeIcon} strokeWidth={2} data-icon="inline-start" />
            Code
            <HugeiconsIcon icon={ArrowDown01Icon} strokeWidth={2} data-icon="inline-end" />
          </Button>
        }
      />
      <PopoverContent align="end" className="w-96 gap-2 p-3" data-testid="clone-menu">
        <p className="flex items-center gap-1.5 text-sm font-semibold">
          <HugeiconsIcon icon={TerminalIcon} strokeWidth={2} className="size-4" />
          Clone
        </p>
        <div className="flex border-b text-xs" role="tablist" aria-label="Clone protocol">
          {(
            [
              ["https", "HTTPS"],
              ["ssh", "SSH"],
              ["cli", "GitHub CLI"],
            ] as const
          ).map(([key, label]) => (
            <button
              key={key}
              type="button"
              role="tab"
              aria-selected={proto === key}
              onClick={() => setProto(key)}
              className="text-muted-foreground aria-selected:border-primary aria-selected:text-foreground -mb-px border-b-2 border-transparent px-2 py-1 font-medium"
            >
              {label}
            </button>
          ))}
        </div>
        <div className="flex items-center gap-1">
          <Input
            readOnly
            value={urls[proto]}
            className="font-mono text-xs"
            aria-label="Clone URL"
            onFocus={(e) => e.target.select()}
          />
          <CopyButton value={urls[proto]} label="Copy the clone URL" />
        </div>
        <p className="text-muted-foreground text-xs">
          {proto === "https"
            ? "Clone over HTTPS with a personal access token as the password."
            : proto === "ssh"
              ? "Use a key from your SSH keys settings."
              : "Works with gh pointed at this host: gh auth login --hostname ghes.graftyard.test"}
        </p>
        <div className="flex gap-2 border-t pt-2 text-xs">
          <a
            href={`${data.url}/zipball/${data.default_branch}`}
            className="text-primary hover:underline"
          >
            Download ZIP
          </a>
          <a
            href={`${data.url}/tarball/${data.default_branch}`}
            className="text-primary hover:underline"
          >
            Download tarball
          </a>
        </div>
      </PopoverContent>
    </Popover>
  );
}

// Plain literals, not the `slot()` helper: @/areas/slots imports this file eagerly, so a
// runtime import back from it runs before `slot` exists. The SlotEntry union still checks
// each Component against its slot's props.
export const slots: SlotEntry[] = [
  { slot: "repo-overview", region: "code", order: 0, Component: Overview },
  { slot: "repo-sidebar", region: "languages", order: 30, Component: Languages },
  { slot: "repo-sidebar", region: "contributors", order: 31, Component: Contributors },
  { slot: "repo-actions", region: "clone menu", order: 30, Component: CloneMenu },
];
