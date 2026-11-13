//! The `rest:issues`, `rest:pulls`, `rest:checks`, and `rest:commits` routes and the GraphQL resolvers that `gh pr`
//! and `gh issue` use (`build/BUILD-PLAN.md`, M2-WP2; `design/SHARED.md`, "Surfaces, tiers, and oracles"), as pure
//! request-to-response code over a repository's fold tiers. The edge Worker hosts them (`design/DESIGN.md`, "Stack":
//! the edge serves "HTTPS git, REST, GraphQL"); nothing here needs a Worker, so tests run them on `MemStore` tiers.
//!
//! M2-WP4 adds `lfs` over HTTPS, `rest:releases`, `uploads-release-assets`, `codeload-archives`, `meta-endpoint`,
//! `rest:meta`, `rest:rate-limit`, and `DELETE /git/refs/{ref}` (`lfs`, `release`, `rest`), with blobs in the store of
//! `graftyard_git::blob`.
//!
//! The scope is the requests a pinned gh and git-lfs send, captured in `gh/` and `lfs/` (see their READMEs), and
//! nothing beyond them.

pub mod agents;
pub mod exec;
pub mod gql;
pub mod graphql;
pub mod hosted;
pub mod leases;
pub mod lfs;
pub mod narrow;
pub mod release;
pub mod repos;
pub mod rest;
pub mod schema;
pub mod tasks;
pub mod trust;
pub mod ui;
pub mod view;
pub mod webhooks;
pub mod wire;
