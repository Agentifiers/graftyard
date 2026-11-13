use graftyard_types::{AccountId, DbId, PlacementRecord, Region, Site, SiteLink, Timestamp, Ulid};

use super::*;

mod group;

const CUSTOMER: [u8; 32] = [7; 32];
const OTHER: [u8; 32] = [8; 32];
const T0: Timestamp = Timestamp(1_790_000_000_000);

fn install(n: u8) -> InstallId {
    InstallId(Ulid::new(Timestamp(1_790_000_000_000), [n; 10]))
}

fn site(id: u8, region: Region, spare: bool) -> Site {
    Site {
        id: SiteId(id),
        script: format!("graft-site-{id}"),
        account: "acct".into(),
        region,
        jurisdiction: None,
        bucket: format!("graft-packs-{id}"),
        release: None,
        link: SiteLink::Binding { member_class: "Member".into() },
        spare,
    }
}

fn tri() -> PlacementRecord {
    PlacementRecord {
        name: PlacementName::Tri,
        sites: [SiteId(0), SiteId(1), SiteId(2)].into_iter().collect(),
        home: SiteId(0),
        generation: 1,
        moving_from: None,
        at: Timestamp(5),
    }
}

fn genesis_for(id: InstallId, customer: &[u8; 32]) -> Genesis {
    Genesis {
        install: id,
        account: AccountId(DbId(1)),
        keys: Keys {
            generation: 0,
            customer: public_key(customer),
            vendor: Some(public_key(&[2; 32])),
            release: public_key(&[3; 32]),
            drain: public_key(&[4; 32]),
            executor: public_key(&[5; 32]),
            agents_executor: None,
            ring: SmallVec::new(),
        },
        sites: [
            site(0, Region::Enam, false),
            site(1, Region::Weur, false),
            site(2, Region::Apac, false),
            site(3, Region::Eeur, true),
        ]
        .into_iter()
        .collect(),
        placements: [SystemChain::Release, SystemChain::Names, SystemChain::Tokens, SystemChain::Ids]
            .into_iter()
            .map(|c| (c, tri()))
            .collect(),
        evaluation: false,
        restore: None,
    }
}

fn unsigned(g: Genesis) -> SignedEntry {
    let bound = Bound { install: g.install, group: ChainId::Release, prev: None, parent: None };
    SignedEntry::unsigned(bound, ReleaseEntry::Genesis(Box::new(g)))
}

fn signed(g: Genesis, seed: &[u8; 32]) -> Vec<u8> {
    let mut e = unsigned(g);
    e.sign(seed);
    e.encode()
}

fn anchor() -> Anchor {
    Anchor { install: install(1), customer: public_key(&CUSTOMER) }
}

#[test]
fn a_signed_genesis_round_trips_and_is_accepted() {
    let g = genesis_for(install(1), &CUSTOMER);
    let bytes = signed(g.clone(), &CUSTOMER);
    let e = SignedEntry::decode(&bytes).unwrap();
    assert_eq!(e.encode(), bytes, "decode then encode gives the same bytes");
    assert_eq!(accept_genesis(&anchor(), &bytes), Ok(g));
}

#[test]
fn anchor_vars_parse_as_scripts_hold_them() {
    let a = anchor();
    let parsed = Anchor::from_vars(&a.install.to_string(), &a.customer.hex()).unwrap();
    assert_eq!(parsed, a);
    assert!(Anchor::from_vars("not-a-ulid", &a.customer.hex()).is_err());
    assert!(Anchor::from_vars(&a.install.to_string(), &a.customer.hex().to_uppercase()).is_err());
    assert!(Anchor::from_vars(&a.install.to_string(), &a.customer.hex()[..62]).is_err());
}

/// A bit flip in any byte of the signed bytes is refused: either the entry no longer decodes, or the signature no
/// longer verifies, or the anchor no longer matches. Byte i flips bit i % 8, so every byte and every bit position is
/// hit at an eighth of the cost of all flips (Ed25519 verification is slow in debug builds).
#[test]
fn a_bit_flip_in_any_byte_of_a_signed_genesis_is_refused() {
    let bytes = signed(genesis_for(install(1), &CUSTOMER), &CUSTOMER);
    for i in 0..bytes.len() {
        let mut t = bytes.clone();
        t[i] ^= 1 << (i % 8);
        assert!(accept_genesis(&anchor(), &t).is_err(), "flip of byte {i} bit {} was accepted", i % 8);
    }
}

