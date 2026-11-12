//! `graftyard placement set <owner|default> <name> --sites ...` (`design/DESIGN.md`, "Usage (caller's view)"; Shape,
//! "Sites and placement", `PlacementRecord` and `Placement::new`).
//!
//! It builds the record, runs `validate_record` (`Placement::new`'s checks for a multi-site record, `Placement::solo`
//! with the install's signed `evaluation` flag for `Solo`) against the install's site table and the `Rtt` rows
//! `sites add` recorded, and emits the `Names` command the `Names` leader on `account_shard(owner)` would decide. With
//! no hosted `Names` group yet, it writes that command as a request file and says so.

use std::path::Path;

use graftyard_identity::{NamesCmd, NamesOp, account_shard, validate_record};
use graftyard_raft::Record;
use graftyard_types::{
    AccountId, ChainId, CredentialShard, DbId, PlacementChecks, PlacementName, PlacementRecord, SiteId,
};
use serde_json::json;
use smallvec::SmallVec;

use crate::config::{InstallToml, Res, hex, now};

/// The install account's id: the `Genesis.account` that `graftyard genesis` writes. Id 1 sits in cell 0's block 0,
/// which `Ids` never leases (it leases from block 1, `crates/identity`, `IdsFold::new`), so no user or org takes it.
pub fn install_account() -> AccountId {
    AccountId(DbId::pack(0, 0, 1).expect("block 0 fits"))
}

pub fn parse_name(s: &str) -> Res<PlacementName> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "tri" => PlacementName::Tri,
        "na3" => PlacementName::Na3,
        "us2w" => PlacementName::Us2w,
        "eu2w" => PlacementName::Eu2w,
        "apac2w" => PlacementName::Apac2w,
        "solo" => PlacementName::Solo,
        _ => return Err(format!("{s:?} is not a placement: tri, na3, us2w, eu2w, apac2w, or solo")),
    })
}

/// A site named by letter (`A` is site 0), id, or script name.
pub fn parse_site(t: &InstallToml, s: &str) -> Res<SiteId> {
    let s = s.trim();
    let id = if s.len() == 1 && s.as_bytes()[0].is_ascii_alphabetic() {
        SiteId(s.to_ascii_uppercase().as_bytes()[0] - b'A')
    } else if let Ok(n) = s.parse::<u8>() {
        SiteId(n)
    } else if let Some(site) = t.site_by_script(s) {
        site.id
    } else {
        return Err(format!("{s:?} is not a site of this install"));
    };
    match t.sites.iter().find(|x| x.id == id) {
        Some(site) if site.spare => {
            Err(format!("{s:?} is spare site {} ({}): a spare holds no owner", id.0, site.script))
        }
        Some(_) => Ok(id),
        None => Err(format!("{s:?} names site {}, which this install does not have", id.0)),
    }
}

pub struct Emitted {
    pub record: PlacementRecord,
    pub request: serde_json::Value,
}

/// Validates and builds the request. Pure apart from the clock, so the refusals are tested without the network.
pub fn build(
    t: &InstallToml,
    owner: &str,
    name: PlacementName,
    sites: &[SiteId],
    home: Option<SiteId>,
) -> Res<Emitted> {
    let (owner_id, op_name) = if owner == "default" {
        (install_account(), "default")
    } else if let Ok(n) = owner.parse::<i64>() {
        (AccountId(DbId(n)), "owner")
    } else {
        return Err(format!(
            "owner {owner:?}: `placement set` takes `default` or an account id; resolving a login needs the hosted Names \
             group (R14)"
        ));
    };
    let sites: SmallVec<[SiteId; 3]> = sites.iter().copied().collect();
    let record = PlacementRecord {
        name,
        home: home.unwrap_or(sites.first().copied().unwrap_or(SiteId(0))),
        sites,
        generation: 1,
        moving_from: None,
        at: now(),
    };
    let rtt = t.rtt_rows();
    let table = t.table();
    let checks = PlacementChecks { sites: &table, separate_accounts: t.separate_accounts(), rtt: &rtt, near: &[] };
    validate_record(owner_id, &record, t.evaluation, &checks).map_err(|e| format!("refused: {e}"))?;
    let cmd = NamesCmd {
        cred: CredentialShard::NONE,
        op: NamesOp::InitPlacement { owner: owner_id, record: record.clone() },
    };
    let mut bytes = Vec::new();
    cmd.encode(&mut bytes);
    let shard = account_shard(owner_id);
    let request = json!({
        "kind": "graftyard.names.command",
        "install": t.install,
        "chain": ChainId::Names(shard),
        "op": format!("InitPlacement ({op_name})"),
        "owner": owner_id.0.0,
        "record": record,
        "command_hex": hex(&bytes),
        "note": "No Names group is hosted yet (M1-WP7 hosting). This is the command the Names leader on `chain` \
                 decides. It is unsigned: Names commands carry a CredentialShard, not a signature, and the design \
                 names no operator credential for `placement set` (R14).",
    });
    Ok(Emitted { record, request })
}

