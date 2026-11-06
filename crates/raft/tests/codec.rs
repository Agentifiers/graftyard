//! The crate's one storage and wire encoding (`graftyard_raft::codec`): every stored and sent type round-trips, and
//! the digest's canonical encoding is unchanged by it.

mod common;

use common::kv::{KV_FORMAT, KvEvent};
use common::sim::{CHAIN, record};
use graftyard_raft::codec::{CODEC_VERSION, from_bytes, to_bytes};
use graftyard_raft::{
    AppendReq, AppendRes, Deny, Digest, EntryBody, Envelope, ExtendLeases, GroupConfig, Index, LogEntry, MemberState,
    Message, Refusal, ReleaseTag, RequestId, Term, TierSet, TransferId, Ulid, VoteReq, VoteRes, bootstrap, digest,
};
use graftyard_types::{Placement, ReleaseId, SiteId, Timestamp};
use smallvec::SmallVec;

type Entry = LogEntry<KvEvent>;

fn hex(d: Digest) -> String {
    d.hex()
}

fn entries() -> Vec<Entry> {
    let boot: Entry = bootstrap(CHAIN, &record()).unwrap();
    let p = Box::new(Placement::from_record(CHAIN, &record()).unwrap());
    let events = LogEntry {
        term: Term(3),
        index: Index(2),
        prev: digest(&boot),
        format: KV_FORMAT,
        tiers: TierSet::CORE,
        at: Timestamp(123_456_789),
        requests: [(RequestId(Ulid(0x0123_4567_89ab_cdef_0011_2233_4455_6677)), 0..1), (RequestId(Ulid(7)), 1..2)]
            .into_iter()
            .collect(),
        extend: Some(ExtendLeases { since: Timestamp(5), by: std::time::Duration::from_millis(1500) }),
        body: EntryBody::Events(vec![
            KvEvent::Set { key: "k".into(), value: "v".into(), expect: Some(None) },
            KvEvent::Push { key: "k".into(), s: "x".into() },
        ]),
    };
    let joint = LogEntry {
        body: EntryBody::Config(GroupConfig::Joint { old: p.clone(), new: p.clone() }),
        requests: SmallVec::new(),
        extend: None,
        ..events.clone()
    };
    let degraded =
        LogEntry { body: EntryBody::Config(GroupConfig::Degraded { placement: p, lost: SiteId(2) }), ..joint.clone() };
    let noop = LogEntry { body: EntryBody::Noop, ..joint.clone() };
    vec![boot, events, joint, degraded, noop]
}

/// The digest's canonical bytes are `encode_entry`'s, not the codec's: these two values were computed before the
/// codec existed, with the crate's own `RequestId(u128)`, and must not move.
#[test]
fn the_digest_encoding_is_unchanged() {
    let e = entries();
    assert_eq!(hex(digest(&e[0])), "816b1965ba91df69efc60fa3573bdeb83622a683e1aa6dead82ac8d9183c2db6");
    assert_eq!(hex(digest(&e[1])), "449c08167fbbb78a75923120407da5fbaa557770351d34f0d4221e4991ff3970");
}

#[test]
fn entries_round_trip_to_the_same_digest() {
    for e in entries() {
        let back: Entry = from_bytes(&to_bytes(&e)).unwrap();
        assert_eq!(back, e);
        assert_eq!(digest(&back), digest(&e));
    }
}

