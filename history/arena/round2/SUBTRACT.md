# What a global namespace makes obsolete in DESIGN.md

Subtract these before adding anything. Section names refer to `DESIGN.md`.

| Obsolete | Where | Why |
| --- | --- | --- |
| One GHES hostname per managed tenant (`TenantHost`, the host-keyed `TenantMap`, SNI routing by tenant, `<label>.tributary.cloud`) | Summary, Usage, "Tenancy and cell placement", "Deployment modes", wire text on hostnames | Managed is one public host. Clients still see a GHES-shaped host, because the host is not `github.com` or `*.ghe.com`. |
| A per-tenant NLB and SSH port per tenant, and per-tenant host keys (ACT-6) | "Tenancy and cell placement", `git-ssh` row, Tradeoffs, Open questions | With one namespace, SSH keys are unique across the service and the repository path names one repository, so SSH routes like github.com does. ACT-6's capture attack needed two namespaces in one cell. |
| `TenantId` as the leading key of every table, and "ids are tenant-local" | Every table description, `RepoKey`, `id_block(tenant, kind)` | Users, orgs, and repositories are global. Ids must be unique across every cell. |
| Tenant migration (`migrate_tenant`, `TenantState` keyed by tenant) | "Tenancy and cell placement", ACT-3 | The unit that moves between cells changes. The fence pattern from ACT-3 survives, keyed by the new unit. |
| Per-tenant signing keys that move with the tenant (ACT-9) | "Identity and tokens", OIDC paragraph | GitHub Apps install across owners, so any cell may need to verify any installation token. |
| First-party OAuth apps "seeded into every tenant" | "Identity and tokens", wire text | Seeded once for the service. |
| SAML and OIDC "per tenant" | "Identity and tokens" | On a shared namespace, SSO attaches to orgs and enterprise accounts, as on GitHub Enterprise Cloud. |
| A per-tenant credential and permission snapshot for stale fetches (ACT-14) | "Repository storage and ref transactions", "Identity and tokens" | The snapshot's unit changes with identity's new home. |
| "No global user graph, no cross-tenant fork, no global router" | "Tenancy and cell placement", "Alternatives considered" | The operator now requires all three. |