pub fn set(config: &Path, owner: &str, name: &str, sites: &[String], home: Option<&str>, out: &Path) -> Res<()> {
    let mut t = InstallToml::load(config)?;
    let name = parse_name(name)?;
    let ids = sites.iter().map(|s| parse_site(&t, s)).collect::<Res<Vec<_>>>()?;
    let home = home.map(|h| parse_site(&t, h)).transpose()?;
    let e = build(&t, owner, name, &ids, home)?;
    std::fs::write(out, serde_json::to_string_pretty(&e.request).expect("json") + "\n")
        .map_err(|err| format!("{}: {err}", out.display()))?;
    println!(
        "placement {owner} {name:?} on sites {:?} home {} passed Placement::new's checks",
        ids.iter().map(|s| s.0).collect::<Vec<_>>(),
        e.record.home.0
    );
    println!(
        "no hosted Names group yet: wrote the request for the Names leader on {} to {}",
        e.request["chain"].as_str().unwrap_or(""),
        out.display()
    );
    if owner == "default" {
        t.default_placement = Some(e.record);
        t.save(config)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use graftyard_types::{Region, Site, SiteLink, Timestamp};

    use super::*;
    use crate::config::RttRow;

    fn site(id: u8, region: Region, spare: bool) -> Site {
        Site {
            id: SiteId(id),
            script: format!("graft-site-{}", (b'a' + id) as char),
            account: "acct".into(),
            region,
            jurisdiction: None,
            bucket: format!("b{id}"),
            release: None,
            link: SiteLink::Binding { member_class: "Member".into() },
            spare,
        }
    }

    fn install(evaluation: bool) -> InstallToml {
        let row = |from: u8, peer: u8, ms: u16| RttRow {
            from: SiteId(from),
            peer: SiteId(peer),
            median_ms: ms,
            at: Timestamp(1),
        };
        InstallToml {
            install: "01K8ZZZZZZZZZZZZZZZZZZZZZZ".into(),
            account: "acct".into(),
            zone: "git.acme.example".into(),
            customer_key: graftyard_release::public_key(&[7; 32]).hex(),
            evaluation,
            genesis: None,
            sites: vec![
                site(0, Region::Enam, false),
                site(1, Region::Weur, false),
                site(2, Region::Apac, false),
                site(3, Region::Wnam, false),
                site(4, Region::Eeur, true),
            ],
            rtt: vec![row(0, 1, 98), row(0, 2, 194), row(1, 2, 187), row(0, 3, 12), row(1, 3, 140), row(2, 3, 150)],
            default_placement: None,
        }
    }

    fn ids(t: &InstallToml, s: &[&str]) -> Vec<SiteId> {
        s.iter().map(|s| parse_site(t, s).unwrap()).collect()
    }

    #[test]
    fn tri_on_three_regions_is_emitted_for_the_names_leader() {
        let t = install(false);
        let e = build(&t, "default", PlacementName::Tri, &ids(&t, &["A", "B", "C"]), None).unwrap();
        assert_eq!(e.record.sites.as_slice(), &[SiteId(0), SiteId(1), SiteId(2)]);
        assert_eq!(e.request["chain"], json!(ChainId::Names(account_shard(install_account())).to_string()));
        assert!(e.request["command_hex"].as_str().unwrap().len() > 20);
    }

    #[test]
    fn two_sites_in_one_region_are_refused() {
        let mut t = install(false);
        t.sites[3].region = Region::Enam; // site D now shares enam with A
        let r = build(&t, "default", PlacementName::Tri, &ids(&t, &["A", "B", "D"]), None);
        assert!(r.as_ref().is_err_and(|e| e.contains("one region")), "{:?}", r.map(|_| ()));
        let r = build(&t, "default", PlacementName::Tri, &[SiteId(0), SiteId(1), SiteId(0)], None);
        assert!(r.is_err_and(|e| e.contains("twice")));
    }

    #[test]
    fn a_pair_under_min_pair_rtt_or_unmeasured_is_refused() {
        let t = install(false);
        // enam and wnam read 12 ms apart in these rows.
        let r = build(&t, "default", PlacementName::Na3, &ids(&t, &["A", "D", "B"]), None);
        assert!(r.is_err_and(|e| e.contains("under MIN_PAIR_RTT")));
        let mut t = install(false);
        t.rtt.retain(|r| !(r.from == SiteId(1) && r.peer == SiteId(2)));
        let r = build(&t, "default", PlacementName::Tri, &ids(&t, &["A", "B", "C"]), None);
        assert!(r.is_err_and(|e| e.contains("no round-trip measurement")));
    }

    #[test]
    fn solo_needs_the_evaluation_flag() {
        let t = install(false);
        let r = build(&t, "eval-org", PlacementName::Solo, &ids(&t, &["A"]), None);
        assert!(r.is_err(), "a login is not an account id");
        let r = build(&t, "42", PlacementName::Solo, &ids(&t, &["A"]), None);
        assert!(r.is_err_and(|e| e.contains("evaluation flag")));
        let t = install(true);
        assert!(build(&t, "42", PlacementName::Solo, &ids(&t, &["A"]), None).is_ok());
    }

    #[test]
    fn spares_unknown_sites_and_bad_homes_are_refused() {
        let t = install(false);
        assert!(parse_site(&t, "E").is_err_and(|e| e.contains("spare")));
        assert!(parse_site(&t, "Z").is_err());
        assert_eq!(parse_site(&t, "graft-site-b").unwrap(), SiteId(1));
        let r = build(&t, "default", PlacementName::Us2w, &ids(&t, &["A", "D", "B"]), Some(SiteId(1)));
        assert!(r.is_err(), "the witness slot cannot be home");
    }
}
