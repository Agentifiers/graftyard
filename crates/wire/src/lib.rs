//! The shared wire layer (`build/BUILD-PLAN.md`, M2-WP4; `design/SHARED.md`, "Cross-cutting conventions",
//! "Hostnames and endpoints", and "Client ids and object ids"). Pure: the edge and the API handlers call it, and it
//! holds every rule a response follows whatever surface rendered it.
//!
//! A handler returns a `Reply` with its status, body, and its own headers. `serve` admits the request (a
//! `User-Agent`, a known API version) and `finish` adds the cross-cutting headers, so no handler sets or skips them.

pub mod host;
pub mod ids;
pub mod page;
pub mod rate;
pub mod reply;
pub mod time;

pub use host::{Endpoint, Host, HostError};
pub use ids::{FirstPartyClient, NodeId, NodeType, database_id, full_database_id};
pub use page::{Page, cursor, link, parse_cursor};
pub use rate::{RateLimits, Window};
pub use reply::{
    ApiVersion, Body, ENTERPRISE_VERSION, FieldError, Incoming, MediaType, Meta, Reply, Surface, basic_error,
    counts_against_limit, etag, finish, not_modified, request_id_header, serve, validation_error,
};
pub use time::iso;