#[test]
fn truncated_extended_and_foreign_bytes_are_malformed() {
    let bytes = signed(genesis_for(install(1), &CUSTOMER), &CUSTOMER);
    for n in [0, 1, DOMAIN.len(), bytes.len() / 2, bytes.len() - 1] {
        assert!(matches!(accept_genesis(&anchor(), &bytes[..n]), Err(Refusal::Malformed(_))), "prefix {n}");
    }
    let mut long = bytes.clone();
    long.push(0);
    assert!(matches!(accept_genesis(&anchor(), &long), Err(Refusal::Malformed(_))));
    assert!(matches!(accept_genesis(&anchor(), b"{\"genesis\":true}"), Err(Refusal::Malformed(_))));
    // A huge forged count is refused before anything is allocated.
    let mut forged = DOMAIN.to_vec();
    forged.extend_from_slice(&[0xff; 64]);
    assert!(matches!(accept_genesis(&anchor(), &forged), Err(Refusal::Malformed(_))));
}

#[test]
fn a_signature_by_another_key_is_refused() {
    // The genesis names the anchor's key but another key signed it.
    let bytes = signed(genesis_for(install(1), &CUSTOMER), &OTHER);
    assert_eq!(accept_genesis(&anchor(), &bytes), Err(Refusal::WrongKey));
    // Another key's genesis, signed by that key, consistent with itself.
    let bytes = signed(genesis_for(install(1), &OTHER), &OTHER);
    assert_eq!(accept_genesis(&anchor(), &bytes), Err(Refusal::WrongKey));
    // A second signature next to the right one.
    let mut e = unsigned(genesis_for(install(1), &CUSTOMER));
    e.sign(&CUSTOMER);
    e.sign(&OTHER);
    assert_eq!(accept_genesis(&anchor(), &e.encode()), Err(Refusal::WrongKey));
    // The anchor's key, but a signature lifted from another entry.
    let mut e = unsigned(genesis_for(install(1), &CUSTOMER));
    let mut other = unsigned(genesis_for(install(1), &CUSTOMER));
    other.body = ReleaseEntry::Keys(Box::new(genesis_for(install(1), &CUSTOMER).keys));
    other.sign(&CUSTOMER);
    e.sigs = other.sigs.clone();
    assert_eq!(accept_genesis(&anchor(), &e.encode()), Err(Refusal::BadSignature));
}

#[test]
fn a_genesis_signed_by_the_anchor_but_naming_another_customer_key_is_refused() {
    let mut g = genesis_for(install(1), &CUSTOMER);
    g.keys.customer = public_key(&OTHER);
    let bytes = signed(g, &CUSTOMER);
    assert_eq!(
        accept_genesis(&anchor(), &bytes),
        Err(Refusal::KeysMismatch { want: public_key(&CUSTOMER), got: public_key(&OTHER) })
    );
}

#[test]
fn a_genesis_for_another_install_is_refused() {
    let bytes = signed(genesis_for(install(2), &CUSTOMER), &CUSTOMER);
    assert_eq!(accept_genesis(&anchor(), &bytes), Err(Refusal::WrongInstall { want: install(1), got: install(2) }));
    // Bound to this install, body naming another.
    let mut e = unsigned(genesis_for(install(2), &CUSTOMER));
    e.bound.install = install(1);
    e.sign(&CUSTOMER);
    assert_eq!(
        accept_genesis(&anchor(), &e.encode()),
        Err(Refusal::WrongInstall { want: install(1), got: install(2) })
    );
}

#[test]
fn a_genesis_bound_to_another_group_or_position_is_refused() {
    let mut e = unsigned(genesis_for(install(1), &CUSTOMER));
    e.bound.group = ChainId::Ids;
    e.sign(&CUSTOMER);
    assert_eq!(accept_genesis(&anchor(), &e.encode()), Err(Refusal::WrongGroup(ChainId::Ids)));
    let mut e = unsigned(genesis_for(install(1), &CUSTOMER));
    e.bound.prev = Some(Digest([1; 32]));
    e.sign(&CUSTOMER);
    assert!(matches!(accept_genesis(&anchor(), &e.encode()), Err(Refusal::BadBound(_))));
    let mut e = unsigned(genesis_for(install(1), &CUSTOMER));
    e.bound.parent = Some(IntentId(Ulid(9)));
    e.sign(&CUSTOMER);
    assert!(matches!(accept_genesis(&anchor(), &e.encode()), Err(Refusal::BadBound(_))));
}

