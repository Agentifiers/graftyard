//! The lifeboat's pure half (`design/DESIGN.md`, "Install operations", Lifeboat; Shape, "Lifeboat feed";
//! `decisions/DECISIONS.md`, "`lifeboat-learner`").
//!
//! The lifeboat keeps a read-only copy of an install off Cloudflare: each chain is a learner of the site script's own
//! member classes, fed committed rows by an importer (`graftyard_raft::import`) and never by a leader. This crate
//! decides what the bundle does and holds no I/O:
//!
//! - `puller`: where the next rows come from. The R2 feed while it is fresh; once its newest object is older than
//!   `FEED_STALE_MS`, each known chain's tail from the edge's export endpoints, signed with the lifeboat key
//!   (`graftyard_identity::lifeboat`), which serve from followers and learners and never the leader; back to the feed
//!   once it moves again. Every chain's rows stay contiguous whichever source gave them, and a halted chain stays
//!   halted.
//! - `status`: `LifeboatStatus` and the `X-Graftyard-Lifeboat-Lag` header every response carries.
//! - `blob`: `graft-blob`'s durable create-only write and its checked reads and scrub, over the `Fs` trait.
//! - `acme`: when to renew the certificate through DNS-01, and the challenge record's name.

pub mod acme;
pub mod blob;
pub mod puller;
pub mod status;

/// The puller's one Durable Object in the bundle, which the front asks for the copy's status.
pub const PULLER_OBJECT: &str = "puller";
