//! What every surface renders the same way: timestamps, node ids, URLs, and accounts (`design/SHARED.md`,
//! "Cross-cutting conventions" and "Client ids and object ids").

use graftyard_types::{AccountId, DbId, Timestamp};
use serde_json::{Value, json};

use crate::view::{Directory, Repo};

/// ISO 8601 in UTC with a `Z`, to the second, as GitHub renders `created_at`.
pub fn iso(t: Timestamp) -> String {
    let secs = (t.0 / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// A node id in GitHub's new format: the type prefix, `_`, then base64url of the MessagePack array `[0, ids...]`,
/// with the repository id first for repository-scoped types (`research/wire-census.md` #26 and #27).
pub fn node_id(prefix: &str, ids: &[i64]) -> String {
    let mut b = vec![0x90 | (ids.len() as u8 + 1), 0];
    for id in ids {
        msgpack_int(&mut b, *id);
    }
    format!("{prefix}_{}", b64url(&b))
}

/// Parses a node id this module wrote: its prefix and ids.
pub fn parse_node_id(s: &str) -> Option<(&str, Vec<i64>)> {
    let (prefix, rest) = s.split_once('_')?;
    let b = unb64url(rest)?;
    let (&head, mut tail) = b.split_first()?;
    if head & 0xf0 != 0x90 || head & 0x0f == 0 {
        return None;
    }
    let mut ids = Vec::new();
    for i in 0..(head & 0x0f) {
        let (v, t) = msgpack_read(tail)?;
        tail = t;
        if i > 0 {
            ids.push(v);
        } else if v != 0 {
            return None;
        }
    }
    tail.is_empty().then_some((prefix, ids))
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
        0xcf => take(8),
        _ => None,
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn b64url(b: &[u8]) -> String {
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = c.iter().enumerate().fold(0u32, |a, (i, x)| a | u32::from(*x) << (16 - 8 * i));
        for i in 0..=c.len() {
            s.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    s
}

fn unb64url(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        acc = acc << 6 | B64.iter().position(|x| *x == c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// An opaque connection cursor for position `n`.
pub fn cursor(n: i64) -> String {
    b64url(format!("cursor:{n}").as_bytes())
}

pub fn parse_cursor(s: &str) -> Option<i64> {
    String::from_utf8(unb64url(s)?).ok()?.strip_prefix("cursor:")?.parse().ok()
}

/// The install's host, which every URL names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host(pub String);

impl Host {
    pub fn api(&self) -> String {
        format!("https://{}/api/v3", self.0)
    }
    pub fn web(&self) -> String {
        format!("https://{}", self.0)
    }
}

/// GitHub's `simple-user` for an account, or `None` for a host bot with no account.
pub fn simple_user(h: &Host, dir: &dyn Directory, id: Option<AccountId>) -> Value {
    let Some(id) = id else { return Value::Null };
    let login = dir.login(id).unwrap_or_else(|| "ghost".into());
    let (api, web) = (h.api(), h.web());
    json!({
        "login": login, "id": id.0.0, "node_id": node_id(if dir.is_org(id) { "O" } else { "U" }, &[id.0.0]),
        "avatar_url": format!("{web}/identicons/{login}.png"), "gravatar_id": "",
        "url": format!("{api}/users/{login}"), "html_url": format!("{web}/{login}"),
        "followers_url": format!("{api}/users/{login}/followers"),
        "following_url": format!("{api}/users/{login}/following{{/other_user}}"),
        "gists_url": format!("{api}/users/{login}/gists{{/gist_id}}"),
        "starred_url": format!("{api}/users/{login}/starred{{/owner}}{{/repo}}"),
        "subscriptions_url": format!("{api}/users/{login}/subscriptions"),
        "organizations_url": format!("{api}/users/{login}/orgs"),
        "repos_url": format!("{api}/users/{login}/repos"),
        "events_url": format!("{api}/users/{login}/events{{/privacy}}"),
        "received_events_url": format!("{api}/users/{login}/received_events"),
        "type": if dir.is_org(id) { "Organization" } else { "User" }, "site_admin": false,
    })
}

/// GitHub's `author_association` and GraphQL's `CommentAuthorAssociation` for an account on this repository.
pub fn association(r: &Repo<'_>, who: Option<AccountId>) -> &'static str {
    use graftyard_identity::RepoPermission as P;
    match who {
        Some(a) if a == r.at.owner_id => "OWNER",
        Some(a) => match r.dir.permission(a) {
            Some(P::Write | P::Maintain | P::Admin) => "COLLABORATOR",
            _ => "NONE",
        },
        None => "NONE",
    }
}

/// A label's hex color.
pub fn color(c: [u8; 3]) -> String {
    format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

pub fn db(id: DbId) -> i64 {
    id.0
}
