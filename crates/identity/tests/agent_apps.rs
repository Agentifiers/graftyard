//! The install-owned agent Apps' setup (`tests/am1/rest/apps.mjs`) on real `graftyard_raft::Member`s: the exact JSON
//! sequence the script sends, read from `apps.mjs --emit`, commits on the install's `Authz(Genesis.account)` chain and
//! the `Names` shards, and leaves `graftyard-workspace` and `graftyard-agent` as `GraftyardRun` and `Graftyard MCP` as
//! `Mcp` App rows the install owns, each with its record, routes, key, and an installation on the org. A second run
//! changes nothing. Without the record first, `PutAgentApp` is refused `NotOwnApp`, as `tests/am1/results/am1-fix.md`
//! found on graft-m1.

mod common;

use common::world::{FreshLeader, OPS, World};
use graftyard_identity::{
    AccountKind, AgentKind, AppId, AuthzCmd, AuthzErr, AuthzOp, AuthzRead, ClientId, InstallationId, NameKey, NamesCmd,
    NamesOut, NamesRead, OrgRole, Slug, UserId, install_account,
};
use graftyard_raft::CommitError;
use graftyard_types::{AccountId, ChainId, DbId};

const ORG: AccountId = AccountId(DbId(7_100_001));

fn emit(ids: &[i64]) -> Vec<(ChainId, serde_json::Value)> {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/am1/rest/apps.mjs");
    let mut args = vec![script.to_owned(), "--emit".into(), ORG.0.0.to_string()];
    args.extend(ids.iter().map(i64::to_string));
    let out = std::process::Command::new("node").args(&args).output().expect("node runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    v.into_iter().map(|c| (c["chain"].as_str().unwrap().parse().unwrap(), c["cmd"].clone())).collect()
}

/// Commits one command of the sequence; `Some(App)` for a slug claim.
fn send(w: &mut World, chain: ChainId, cmd: serde_json::Value) -> Option<AppId> {
    match chain {
        ChainId::Authz(o) => {
            let c: AuthzCmd = serde_json::from_value(cmd).unwrap();
            match w.authz_cmd(o, c) {
                Ok(_) | Err(CommitError::Refused { err: AuthzErr::AlreadyOpened, .. }) => None,
                e => panic!("{chain}: {e:?}"),
            }
        }
        ChainId::Names(_) => {
            let c: NamesCmd = serde_json::from_value(cmd).unwrap();
            match w.names_cmd(chain, c).unwrap().out {
                NamesOut::App(a) => Some(a),
                _ => None,
            }
        }
        other => panic!("{other}"),
    }
}

#[test]
fn the_setup_registers_three_install_owned_agent_apps_and_a_rerun_changes_nothing() {
    let mut w = World::default();
    // The org the Apps install on, with an admin.
    w.authz_cmd(ORG, AuthzCmd { cred: OPS, op: AuthzOp::Open { kind: AccountKind::Org, enterprise: None } }).unwrap();
    let op = AuthzOp::SetMember { user: UserId(AccountId(DbId(7_100_003))), role: Some(OrgRole::Admin) };
    w.authz_cmd(ORG, AuthzCmd { cred: OPS, op }).unwrap();
    // A `GraftyardRun` row before the install owns the App is refused.
    let early = graftyard_identity::AgentApp { app: AppId(DbId(77)), kind: AgentKind::GraftyardRun, confine: None };
    w.authz_cmd(
        install_account(),
        AuthzCmd { cred: OPS, op: AuthzOp::Open { kind: AccountKind::Org, enterprise: None } },
    )
    .unwrap();
    match w.authz_cmd(install_account(), AuthzCmd { cred: OPS, op: AuthzOp::PutAgentApp(early) }) {
        Err(CommitError::Refused { err: AuthzErr::NotOwnApp, .. }) => {}
        e => panic!("{e:?}"),
    }
    // The slug claims answer the App ids; the rest of the sequence names them.
    let plan = emit(&[0, 0, 0]);
    let mut ids = Vec::new();
    for (chain, cmd) in plan.iter().filter(|(_, c)| c["op"].get("ClaimSlug").is_some()) {
        ids.push(send(&mut w, *chain, cmd.clone()).expect("an App id").0.0);
    }
    let plan = emit(&ids);
    assert_eq!(plan.len(), 1 + 3 * 7);
    for (chain, cmd) in plan.iter().filter(|(_, c)| c["op"].get("ClaimSlug").is_none()) {
        send(&mut w, *chain, cmd.clone());
    }
    let install = w.authz(install_account()).fresh_leader();
    let org = w.authz(ORG).fresh_leader();
    let kinds = [AgentKind::GraftyardRun, AgentKind::GraftyardRun, AgentKind::Mcp];
    for ((slug, id), kind) in ["graftyard-workspace", "graftyard-agent", "graftyard-mcp"].iter().zip(&ids).zip(kinds) {
        let app = AppId(DbId(*id));
        let rec = install.app(app).unwrap_or_else(|| panic!("{slug}: no record on authz:1"));
        assert_eq!((rec.slug.as_str(), rec.client, rec.keys.len()), (*slug, ClientId::for_app(app), 1));
        assert_eq!(install.agent_app(app).map(|a| a.kind), Some(kind), "{slug}");
        let names = |w: &mut World, k: NameKey| w.names_fold(k);
        assert_eq!(names(&mut w, NameKey::App(app)).app_owner(app), Some(install_account()), "{slug}");
        let client = ClientId::for_app(app);
        assert!(names(&mut w, NameKey::Client(client)).client(&client).is_some(), "{slug}");
        let s = Slug::parse(slug).unwrap();
        assert_eq!(names(&mut w, NameKey::AppSlug(s.clone())).slug(&s).map(|r| r.app), Some(app));
        let inst = org.more.installations.iter().find(|(_, st)| st.row.as_ref().is_some_and(|r| r.app == app));
        let (iid, st) = inst.unwrap_or_else(|| panic!("{slug}: not installed on the org"));
        assert_eq!(st.row.as_ref().unwrap().permissions, rec.permissions, "{slug}: installed with the App's grant");
        let iid: InstallationId = *iid;
        assert_eq!(names(&mut w, NameKey::Installation(iid)).installation(iid), Some((app, ORG)));
    }
    // `graftyard-workspace` alone has `contents: write` among the Graftyard-run pair, as AGENT-LAYER's table says.
    let ws = install.app(AppId(DbId(ids[0]))).unwrap();
    assert_eq!(ws.permissions.0.get("contents"), Some(&graftyard_identity::Access::Write));
    let ag = install.app(AppId(DbId(ids[1]))).unwrap();
    assert_eq!(ag.permissions.0.get("contents"), Some(&graftyard_identity::Access::Read));
    // A rerun: every command is decided again and answers with no event, so no chain's log gains an `Events` entry.
    fn count<E>(log: &[graftyard_raft::LogEntry<E>]) -> usize {
        log.iter().filter(|e| matches!(&e.body, graftyard_raft::EntryBody::Events(v) if !v.is_empty())).count()
    }
    let events = |w: &mut World| -> usize {
        let mut n = 0;
        for g in w.authz.values() {
            n += count(&g.disks[0].log);
        }
        for g in w.names.values() {
            n += count(&g.disks[0].log);
        }
        n
    };
    let before = events(&mut w);
    for (chain, cmd) in plan.iter().filter(|(_, c)| c["op"].get("ClaimSlug").is_none()) {
        send(&mut w, *chain, cmd.clone());
    }
    assert_eq!(events(&mut w), before, "a rerun wrote an event");
    assert_eq!(
        w.authz(install_account()).fresh_leader().agent_app(AppId(DbId(ids[2]))),
        install.agent_app(AppId(DbId(ids[2])))
    );
}
