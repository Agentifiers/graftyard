//! The `rest:issues`, `rest:pulls`, `rest:checks`, and `rest:commits` routes and the GraphQL resolvers that `gh pr`
//! and `gh issue` use (`build/BUILD-PLAN.md`, M2-WP2; `design/SHARED.md`, "Surfaces, tiers, and oracles"), as pure
//! request-to-response code over a repository's fold tiers. The edge Worker hosts them (`design/DESIGN.md`, "Stack":
//! the edge serves "HTTPS git, REST, GraphQL"); nothing here needs a Worker, so tests run them on `MemStore` tiers.
//!
//! The scope is the requests a pinned gh sends, captured in `gh/` (see `gh/README.md`), and nothing beyond them.

pub mod exec;
pub mod gql;
pub mod graphql;
pub mod rest;
pub mod view;
pub mod wire;
