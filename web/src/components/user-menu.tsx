import { Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { navGroups } from "@/areas/nav";
import { userQuery } from "@/areas/core/queries";
import { UserAvatar } from "@/components/github";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

// The avatar at the top right and its menu, built from every area's "user-menu" entries.
// Entries may use $owner, which is filled with the viewer's login.
export function UserMenu() {
  const { data: user } = useSuspenseQuery(userQuery());
  const groups = navGroups("user-menu");
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        className="hover:bg-muted flex items-center gap-2 rounded-sm px-1 py-0.5 text-sm"
        aria-label="Open user menu"
      >
        <UserAvatar user={user} />
        <span className="text-muted-foreground hidden sm:inline">{user.login}</span>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56">
        <DropdownMenuGroup>
          <DropdownMenuLabel>
            Signed in as <span className="text-foreground font-semibold">{user.login}</span>
          </DropdownMenuLabel>
        </DropdownMenuGroup>
        {groups.map(({ group, entries }) => (
          <DropdownMenuGroup key={group ?? ""}>
            <DropdownMenuSeparator />
            {entries.map((e) => (
              <DropdownMenuItem
                key={e.to}
                render={<Link to={e.to as never} params={{ owner: user.login } as never} />}
              >
                <HugeiconsIcon icon={e.icon} strokeWidth={2} />
                {e.label}
              </DropdownMenuItem>
            ))}
          </DropdownMenuGroup>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
