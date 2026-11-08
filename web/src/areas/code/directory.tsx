import { Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { File02Icon, Folder01Icon } from "@hugeicons/core-free-icons";
import { Ago } from "@/components/github";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { treeCommitsQuery } from "./queries";
import { firstLine, joinPath, LatestCommit, type Repo } from "./refs";

// A directory's entries, each with the last commit that touched it, from the private
// tree-commits read. The repository overview and the tree page both draw it.

export function Directory({ owner, repo, ref, path }: Repo & { ref: string; path: string }) {
  const { data } = useSuspenseQuery(treeCommitsQuery({ owner, repo, ref, path }));
  const parent = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
  if (data.entries.length === 0) {
    return (
      <Empty className="py-10">
        <EmptyHeader>
          <EmptyTitle>This directory is empty</EmptyTitle>
          <EmptyDescription>Add a file to start it.</EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  }
  return (
    <div className="divide-y" data-testid="directory">
      <LatestCommit
        owner={owner}
        repo={repo}
        commit={data.latest}
        count={data.count}
        historyTo={joinPath(ref, path)}
      />
      <table className="w-full table-fixed text-sm">
        <tbody className="divide-y">
          {path && (
            <tr className="hover:bg-muted/40">
              <td className="px-3 py-1.5" colSpan={3}>
                <Link
                  to="/$owner/$repo/tree/$"
                  params={{ owner, repo, _splat: joinPath(ref, parent) }}
                  className="text-primary font-mono hover:underline"
                  aria-label="Parent directory"
                >
                  ..
                </Link>
              </td>
            </tr>
          )}
          {data.entries.map((e) => (
            <tr key={e.path} className="hover:bg-muted/40" data-entry={e.name}>
              <td className="w-2/5 px-3 py-1.5">
                <span className="flex min-w-0 items-center gap-2">
                  <HugeiconsIcon
                    icon={e.type === "dir" ? Folder01Icon : File02Icon}
                    strokeWidth={2}
                    className={
                      e.type === "dir"
                        ? "size-4 shrink-0 text-sky-600 dark:text-sky-400"
                        : "text-muted-foreground size-4 shrink-0"
                    }
                    aria-label={e.type === "dir" ? "Directory" : "File"}
                  />
                  <Link
                    to={e.type === "dir" ? "/$owner/$repo/tree/$" : "/$owner/$repo/blob/$"}
                    params={{ owner, repo, _splat: joinPath(ref, e.path) }}
                    className="truncate hover:underline"
                  >
                    {e.name}
                  </Link>
                </span>
              </td>
              <td className="text-muted-foreground hidden truncate px-3 py-1.5 sm:table-cell">
                <Link
                  to="/$owner/$repo/commit/$sha"
                  params={{ owner, repo, sha: e.commit.sha }}
                  className="hover:text-foreground hover:underline"
                  title={e.commit.message}
                >
                  {firstLine(e.commit.message)}
                </Link>
              </td>
              <td className="text-muted-foreground w-28 px-3 py-1.5 text-right text-xs whitespace-nowrap">
                <Ago at={e.commit.date} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