#[test]
fn state_and_every_message_round_trip() {
    let mut s = MemberState::fresh();
    s.voted_for = Some(SiteId(2));
    s.led_term = Some(Term(4));
    s.no_lead_until = Some(ReleaseId([3; 32]));
    s.cannot_follow.push(SiteId(1));
    s.transfer = Some(TransferId(11));
    s.stalled_since = Some(Timestamp(12));
    s.boots = [Timestamp(1), Timestamp(2)];
    s.stepped_down_at = Some(Timestamp(13));
    s.promised_to = Some(SiteId(0));
    s.saved_at = Timestamp(14);
    s.transfer_at = Timestamp(15);
    s.lead_seq = 16;
    s.follow_seq = 17;
    assert_eq!(from_bytes::<MemberState>(&to_bytes(&s)).unwrap(), s);

    let rel = ReleaseTag { id: ReleaseId([4; 32]), seq: 2 };
    let msgs = vec![
        Message::Append(AppendReq {
            term: Term(5),
            leader: SiteId(0),
            boot: u64::MAX,
            round: 6,
            sent_at: Timestamp(7),
            prev_index: Index(1),
            prev_term: Term(1),
            prev_digest: Digest([9; 32]),
            entries: entries(),
            commit: Index(1),
            transfer: Some((SiteId(1), TransferId(3))),
            lead_seq: 4,
        }),
        Message::AppendRes(AppendRes {
            term: Term(5),
            boot: 8,
            round: 6,
            sent_at: Timestamp(7),
            prev_index: Index(1),
            ok: false,
            matched: Index(1),
            last: Index(4),
            promise: true,
            refused: Some(Refusal { release: rel, index: Index(3), reason: "no".into() }),
            stepped_down_at: Some(Timestamp(2)),
            boots: [Timestamp(3), Timestamp(4)],
            release: rel,
        }),
        Message::Vote(VoteReq {
            term: Term(5),
            candidate: SiteId(2),
            pre: true,
            last_index: Index(3),
            last_term: Term(2),
            transfer: Some(TransferId(1)),
            boots: [Timestamp(0), Timestamp(9)],
            stepped_down_at: None,
            bootstrapped: true,
        }),
        Message::VoteRes(VoteRes {
            term: Term(5),
            asked: Term(6),
            pre: false,
            granted: false,
            deny: Some(Deny::Suspected),
        }),
        Message::Propose(entries().remove(0)),
        Message::Proposed { stored: true },
        Message::TimeoutNow { term: Term(5), transfer: TransferId(2) },
        Message::TransferRefused { term: Term(5), transfer: TransferId(2) },
        Message::Probe { term: Term(5), id: 3, target: SiteId(1) },
        Message::ProbeRes { term: Term(5), id: 3, target: SiteId(1), failed: 2, of: 20 },
        Message::Ping { probe: 4, n: 1 },
        Message::Pong { probe: 4, n: 1 },
        Message::Install(graftyard_raft::InstallSnapshot {
            term: Term(5),
            boot: 8,
            round: 6,
            sent_at: Timestamp(7),
            lead_seq: 4,
            source: graftyard_raft::InstallSource {
                core: "snapshot/core".into(),
                metadata: None,
                archives: ["archive/1-9".into()].into_iter().collect(),
                through: Index(9),
                through_term: Term(4),
                through_digest: Digest([3; 32]),
                config: (
                    Index(1),
                    graftyard_raft::bootstrap::<KvEvent>(common::sim::CHAIN, &common::sim::record())
                        .map(|e| match e.body {
                            EntryBody::Config(c) => c,
                            _ => unreachable!(),
                        })
                        .unwrap(),
                ),
                id_block: graftyard_raft::IdBlock::whole(2, 9),
            },
        }),
        Message::ReadIndex { batch: 3 },
        Message::ReadIndexRes { batch: 3, index: Some((Index(9), Digest([4; 32]))) },
        Message::ReadIndexRes { batch: 4, index: None },
    ];
    let envs: Vec<Envelope<KvEvent>> =
        msgs.into_iter().map(|msg| Envelope { from: SiteId(0), to: SiteId(2), msg }).collect();
    assert_eq!(from_bytes::<Vec<Envelope<KvEvent>>>(&to_bytes(&envs)).unwrap(), envs);
}

#[test]
fn another_version_truncation_and_trailing_bytes_are_refused() {
    let s = MemberState::fresh();
    let b = to_bytes(&s);
    assert_eq!(b[0], CODEC_VERSION);
    let mut other = b.clone();
    other[0] = CODEC_VERSION + 1;
    assert!(from_bytes::<MemberState>(&other).is_err());
    assert!(from_bytes::<MemberState>(&b[..b.len() - 1]).is_err());
    let mut long = b.clone();
    long.push(0);
    assert!(from_bytes::<MemberState>(&long).is_err());
    assert!(from_bytes::<MemberState>(&[]).is_err());
}

/// `EntryBody::IdBlock` encodes as tag 3, then the base as `u64` and the length as `u32`, and round-trips the codec.
#[test]
fn id_block_entries_encode_with_tag_3() {
    let e = entries();
    let b = graftyard_raft::IdBlock::whole(2, 9).unwrap();
    let entry = LogEntry { body: EntryBody::IdBlock(b), requests: SmallVec::new(), extend: None, ..e[1].clone() };
    let bytes = graftyard_raft::encode_entry(&entry);
    let mut tail = vec![3u8];
    tail.extend_from_slice(&(b.base().0 as u64).to_be_bytes());
    tail.extend_from_slice(&65_536u32.to_be_bytes());
    assert!(bytes.ends_with(&tail));
    assert_eq!(from_bytes::<Entry>(&to_bytes(&entry)).unwrap(), entry);
    // A decoded block keeps `IdBlock::new`'s invariants: an empty one, or one past its block's last offset, fails.
    #[derive(serde::Serialize)]
    struct Raw {
        base: graftyard_types::DbId,
        len: u32,
    }
    for (base, len) in [(b.base(), 0), (b.base(), 65_537), (graftyard_types::DbId(0), 1)] {
        assert!(from_bytes::<graftyard_raft::IdBlock>(&to_bytes(&Raw { base, len })).is_err(), "{base:?} {len}");
    }
    assert_eq!(from_bytes::<graftyard_raft::IdBlock>(&to_bytes(&Raw { base: b.base(), len: 65_536 })), Ok(b));
}
