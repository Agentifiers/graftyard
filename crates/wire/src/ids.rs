//! Client ids and object ids (`design/SHARED.md`, "Client ids and object ids"): the first-party OAuth client ids, the
//! `databaseId` and `fullDatabaseId` rule, and node ids in GitHub's new MessagePack format, with the legacy base64
//! form accepted as input (`research/wire-census.md` #26 to #28).

use graftyard_types::RepoId;
use smallvec::SmallVec;

/// First-party GitHub clients, seeded once with their real client ids (`design/SHARED.md`, "Identity and tokens").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirstPartyClient {
    GhCli,
    Gcm,
    VsCode,
    DesktopProd,
    DesktopDev,
}

impl FirstPartyClient {
    pub const ALL: [FirstPartyClient; 5] = [Self::GhCli, Self::Gcm, Self::VsCode, Self::DesktopProd, Self::DesktopDev];

    pub const fn client_id(self) -> &'static str {
        match self {
            Self::GhCli => "178c6fc778ccc68e1d6a",
            Self::Gcm => "0120e057bd645470c1ed",
            Self::VsCode => "01ab8ac9400c4e429b23",
            Self::DesktopProd => "de0e3c7e9973e1c4dd77",
            Self::DesktopDev => "3a723b10ac5575cc5bb9",
        }
    }

    pub fn by_client_id(id: &str) -> Option<FirstPartyClient> {
        Self::ALL.into_iter().find(|c| c.client_id() == id)
    }
}

/// GraphQL's `databaseId: Int` is 32-bit, so it is set only where the id fits; `fullDatabaseId: BigInt` is the id as
/// a string everywhere (`research/wire-census.md` #28).
pub fn database_id(id: i64) -> Option<i32> {
    i32::try_from(id).ok()
}

pub fn full_database_id(id: i64) -> String {
    id.to_string()
}

/// Every node type a handler names: its new-format prefix, its GraphQL type, and whether the repository id leads its
/// ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeType {
    User,
    Organization,
    Repository,
    Issue,
    PullRequest,
    IssueComment,
    PullRequestReview,
    Label,
    StatusContext,
    CheckRun,
    Release,
    ReleaseAsset,
    /// A GitHub App's bot account (`{slug}[bot]`), GitHub's `BOT_` prefix.
    Bot,
    /// A pull request's `merged` issue event (F137), GitHub's `ME_` prefix.
    MergedEvent,
}

impl NodeType {
    const ALL: [NodeType; 14] = [
        Self::User,
        Self::Organization,
        Self::Repository,
        Self::Issue,
        Self::PullRequest,
        Self::IssueComment,
        Self::PullRequestReview,
        Self::Label,
        Self::StatusContext,
        Self::CheckRun,
        Self::Release,
        Self::ReleaseAsset,
        Self::Bot,
        Self::MergedEvent,
    ];

    pub const fn prefix(self) -> &'static str {
        match self {
            Self::User => "U",
            Self::Organization => "O",
            Self::Repository => "R",
            Self::Issue => "I",
            Self::PullRequest => "PR",
            Self::IssueComment => "IC",
            Self::PullRequestReview => "PRR",
            Self::Label => "LA",
            Self::StatusContext => "SC",
            Self::CheckRun => "CR",
            Self::Release => "RE",
            Self::ReleaseAsset => "RA",
            Self::Bot => "BOT",
            Self::MergedEvent => "ME",
        }
    }

    pub const fn type_name(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Organization => "Organization",
            Self::Repository => "Repository",
            Self::Issue => "Issue",
            Self::PullRequest => "PullRequest",
            Self::IssueComment => "IssueComment",
            Self::PullRequestReview => "PullRequestReview",
            Self::Label => "Label",
            Self::StatusContext => "StatusContext",
            Self::CheckRun => "CheckRun",
            Self::Release => "Release",
            Self::ReleaseAsset => "ReleaseAsset",
            Self::Bot => "Bot",
            Self::MergedEvent => "MergedEvent",
        }
    }

    /// Repository-scoped types put the repository id first (`research/wire-census.md` #27).
    pub const fn repo_scoped(self) -> bool {
        !matches!(self, Self::User | Self::Organization | Self::Repository | Self::Bot)
    }

    fn by_prefix(p: &str) -> Option<NodeType> {
        Self::ALL.into_iter().find(|t| t.prefix() == p)
    }

    fn by_type_name(n: &str) -> Option<NodeType> {
        Self::ALL.into_iter().find(|t| t.type_name() == n)
    }
}