#[test]
fn unsigned_and_non_genesis_entries_are_refused() {
    let e = unsigned(genesis_for(install(1), &CUSTOMER));
    assert_eq!(accept_genesis(&anchor(), &e.encode()), Err(Refusal::WrongKey));
    let mut k = unsigned(genesis_for(install(1), &CUSTOMER));
    k.body = ReleaseEntry::Keys(Box::new(genesis_for(install(1), &CUSTOMER).keys));
    k.sign(&CUSTOMER);
    assert_eq!(accept_genesis(&anchor(), &k.encode()), Err(Refusal::NotGenesis("Keys")));
}

#[test]
fn genesis_body_rules() {
    let check = |f: &dyn Fn(&mut Genesis)| {
        let mut g = genesis_for(install(1), &CUSTOMER);
        f(&mut g);
        accept_genesis(&anchor(), &signed(g, &CUSTOMER)).map(|_| ())
    };
    assert_eq!(check(&|_| {}), Ok(()));
    assert_eq!(check(&|g| g.keys.generation = 1), Err(Refusal::KeysGeneration(1)));
    let refused = |r: Result<(), Refusal>| assert!(matches!(r, Err(Refusal::BadGenesis(_))), "{r:?}");
    refused(check(&|g| g.keys.drain = g.keys.executor));
    refused(check(&|g| g.keys.vendor = Some(g.keys.customer)));
    refused(check(&|g| {
        g.placements.pop();
    }));
    refused(check(&|g| g.placements[1].0 = SystemChain::Release));
    refused(check(&|g| g.placements[0].1.sites[2] = SiteId(3))); // a spare
    refused(check(&|g| g.placements[0].1.sites[2] = SiteId(9))); // unknown
    refused(check(&|g| g.sites[1].region = Region::Enam)); // two voters in enam
    refused(check(&|g| g.sites[3].id = SiteId(0)));
    refused(check(&|g| g.placements[0].1.moving_from = Some(Box::new(tri()))));
    refused(check(&|g| {
        g.restore = Some(RestoreFrom { source: RestoreSource::Lifeboat { host: "x".into() }, cutoff: Timestamp(1) })
    }));
    let solo = |g: &mut Genesis| {
        for (_, r) in g.placements.iter_mut() {
            *r = PlacementRecord { name: PlacementName::Solo, sites: [SiteId(0)].into_iter().collect(), ..tri() };
        }
    };
    refused(check(&solo));
    assert_eq!(
        check(&|g| {
            solo(g);
            g.evaluation = true;
        }),
        Ok(())
    );
}

#[test]
fn the_group_accepts_one_genesis_and_refuses_a_replay_or_a_second() {
    let a = anchor();
    let mut fold = ReleaseFold::default();
    let bytes = signed(genesis_for(install(1), &CUSTOMER), &CUSTOMER);
    let ev = fold.decide(&a, T0, &bytes).unwrap();
    assert_eq!(&*ev.bytes, &bytes[..]);
    fold.check(&a, T0, &ev).unwrap();
    fold.apply(&ev, T0);
    let digest = SignedEntry::decode(&bytes).unwrap().digest();
    assert_eq!(fold.last, Some(digest));
    assert_eq!(fold.keys.as_ref().map(|k| k.customer), Some(a.customer));
    // The same bytes again, and a different valid genesis for the same install.
    assert_eq!(fold.decide(&a, T0, &bytes), Err(Refusal::AlreadyInstalled(digest)));
    let mut g2 = genesis_for(install(1), &CUSTOMER);
    g2.account = AccountId(DbId(2));
    assert_eq!(fold.decide(&a, T0, &signed(g2, &CUSTOMER)), Err(Refusal::AlreadyInstalled(digest)));
    // A follower that already applied it refuses the event again.
    assert_eq!(fold.check(&a, T0, &ev), Err(Refusal::AlreadyInstalled(digest)));
    assert_eq!(Refusal::AlreadyInstalled(digest).status(), 409);
}

#[test]
fn signing_is_deterministic_and_resigning_replaces() {
    let mut a = unsigned(genesis_for(install(1), &CUSTOMER));
    let mut b = a.clone();
    a.sign(&CUSTOMER);
    b.sign(&CUSTOMER);
    b.sign(&CUSTOMER);
    assert_eq!(a.encode(), b.encode());
    assert_eq!(a.sigs.len(), 1);
}

#[test]
fn refusal_variants_are_stable_names() {
    assert_eq!(Refusal::WrongKey.variant(), "WrongKey");
    assert_eq!(Refusal::AlreadyInstalled(Digest([0; 32])).variant(), "AlreadyInstalled");
}
