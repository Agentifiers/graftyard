//! Every serde enum whose variant index is stored or sent between members keeps an explicit, stable wire tag.
//!
//! Postcard (`graftyard_raft::codec`, log rows, request rows, fold rows, member-to-member envelopes) writes an enum
//! variant as its position in the declaration, and serde_json (member commands from the edge, replies) as its name.
//! Inserting, removing, reordering, or renaming a variant therefore changes what a stored row or another release's
//! message decodes to: `m3-repos` inserted `RepoEvent::Created` and `Deleted` (33, 34) before the lease events of
//! `9fb5032`, and `graft-m1`'s lease groups stopped restoring. `wire_tags.golden` is the tag table: one line per
//! variant, `<enum> <tag> <name> <sample bytes>`. This test builds one value of every variant of every listed enum,
//! checks that postcard writes the pinned tag as its first byte and decodes it back to the same bytes, and that the
//! names are the pinned ones. A new variant goes at the end of its enum, and its line at the end of the enum's lines.

use std::fmt;

use serde::de::{self, DeserializeSeed, IntoDeserializer, Visitor};
use serde::{Deserialize, Serialize};

// ---- a deserializer that builds a sample of any type, choosing the outer enum's variant ----

#[derive(Debug)]
struct E(String);

impl fmt::Display for E {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for E {}

impl de::Error for E {
    fn custom<T: fmt::Display>(m: T) -> E {
        E(m.to_string())
    }
}

/// Strings the validated text types parse: a login or slug, a ULID, a chain, a ref, a URL, an R2 key.
const STRINGS: &[&str] = &[
    "a",
    "00000000000000000000000001",
    "repo:1",
    "refs/heads/a",
    "https://a.test/",
    "pack/1/00000000000000000000000001.pack",
];

struct Ctx {
    /// The variant the outer enum takes; inner enums take their first variant.
    force: Option<u32>,
    /// The outer enum's serde name and variant names, in declaration order.
    outer: Option<(&'static str, &'static [&'static str])>,
    /// The choice at each choice point, in the order they are reached: a string read takes `STRINGS[c]`, a sequence
    /// has `c` elements. Past the end, 0.
    picks: Vec<usize>,
    /// The choice points reached so far, and each one's number of choices.
    seen: Vec<usize>,
}

impl Ctx {
    fn choose(&mut self, of: usize) -> usize {
        let c = self.picks.get(self.seen.len()).copied().unwrap_or(0);
        self.seen.push(of);
        c
    }
}

struct B<'a>(&'a mut Ctx);

struct Seq<'a> {
    cx: &'a mut Ctx,
    left: usize,
}

impl de::SeqAccess<'static> for Seq<'_> {
    type Error = E;
    fn next_element_seed<T: DeserializeSeed<'static>>(&mut self, seed: T) -> Result<Option<T::Value>, E> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        seed.deserialize(B(&mut *self.cx)).map(Some)
    }
}

struct Enum<'a> {
    cx: &'a mut Ctx,
    idx: u32,
}

impl<'a> de::EnumAccess<'static> for Enum<'a> {
    type Error = E;
    type Variant = B<'a>;
    fn variant_seed<V: DeserializeSeed<'static>>(self, seed: V) -> Result<(V::Value, B<'a>), E> {
        let d: de::value::U32Deserializer<E> = self.idx.into_deserializer();
        Ok((seed.deserialize(d)?, B(self.cx)))
    }
}

impl de::VariantAccess<'static> for B<'_> {
    type Error = E;
    fn unit_variant(self) -> Result<(), E> {
        Ok(())
    }
    fn newtype_variant_seed<T: DeserializeSeed<'static>>(self, seed: T) -> Result<T::Value, E> {
        seed.deserialize(self)
    }
    fn tuple_variant<V: Visitor<'static>>(self, len: usize, v: V) -> Result<V::Value, E> {
        v.visit_seq(Seq { cx: self.0, left: len })
    }
    fn struct_variant<V: Visitor<'static>>(self, fields: &'static [&'static str], v: V) -> Result<V::Value, E> {
        v.visit_seq(Seq { cx: self.0, left: fields.len() })
    }
}

macro_rules! num {
    ($($m:ident $visit:ident $v:expr;)*) => {
        $(fn $m<V: Visitor<'static>>(self, v: V) -> Result<V::Value, E> {
            v.$visit($v)
        })*
    };
}

impl de::Deserializer<'static> for B<'_> {
    type Error = E;