/// A node id. `ids` is `[repo, id]` for a repository-scoped type and `[id]` otherwise. A legacy id carries only the
/// object's own id, so a repository-scoped legacy id has `legacy` set and names no repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeId {
    pub ty: NodeType,
    pub ids: SmallVec<[i64; 3]>,
    pub legacy: bool,
}

impl NodeId {
    /// A repository-scoped object of `repo`.
    pub fn scoped(ty: NodeType, repo: RepoId, id: i64) -> NodeId {
        debug_assert!(ty.repo_scoped());
        NodeId { ty, ids: SmallVec::from_slice(&[repo.0.0, id]), legacy: false }
    }

    /// An account or a repository: one id.
    pub fn top(ty: NodeType, id: i64) -> NodeId {
        debug_assert!(!ty.repo_scoped());
        NodeId { ty, ids: SmallVec::from_slice(&[id]), legacy: false }
    }

    /// GitHub's new format: the prefix, `_`, then base64url of the MessagePack array `[0, ids...]`. Every Graftyard
    /// object has only a new-format id, so `X-Github-Next-Global-ID: 1` changes nothing that is rendered.
    pub fn encode(&self) -> String {
        let mut b = vec![0x90 | (self.ids.len() as u8 + 1), 0];
        for id in &self.ids {
            msgpack_int(&mut b, *id);
        }
        format!("{}_{}", self.ty.prefix(), b64(&b, B64URL, false))
    }

    /// Either format. A legacy id is base64 of `0{len}:{TypeName}{id}`, as `MDQ6VXNlcjM0MDczMDM=` is `04:User3407303`.
    pub fn parse(s: &str) -> Option<NodeId> {
        match s.split_once('_') {
            Some((p, rest)) => {
                let ty = NodeType::by_prefix(p)?;
                let ids = parse_msgpack(&unb64(rest, B64URL)?)?;
                (ids.len() == if ty.repo_scoped() { 2 } else { 1 }).then_some(NodeId { ty, ids, legacy: false })
            }
            None => {
                let text = String::from_utf8(unb64(s.trim_end_matches('='), B64STD)?).ok()?;
                let (len, rest) = text.split_once(':')?;
                let len: usize = len.parse().ok().filter(|_| len.starts_with('0'))?;
                let (name, id) = (rest.get(..len)?, rest.get(len..)?);
                let id: i64 = id.parse().ok().filter(|v: &i64| *v >= 0 && id == v.to_string())?;
                Some(NodeId { ty: NodeType::by_type_name(name)?, ids: SmallVec::from_slice(&[id]), legacy: true })
            }
        }
    }

    /// The legacy spelling, for tests and for clients that send it back.
    pub fn encode_legacy(ty: NodeType, id: i64) -> String {
        legacy(ty.type_name(), id)
    }

    /// The repository the id routes to: its own id for a repository, the leading id for a new-format scoped id, and
    /// `None` for an account or a legacy scoped id, which names no repository.
    pub fn repo(&self) -> Option<RepoId> {
        use graftyard_types::DbId;
        match (self.ty, self.legacy) {
            (NodeType::Repository, _) => Some(RepoId(DbId(self.ids[0]))),
            (t, false) if t.repo_scoped() => Some(RepoId(DbId(self.ids[0]))),
            _ => None,
        }
    }

    /// The object's own id: the last of `ids`.
    pub fn id(&self) -> i64 {
        *self.ids.last().expect("a node id has an id")
    }
}

