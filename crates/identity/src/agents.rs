//! Agent actors on the install's own `Authz` chain (`design/AGENT-LAYER.md`, Shape, "Agent actors in the layer:
//! GitHub Apps"; `design/DESIGN.md`, Shape, "Agent load and agent safety"): `AgentApp` rows, written only on
//! `Authz(Genesis.account)`, and the org's agent settings on `OrgPolicy`.

use graftyard_raft::Enc;
use graftyard_types::{AccountId, DbId};
use serde::{Deserialize, Serialize};

use crate::apps::{self, AppId};
use crate::glob::RefPattern;

/// Which agent an App is. `GraftyardRun` is one of Graftyard's own Apps (`graftyard-agent`, `graftyard-workspace`),
/// `ThirdParty` a vendor App an admin flagged, and `Mcp` the first-party `Graftyard MCP` App.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentKind {
    GraftyardRun,
    ThirdParty,
    Mcp,
}

/// An install setting. A `ThirdParty` App with `confine` is `ActorKind::Agent`, bound to that ref pattern; without it
/// it is a `Bot` whose commands take the Agent admission class.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentApp {
    pub app: AppId,
    pub kind: AgentKind,
    pub confine: Option<RefPattern>,
}

/// Where an org's agents work: a fork in `<org>-agents`, the default, or the repository itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkspaceMode {
    #[default]
    Fork,
    InRepo,
}

/// `Genesis.account`: the install account `graftyard genesis` writes, id 1 of cell 0's block 0, which `Ids` never
/// leases (`cli/src/placement.rs`, `install_account`) [inference: `Genesis.account` may name any block-0 account, and
/// the CLI always names this one]. `AgentApp` rows live on its `Authz` chain, which every App command reads.
pub fn install_account() -> AccountId {
    AccountId(DbId::pack(0, 0, 1).expect("block 0 fits"))
}

pub(crate) fn agent_app(e: &mut Enc, a: &AgentApp) {
    apps::app_id(e, a.app);
    e.u8(a.kind as u8);
    e.option(a.confine.as_ref(), |e, c| e.str(c.as_str()));
}