    num! {
        deserialize_bool visit_bool false;
        deserialize_i8 visit_i8 1;
        deserialize_i16 visit_i16 1;
        deserialize_i32 visit_i32 1;
        deserialize_i64 visit_i64 1;
        deserialize_i128 visit_i128 1;
        deserialize_u8 visit_u8 1;
        deserialize_u16 visit_u16 1;
        deserialize_u32 visit_u32 1;
        deserialize_u64 visit_u64 1;
        deserialize_u128 visit_u128 1;
        deserialize_f32 visit_f32 1.0;
        deserialize_f64 visit_f64 1.0;
        deserialize_char visit_char 'a';
    }

    fn deserialize_any<V: Visitor<'static>>(self, _: V) -> Result<V::Value, E> {
        Err(E("deserialize_any: not a postcard type".into()))
    }
    fn deserialize_str<V: Visitor<'static>>(self, v: V) -> Result<V::Value, E> {
        v.visit_borrowed_str(STRINGS[self.0.choose(STRINGS.len())])
    }
    fn deserialize_string<V: Visitor<'static>>(self, v: V) -> Result<V::Value, E> {
        self.deserialize_str(v)
    }
    fn deserialize_bytes<V: Visitor<'static>>(self, v: V) -> Result<V::Value, E> {
        v.visit_byte_buf(Vec::new())
    }
    fn deserialize_byte_buf<V: Visitor<'static>>(self, v: V) -> Result<V::Value, E> {
        v.visit_byte_buf(Vec::new())
    }
    fn deserialize_option<V: Visitor<'static>>(self, v: V) -> Result<V::Value, E> {
        v.visit_none()
    }
    fn deserialize_unit<V: Visitor<'static>>(self, v: V) -> Result<V::Value, E> {
        v.visit_unit()
    }
    fn deserialize_unit_struct<V: Visitor<'static>>(self, _: &'static str, v: V) -> Result<V::Value, E> {
        v.visit_unit()
    }
    fn deserialize_newtype_struct<V: Visitor<'static>>(self, _: &'static str, v: V) -> Result<V::Value, E> {
        v.visit_newtype_struct(self)
    }
    fn deserialize_seq<V: Visitor<'static>>(self, v: V) -> Result<V::Value, E> {
        let left = self.0.choose(2);
        v.visit_seq(Seq { cx: self.0, left })
    }
    fn deserialize_tuple<V: Visitor<'static>>(self, len: usize, v: V) -> Result<V::Value, E> {
        v.visit_seq(Seq { cx: self.0, left: len })
    }
    fn deserialize_tuple_struct<V: Visitor<'static>>(self, _: &'static str, len: usize, v: V) -> Result<V::Value, E> {
        v.visit_seq(Seq { cx: self.0, left: len })
    }
    fn deserialize_map<V: Visitor<'static>>(self, v: V) -> Result<V::Value, E> {
        v.visit_map(de::value::MapDeserializer::new(std::iter::empty::<((), ())>()))
    }
    fn deserialize_struct<V: Visitor<'static>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, E> {
        v.visit_seq(Seq { cx: self.0, left: fields.len() })
    }
    fn deserialize_enum<V: Visitor<'static>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, E> {
        let idx = match self.0.outer {
            None => {
                self.0.outer = Some((name, variants));
                self.0.force.take().unwrap_or(0)
            }
            Some(_) => 0,
        };
        v.visit_enum(Enum { cx: self.0, idx })
    }
    fn deserialize_identifier<V: Visitor<'static>>(self, _: V) -> Result<V::Value, E> {
        Err(E("identifier outside an enum".into()))
    }
    fn deserialize_ignored_any<V: Visitor<'static>>(self, v: V) -> Result<V::Value, E> {
        v.visit_unit()
    }
    fn is_human_readable(&self) -> bool {
        false
    }
}

/// A sample of `T` whose outer enum is variant `force`, with its serde name and variant names. A validated type that
/// refuses the sample moves the choices on like an odometer, last choice point first, until one is accepted.
fn sample<T: Deserialize<'static>>(force: u32) -> Result<(T, &'static str, &'static [&'static str]), String> {
    let mut picks: Vec<usize> = Vec::new();
    for _ in 0..10_000 {
        let mut cx = Ctx { force: Some(force), outer: None, picks: picks.clone(), seen: Vec::new() };
        let r = T::deserialize(B(&mut cx));
        let Some((name, variants)) = cx.outer else {
            return Err("not an enum on the wire".into());
        };
        let e = match r {
            Ok(t) => return Ok((t, name, variants)),
            Err(e) => e,
        };
        picks.resize(cx.seen.len(), 0);
        loop {
            let Some(last) = picks.len().checked_sub(1) else { return Err(e.0) };
            picks[last] += 1;
            if picks[last] < cx.seen[last] {
                break;
            }
            picks.pop();
        }
    }
    Err("no sample in 10,000 tries".into())
}

