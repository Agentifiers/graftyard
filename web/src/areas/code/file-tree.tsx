import { useEffect, useMemo, useReducer, useRef } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import {
  createTree,
  hotkeysCoreFeature,
  selectionFeature,
  syncDataLoaderFeature,
  type TreeInstance,
} from "@headless-tree/core";
import { HugeiconsIcon } from "@hugeicons/react";
import { File02Icon, Folder01Icon, FolderOpenIcon } from "@hugeicons/core-free-icons";
import { Tree, TreeItem, TreeItemLabel } from "@/components/reui/tree";
import { gitTreeQuery } from "./queries";
import { joinPath, type Repo } from "./refs";

// The left-hand file tree on tree and blob pages: the whole tree at a commit, from
// GET /git/trees/{sha}?recursive=1, drawn with ReUI's Tree over headless-tree.

type Node = { name: string; path: string; dir: boolean };

const ROOT = "";

function build(entries: { path: string; type: string }[]) {
  const nodes = new Map<string, Node>([[ROOT, { name: "", path: ROOT, dir: true }]]);
  const children = new Map<string, string[]>([[ROOT, []]]);
  for (const e of entries) {
    const dir = e.type === "tree";
    nodes.set(e.path, { name: e.path.split("/").at(-1)!, path: e.path, dir });
    if (dir) children.set(e.path, []);
    const parent = e.path.includes("/") ? e.path.slice(0, e.path.lastIndexOf("/")) : ROOT;
    children.get(parent)?.push(e.path);
  }
  for (const list of children.values()) {
    list.sort((a, b) => {
      const da = nodes.get(a)!.dir;
      const dbb = nodes.get(b)!.dir;
      return da === dbb ? a.localeCompare(b) : da ? -1 : 1;
    });
  }
  return { nodes, children };
}

const ancestors = (path: string) =>
  path
    .split("/")
    .slice(0, -1)
    .map((_, i, parts) => parts.slice(0, i + 1).join("/"));

export function FileTree({
  owner,
  repo,
  ref,
  sha,
  path,
}: Repo & { ref: string; sha: string; path: string }) {
  const { data } = useSuspenseQuery(gitTreeQuery({ owner, repo, sha }));
  const { nodes, children } = useMemo(() => build(data.tree), [data]);
  const navigate = useNavigate();
  const [, rerender] = useReducer((n: number) => n + 1, 0);
  const treeRef = useRef<TreeInstance<Node> | null>(null);
  const open = isDirPath(nodes, path) ? [...ancestors(path), path] : ancestors(path);

  if (!treeRef.current) {
    treeRef.current = createTree<Node>({
      rootItemId: ROOT,
      getItemName: (item) => item.getItemData().name,
      isItemFolder: (item) => item.getItemData().dir,
      dataLoader: {
        getItem: (id) => nodes.get(id)!,
        getChildren: (id) => children.get(id) ?? [],
      },
      initialState: { expandedItems: open, selectedItems: [path], focusedItem: path || null },
      setState: () => rerender(),
      features: [syncDataLoaderFeature, selectionFeature, hotkeysCoreFeature],
    });
  }
  const tree = treeRef.current;

  useEffect(() => {
    tree.setMounted(true);
    tree.rebuildTree();
    return () => tree.setMounted(false);
  }, [tree]);

  // The tree follows the page: a new path selects itself and opens its parents.
  useEffect(() => {
    tree.setConfig((c) => ({
      ...c,
      dataLoader: { getItem: (id) => nodes.get(id)!, getChildren: (id) => children.get(id) ?? [] },
      onPrimaryAction: (item) => {
        const node = item.getItemData();
        void navigate({
          to: node.dir ? "/$owner/$repo/tree/$" : "/$owner/$repo/blob/$",
          params: { owner, repo, _splat: joinPath(ref, node.path) },
        });
      },
    }));
    const expanded = new Set([...tree.getState().expandedItems, ...open]);
    tree.applySubStateUpdate("expandedItems", [...expanded]);
    tree.applySubStateUpdate("selectedItems", [path]);
    tree.rebuildTree();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tree, nodes, children, path, ref, owner, repo]);

  return (
    <Tree indent={14} tree={tree} className="text-sm" data-testid="file-tree" aria-label="Files">
      {tree.getItems().map((item) => {
        const node = item.getItemData();
        if (!node) return null;
        return (
          <TreeItem key={item.getId()} item={item} className="w-full text-left">
            <TreeItemLabel className="rounded-sm px-1.5 py-1">
              <HugeiconsIcon
                icon={node.dir ? (item.isExpanded() ? FolderOpenIcon : Folder01Icon) : File02Icon}
                strokeWidth={2}
                className="text-muted-foreground size-4"
              />
              <span className="truncate" data-path={node.path}>
                {node.name}
              </span>
            </TreeItemLabel>
          </TreeItem>
        );
      })}
    </Tree>
  );
}

function isDirPath(nodes: Map<string, Node>, path: string) {
  return path === "" || nodes.get(path)?.dir === true;
}
