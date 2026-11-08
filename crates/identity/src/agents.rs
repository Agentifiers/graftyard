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

/// A task's id (`design/DESIGN.md`, Shape, agent block). A `GraftyardRun` task's is a host-minted UUID; an `External`
/// task's is a name its App chose, 1 to 64 bytes of `[a-z0-9._-]`, starting with a letter or digit, with no `..` and no
/// `.lock` suffix. Its string form is the lowercase hyphenated UUID, or `<app_id>.<name>`. It lives here, beside
/// `AgentApp`, because an installation token's `task` claim carries one (`crate::stateless::InstallationClaims`).
///
/// In JSON (the issue-command and grant wires, the API) it is its string form, since a JSON number cannot hold a UUID;
/// in postcard (the fold's rows) it keeps the derived enum form.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SessionId {
    Run(u128),
    External { app: AppId, name: Box<str> },
}

/// `SessionId`'s derived form, which postcard encodes.
#[derive(Serialize, Deserialize)]
#[serde(rename = "SessionId")]
enum SessionRepr {
    Run(u128),
    External { app: AppId, name: Box<str> },
}

impl Serialize for SessionId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            return s.serialize_str(&self.text());
        }
        match self.clone() {
            SessionId::Run(u) => SessionRepr::Run(u),
            SessionId::External { app, name } => SessionRepr::External { app, name },
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<SessionId, D::Error> {
        if d.is_human_readable() {
            let t = String::deserialize(d)?;
            return SessionId::parse(&t, None).ok_or_else(|| serde::de::Error::custom("not a session id"));
        }
        Ok(match SessionRepr::deserialize(d)? {
            SessionRepr::Run(u) => SessionId::Run(u),
            SessionRepr::External { app, name } => SessionId::External { app, name },
        })
    }
}

impl SessionId {
    /// A session's string form. A bare `name` is scoped to `caller`, as the host scopes an App's own name.
    pub fn parse(s: &str, caller: Option<AppId>) -> Option<SessionId> {
        if let Some(u) = parse_uuid(s) {
            return Some(SessionId::Run(u));
        }
        let (app, name) = match s.split_once('.') {
            Some((a, n)) if !a.is_empty() && a.bytes().all(|b| b.is_ascii_digit()) => (AppId(DbId(a.parse().ok()?)), n),
            _ => (caller?, s),
        };
        let ok = (1..=64).contains(&name.len())
            && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
            && name.as_bytes()[0].is_ascii_alphanumeric()
            && !name.contains("..")
            && !name.ends_with(".lock");
        ok.then(|| SessionId::External { app, name: name.into() })
    }

    pub fn text(&self) -> String {
        match self {
            SessionId::Run(u) => {
                let h = format!("{u:032x}");
                format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
            }
            SessionId::External { app, name } => format!("{}.{name}", app.0.0),
        }
    }
}

fn parse_uuid(s: &str) -> Option<u128> {
    let b = s.as_bytes();
    let dashes = [8, 13, 18, 23];
    if b.len() != 36 || dashes.iter().any(|i| b[*i] != b'-') {
        return None;
    }
    let hex: String = s.chars().filter(|c| *c != '-').collect();
    if !hex.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
        return None;
    }
    u128::from_str_radix(&hex, 16).ok()
}

pub(crate) fn agent_app(e: &mut Enc, a: &AgentApp) {
    apps::app_id(e, a.app);
    e.u8(a.kind as u8);
    e.option(a.confine.as_ref(), |e, c| e.str(c.as_str()));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JSON carries a session as its string form, which a UUID needs; postcard keeps the enum form AM1-WP2's
    /// provenance rows already hold.
    #[test]
    fn a_session_is_its_string_in_json_and_its_enum_in_postcard() {
        let run = SessionId::parse("0193a1b2-c3d4-8e5f-8a9b-0c1d2e3f4a5b", None).unwrap();
        let ext = SessionId::parse("3.fix-1", None).unwrap();
        for s in [run.clone(), ext] {
            let j = serde_json::to_value(&s).unwrap();
            assert_eq!(j, serde_json::Value::String(s.text()));
            assert_eq!(serde_json::from_value::<SessionId>(j).unwrap(), s);
            let b = postcard::to_allocvec(&s).unwrap();
            assert_eq!(postcard::from_bytes::<SessionId>(&b).unwrap(), s);
        }
        let b = postcard::to_allocvec(&run).unwrap();
        assert_eq!(b[0], 0, "the Run variant's tag, as the derived form wrote it");
        assert!(serde_json::from_value::<SessionId>(serde_json::json!("not a session")).is_err());
    }
}