fn varint(b: &[u8]) -> u64 {
    let (mut v, mut s) = (0u64, 0);
    for x in b {
        v |= u64::from(x & 0x7f) << s;
        if x & 0x80 == 0 {
            break;
        }
        s += 7;
    }
    v
}

/// One line per variant of `T`, after checking every variant's postcard bytes start with its tag and decode back.
fn lines<T: Serialize + Deserialize<'static>>(label: &str, out: &mut Vec<String>, errs: &mut Vec<String>) {
    let variants = match sample::<T>(0) {
        Ok((_, _, v)) => v,
        Err(e) => {
            errs.push(format!("{label}: {e}"));
            return;
        }
    };
    for (i, name) in variants.iter().enumerate() {
        let t = match sample::<T>(i as u32) {
            Ok((t, ..)) => t,
            Err(e) => {
                errs.push(format!("{label}::{name}: no sample: {e}"));
                continue;
            }
        };
        let b = postcard::to_allocvec(&t).expect("encodes");
        if varint(&b) != i as u64 {
            errs.push(format!("{label}::{name}: postcard writes tag {}, not its position {i}", varint(&b)));
        }
        // Some variants borrow `&'static str` (`LeaseErr::Invalid`), so the bytes outlive the test.
        match postcard::from_bytes::<T>(Box::leak(b.clone().into_boxed_slice())) {
            Ok(back) if postcard::to_allocvec(&back).expect("encodes") == b => {}
            Ok(_) => errs.push(format!("{label}::{name}: decodes to other bytes")),
            Err(e) => errs.push(format!("{label}::{name}: does not decode: {e}")),
        }
        out.push(format!("{label} {i} {name} {}", b.iter().map(|x| format!("{x:02x}")).collect::<String>()));
    }
}

macro_rules! table {
    ($($t:ty),* $(,)?) => {{
        let (mut out, mut errs) = (Vec::new(), Vec::new());
        $(lines::<$t>(&stringify!($t).replace(' ', ""), &mut out, &mut errs);)*
        (out, errs)
    }};
}