fn msgpack_int(b: &mut Vec<u8>, v: i64) {
    match v {
        0..=0x7f => b.push(v as u8),
        0x80..=0xff => b.extend([0xcc, v as u8]),
        0x100..=0xffff => {
            b.push(0xcd);
            b.extend((v as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            b.push(0xce);
            b.extend((v as u32).to_be_bytes());
        }
        _ => {
            b.push(0xcf);
            b.extend((v as u64).to_be_bytes());
        }
    }
}

/// `[0, ids...]` with at least one id; the leading 0 is the only first element this module writes.
fn parse_msgpack(b: &[u8]) -> Option<SmallVec<[i64; 3]>> {
    let (&head, mut tail) = b.split_first()?;
    if head & 0xf0 != 0x90 || head & 0x0f < 2 {
        return None;
    }
    let mut ids = SmallVec::new();
    for i in 0..(head & 0x0f) {
        let (v, t) = msgpack_read(tail)?;
        tail = t;
        match i {
            0 if v != 0 => return None,
            0 => {}
            _ => ids.push(v),
        }
    }
    tail.is_empty().then_some(ids)
}

fn msgpack_read(b: &[u8]) -> Option<(i64, &[u8])> {
    let (&t, r) = b.split_first()?;
    let take = |n: usize| -> Option<(i64, &[u8])> {
        let (x, rest) = (r.get(..n)?, &r[n..]);
        Some((x.iter().fold(0u64, |a, c| a << 8 | u64::from(*c)) as i64, rest))
    };
    match t {
        0..=0x7f => Some((i64::from(t), r)),
        0xcc => take(1),
        0xcd => take(2),
        0xce => take(4),
        0xcf => take(8).filter(|(v, _)| *v >= 0),
        _ => None,
    }
}

const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const B64STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// A legacy id of a GraphQL type no handler resolves, such as a webhook's `IntegrationInstallation`.
pub fn legacy(type_name: &str, id: i64) -> String {
    b64(format!("0{}:{type_name}{id}", type_name.len()).as_bytes(), B64STD, true)
}

pub(crate) fn b64(b: &[u8], alphabet: &[u8; 64], pad: bool) -> String {
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = c.iter().enumerate().fold(0u32, |a, (i, x)| a | u32::from(*x) << (16 - 8 * i));
        for i in 0..=c.len() {
            s.push(alphabet[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
        if pad {
            s.extend(std::iter::repeat_n('=', 3 - c.len()));
        }
    }
    s
}

pub(crate) fn unb64(s: &str, alphabet: &[u8; 64]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        acc = acc << 6 | alphabet.iter().position(|x| *x == c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

pub(crate) fn b64url(b: &[u8]) -> String {
    b64(b, B64URL, false)
}

pub(crate) fn unb64url(s: &str) -> Option<Vec<u8>> {
    unb64(s, B64URL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn githubs_own_ids_decode() {
        // `research/wire-census.md` #26 and #27.
        let u = NodeId::parse("U_kgDOADP9xw").unwrap();
        assert_eq!((u.ty, u.ids.as_slice(), u.legacy), (NodeType::User, &[3_407_303][..], false));
        assert_eq!(NodeId::top(NodeType::User, 3_407_303).encode(), "U_kgDOADP9xw");
        let cr = NodeId::parse("CR_kwDNjpbPAAAAG_DLe18").unwrap();
        assert_eq!((cr.ty, cr.ids.as_slice()), (NodeType::CheckRun, &[36_502, 120_003_984_223][..]));
        assert_eq!(
            NodeId::scoped(NodeType::CheckRun, RepoId(graftyard_types::DbId(36_502)), 120_003_984_223).encode(),
            "CR_kwDNjpbPAAAAG_DLe18"
        );
        let legacy = NodeId::parse("MDQ6VXNlcjM0MDczMDM=").unwrap();
        assert_eq!((legacy.ty, legacy.id(), legacy.legacy), (NodeType::User, 3_407_303, true));
        let repo = NodeId::parse("MDEwOlJlcG9zaXRvcnkzNjUwMg==").unwrap();
        assert_eq!(repo.repo(), Some(RepoId(graftyard_types::DbId(36_502))));
        assert_eq!(NodeId::encode_legacy(NodeType::Repository, 36_502), "MDEwOlJlcG9zaXRvcnkzNjUwMg==");
    }

    #[test]
    fn a_legacy_scoped_id_names_no_repository() {
        let pr = NodeId::parse(&NodeId::encode_legacy(NodeType::PullRequest, 9)).unwrap();
        assert_eq!((pr.ty, pr.id(), pr.repo()), (NodeType::PullRequest, 9, None));
    }

    #[test]
    fn ids_round_trip_and_malformed_ones_are_refused() {
        let id = NodeId::scoped(NodeType::PullRequest, RepoId(graftyard_types::DbId(7)), 1 << 40);
        assert_eq!(NodeId::parse(&id.encode()), Some(id));
        for bad in ["", "PR_", "ZZ_kgDOADP9xw", "PR_kgDOADP9xw", "U_kwDNjpbPAAAAG_DLe18", "MDQ6VXNlcjAx", "!!"] {
            assert_eq!(NodeId::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn database_id_is_set_only_where_it_fits_an_int() {
        assert_eq!(database_id(7), Some(7));
        assert_eq!(database_id(i64::from(i32::MAX) + 1), None);
        assert_eq!(full_database_id(1 << 45), "35184372088832");
    }

    #[test]
    fn first_party_clients_are_the_five_seeded_ids() {
        assert_eq!(FirstPartyClient::by_client_id("178c6fc778ccc68e1d6a"), Some(FirstPartyClient::GhCli));
        assert_eq!(FirstPartyClient::by_client_id("x"), None);
    }
}
