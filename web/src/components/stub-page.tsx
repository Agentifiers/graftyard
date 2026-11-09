// A route another area has not built yet. It exists so links to it typecheck in every
// area's worktree; the owning area replaces the file. Delete this once no route uses it.
export function StubPage({ area, what }: { area: string; what: string }) {
  return (
    <div className="text-muted-foreground border p-6 text-sm" data-stub={area}>
      {what} is built by the {area} area.
    </div>
  );
}