fn current() -> (Vec<String>, Vec<String>) {
    use graftyard_actions as actions;
    use graftyard_fold as fold;
    use graftyard_git as git;
    use graftyard_identity as id;
    use graftyard_raft as raft;
    use graftyard_types as types;
    table! {
        // The six families' stored events and the commands the edge sends members.
        git::repo::RepoEvent,
        git::issue_ops::IssueOp<git::blob::BlobRef>,
        id::AuthzEvent, id::AuthzOp,
        id::IdentityEvent, id::IdentityOp,
        id::NamesEvent, id::NamesOp,
        id::TokensEvent, id::TokensOp,
        id::IdsEvent,
        // Raft's log rows and member-to-member messages.
        raft::EntryBody<git::repo::RepoEvent>, raft::Message<git::repo::RepoEvent>, raft::Deny, raft::GroupConfig,
        // Enums inside those, inside fold rows, and inside member replies.
        types::CredentialKey, types::Tier, types::MemberKind, types::PlacementName, types::SystemChain,
        types::Region, types::Jurisdiction, types::SiteLink,
        fold::SnapshotFrame, fold::Val,
        id::Visibility, id::AgentKind, id::WorkspaceMode, id::SessionId, id::Access, id::audit::Owed,
        id::AccountKind, id::OrgRole, id::RepoPermission, id::RepoEntry, id::AuthzOut, id::AuthzErr,
        id::IdentityDep, id::CredentialRefusal, id::CredentialRef, id::CredentialEvent, id::SessionEvent,
        id::SsoEvent, id::IdentityErr, id::LoginRow, id::NamesOut, id::TeamEvent, id::Invitee,
        id::InvitationGrant, id::InvitationEvent, id::InstanceAuth, id::SignInMethod, id::Family, id::TokenKind,
        id::WriteScope, id::RepoScope, id::CodeKind, id::TokensErr,
        git::agent::CiMode, git::agent::LeaseEnforce, git::agent::TrailerKey, git::agent::Source,
        git::agent::AgentRule, git::blob::ArchiveFormat, git::feed::LeaseChange, git::feed::PrAction,
        git::feed::ActivityEvent, git::group::ClassWire, git::group::PushOutcome, git::group::CallErrorWire,
        git::group::CommitReply, git::group::OutcomeReply, git::issues::IssueKind, git::issues::PrState,
        git::issues::MergeabilityResult, git::issues::Consumer, git::issues::MergeMethod, git::issues::StateReason,
        git::issues::LockReason, git::issues::IssueChange, git::issues::DiffSide, git::issues::CommentTarget,
        git::issues::CommentEvent, git::issues::ReviewState, git::issues::Grantee, git::issues::ReviewEvent,
        git::issues::LabelEvent, git::issues::Pusher, git::issues::Grouping, git::issues::CheckStatus,
        git::issues::CheckConclusion, git::issues::AnnotationLevel, git::issues::StatusState,
        git::issues::CheckSource, git::issues::LfsLockEvent, git::lease::ActorId, git::lease::LeaseMode,
        git::lease::LeaseErr, git::merge::RuleViolation, git::merge::MergeError, git::merge::MergeCmdOut,
        git::pack::PackRef, git::queue::EjectReason, git::queue::QueueEvent, git::queue_consumer::CacheAsk,
        git::refs::Atomicity, git::repo::RejectReason, git::repo::RefOutcome, git::shard::ImageSlot,
        git::task::TaskKind, git::task::TaskApiState, git::task::TaskActor, git::task::TaskErr,
        git::transfer::TransferErr,
        // M4-WP1: inside `RepoEvent::Hook`'s record.
        git::hooks::HookContentType,
        // M4-WP1: a `HookSequencer`'s stored attempt rows.
        git::sequencer::Outcome,
        git::lifecycle::LifecycleOp, git::lifecycle::LifecycleErr,
        // M4-WP2: inside `IdentityEvent::Record` and `RepoEvent::Watchers`, an `Inbox`'s stored rows and snapshots,
        // and a `SearchShard`'s name.
        id::UserRecord, git::watch::WatcherUpdate, git::inbox::Reason, git::search::Domain,
        // M4-WP3: the Actions family's stored events and commands (format 0x15.1), and the repository's
        // `RunChecksCmd` reply.
        actions::shard::ActionsEvent, actions::shard::ActionsOp, actions::shard::ActionsActor, actions::shard::ActionsOut,
        actions::shard::ActionsErr, actions::shard::RunnerScope, actions::Conclusion, actions::JobState,
        actions::JobEvent, actions::RunStatus, git::run_checks::RunChecksOut,
        // M4-WP3: inside `RepoEvent::ActionsSecret` and `ActionsVariable`.
        id::sealed::SecretScope, id::sealed::SecretVisibility,
        // R24: inside `RepoEvent::WorkflowJob`.
        git::workflow_job::JobAction,
    }
}

/// The pinned table, by enum: `(enum, [(tag, name, hex)])` in file order.
fn by_enum(lines: &[impl AsRef<str>]) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for l in lines.iter().map(AsRef::as_ref).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (e, rest) = l.split_once(' ').expect("an enum, a tag, a name, and bytes");
        match out.last_mut() {
            Some((last, v)) if last == e => v.push(rest.to_owned()),
            _ => out.push((e.to_owned(), vec![rest.to_owned()])),
        }
    }
    out
}

#[test]
fn every_stored_or_sent_enum_keeps_its_variant_tags_and_bytes() {
    let (got, errs) = current();
    assert!(errs.is_empty(), "{}", errs.join("\n"));
    if std::env::var("GRAFTYARD_PIN").is_ok() {
        for l in &got {
            println!("{l}");
        }
        return;
    }
    let want: Vec<&str> = include_str!("wire_tags.golden").lines().collect();
    let (got, want) = (by_enum(&got), by_enum(&want));
    let mut bad = Vec::new();
    for (e, w) in &want {
        let Some((_, g)) = got.iter().find(|(n, _)| n == e) else {
            bad.push(format!("{e}: no longer in the table, but rows and messages may hold it"));
            continue;
        };
        for (i, wl) in w.iter().enumerate() {
            match g.get(i) {
                Some(gl) if gl == wl => {}
                Some(gl) => bad.push(format!(
                    "{e}: tag {i} was `{wl}` and is now `{gl}`: a variant was inserted, removed, reordered, or \
                     renamed, or a field changed. Stored rows and other releases decode by these tags; put a new \
                     variant at the end of its enum"
                )),
                None => bad.push(format!("{e}: tag {i} `{wl}` was removed")),
            }
        }
        for gl in g.iter().skip(w.len()) {
            bad.push(format!("{e}: new variant `{gl}`: add its line at the end of {e}'s lines in wire_tags.golden"));
        }
    }
    for (e, _) in got.iter().filter(|(e, _)| !want.iter().any(|(w, _)| w == e)) {
        bad.push(format!("{e}: new enum in the table: add its lines to wire_tags.golden"));
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
