//! The clone cache's key (`design/DESIGN.md`, Shape, "Packs", `CloneKey`; journey 1, step 6). A protocol v2 fetch with
//! no haves, no client shallow state, and `done` asks for every object reachable from its wants, cut by its depth and
//! filter, so any response the shard gave for the same key answers it.

use graftyard_types::{Digest, NetworkId};
use sha2::{Digest as _, Sha256};

use crate::key::R2Key;
use crate::refs::ObjectId;
use crate::smart::V2Request;

/// The fetch arguments that change the response's bytes, sorted. `agent=` and `server-option=` lines do not.
pub type CapSet = Vec<String>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneKey {
    pub network: NetworkId,
    /// SHA-256 of the sorted, deduplicated wants.
    pub wants: Digest,
    pub depth: Option<u32>,
    pub filter: Option<String>,
    pub caps: CapSet,
    /// With `include-tag`, the answer also carries every annotated tag that points into the pack, so it depends on the
    /// tag refs, not only on the wants: the digest of the network's `refs/tags/` set at the read. Not in the design's
    /// `CloneKey`.
    pub tags: Option<Digest>,
}

/// SHA-256 over the sorted `refs/tags/` refs and their values, for `CloneKey::tags`.
pub fn tags_digest<'a>(refs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Digest {
    let mut t: Vec<(&str, &str)> = refs.into_iter().filter(|(n, _)| n.starts_with("refs/tags/")).collect();
    t.sort();
    let mut h = Sha256::new();
    for (n, o) in t {
        h.update(n.as_bytes());
        h.update(b" ");
        h.update(o.as_bytes());
        h.update(b"\n");
    }
    Digest(h.finalize().into())
}

/// The arguments a cacheable fetch may carry besides `want`, `deepen`, `filter`, and `done`. Anything else (`have`,
/// `shallow`, `deepen-since`, `deepen-not`, `deepen-relative`, `want-ref`, `packfile-uris`) makes the answer depend on
/// the client's state or on refs, so the fetch goes to the shard uncached.
const CACHEABLE_FLAGS: [&str; 5] = ["thin-pack", "ofs-delta", "no-progress", "include-tag", "wait-for-done"];

impl CloneKey {
    /// `None` when the request is not a fetch whose answer is a function of the key. `tags` is the network's
    /// `tags_digest` at the read, used only when the fetch asks for `include-tag`.
    pub fn of(network: NetworkId, req: &V2Request, tags: Digest) -> Option<CloneKey> {
        if req.command != "fetch" {
            return None;
        }
        let mut wants: Vec<ObjectId> = Vec::new();
        let (mut depth, mut filter, mut done) = (None, None, false);
        let mut caps: CapSet = Vec::new();
        for a in &req.args {
            let a = std::str::from_utf8(a).ok()?;
            if let Some(w) = a.strip_prefix("want ") {
                wants.push(ObjectId::parse_hex(w.as_bytes())?);
            } else if let Some(d) = a.strip_prefix("deepen ") {
                depth = Some(d.parse().ok().filter(|d| *d > 0)?);
            } else if let Some(f) = a.strip_prefix("filter ") {
                filter = Some(f.to_owned());
            } else if a == "done" {
                done = true;
            } else if CACHEABLE_FLAGS.contains(&a) {
                caps.push(a.to_owned());
            } else {
                return None;
            }
        }
        for c in &req.caps {
            if c.starts_with("object-format=") {
                caps.push(c.clone());
            }
        }
        if !done || wants.is_empty() {
            return None;
        }
        wants.sort();
        wants.dedup();
        caps.sort();
        caps.dedup();
        let mut h = Sha256::new();
        for w in &wants {
            h.update(w.0);
        }
        let tags = caps.iter().any(|c| c == "include-tag").then_some(tags);
        Some(CloneKey { network, wants: Digest(h.finalize().into()), depth, filter, caps, tags })
    }

    /// Whether `of` reads `tags` for this request: only a cacheable fetch that asks for `include-tag`. Every other
    /// request, an `ls-refs` above all, needs no tag read, which went to the group's leader 33 times a second on LM-1's
    /// rows (round 7) and was thrown away.
    pub fn needs_tags(network: NetworkId, req: &V2Request) -> bool {
        CloneKey::of(network, req, Digest([0; 32])).is_some_and(|k| k.tags.is_some())
    }

    /// SHA-256 of the key's fields, which `R2Key::CloneCache` names.
    pub fn digest(&self) -> Digest {
        let mut h = Sha256::new();
        h.update(b"graftyard/clone/1\0");
        h.update(self.network.0.0.to_be_bytes());
        h.update(self.wants.0);
        h.update(self.depth.map_or(0u64, |d| u64::from(d) + 1).to_be_bytes());
        h.update(self.filter.as_deref().unwrap_or("\0none").as_bytes());
        h.update(b"\0");
        for c in &self.caps {
            h.update(c.as_bytes());
            h.update(b"\0");
        }
        h.update(self.tags.map_or([0; 32], |d| d.0));
        Digest(h.finalize().into())
    }

    pub fn r2_key(&self, replica: u8) -> R2Key {
        R2Key::CloneCache { network: self.network, key: self.digest(), replica }
    }

    /// The key's `edge-compose` manifest entry (`R2Key::CloneManifest`).
    pub fn manifest_key(&self) -> R2Key {
        R2Key::CloneManifest { network: self.network, key: self.digest() }
    }
}

#[cfg(test)]
mod tests {
    use graftyard_types::DbId;

    use super::*;
    use crate::pkt;
    use crate::smart::parse_v2;

    const NET: NetworkId = NetworkId(DbId(5));

    fn fetch(args: &[&str]) -> V2Request {
        let mut b = Vec::new();
        pkt::line(&mut b, "command=fetch");
        pkt::line(&mut b, "agent=git/2.39.5");
        pkt::line(&mut b, "object-format=sha1");
        pkt::delim(&mut b);
        for a in args {
            pkt::line(&mut b, a);
        }
        pkt::flush(&mut b);
        parse_v2(&b).unwrap()
    }

    fn key(n: NetworkId, args: &[&str]) -> Option<CloneKey> {
        CloneKey::of(n, &fetch(args), Digest([0; 32]))
    }

    fn want(c: char) -> String {
        format!("want {}", c.to_string().repeat(40))
    }

    #[test]
    fn a_clone_is_cacheable_and_keyed_by_wants_depth_filter_and_caps() {
        let a = key(NET, &["thin-pack", "ofs-delta", &want('a'), &want('b'), "done"]).unwrap();
        // Order and repetition of wants and flags do not matter, and the agent does not.
        let b = key(NET, &[&want('b'), "ofs-delta", &want('a'), &want('a'), "thin-pack", "done"]);
        assert_eq!(b.as_ref().map(CloneKey::digest), Some(a.digest()));
        let deep = key(NET, &["thin-pack", "ofs-delta", &want('a'), &want('b'), "deepen 1", "done"]);
        let filt = key(NET, &["thin-pack", "ofs-delta", &want('a'), &want('b'), "filter blob:none", "done"]);
        let fewer = key(NET, &["thin-pack", &want('a'), &want('b'), "done"]);
        let other = key(NetworkId(DbId(6)), &["thin-pack", "ofs-delta", &want('a'), &want('b'), "done"]);
        let keys: Vec<Digest> = [deep, filt, fewer, other].into_iter().map(|k| k.unwrap().digest()).collect();
        for (i, k) in keys.iter().enumerate() {
            assert_ne!(*k, a.digest(), "{i}");
            assert!(keys[i + 1..].iter().all(|o| o != k), "{i}");
        }
    }

    #[test]
    fn a_fetch_that_depends_on_client_state_is_not_cacheable() {
        let have = format!("have {}", "c".repeat(40));
        let shallow = format!("shallow {}", "c".repeat(40));
        for args in [
            vec![want('a'), have.clone(), "done".into()],
            vec![want('a'), shallow],
            vec![want('a'), "deepen-since 1700000000".into(), "done".into()],
            vec![want('a'), "deepen 0".into(), "done".into()],
            vec![want('a')],
            vec!["done".into()],
            vec![want('a'), "want-ref refs/heads/main".into(), "done".into()],
            vec!["want nothex".into(), "done".into()],
        ] {
            let a: Vec<&str> = args.iter().map(String::as_str).collect();
            assert_eq!(key(NET, &a), None, "{args:?}");
        }
        let mut ls = fetch(&[]);
        ls.command = "ls-refs".into();
        assert_eq!(CloneKey::of(NET, &ls, Digest([0; 32])), None);
    }

    #[test]
    fn only_a_cacheable_include_tag_fetch_needs_the_tags() {
        let w = want('a');
        assert!(CloneKey::needs_tags(NET, &fetch(&[w.as_str(), "include-tag", "done"])));
        assert!(!CloneKey::needs_tags(NET, &fetch(&[w.as_str(), "done"])), "no include-tag");
        let have = format!("have {}", "c".repeat(40));
        assert!(!CloneKey::needs_tags(NET, &fetch(&[w.as_str(), "include-tag", have.as_str(), "done"])), "uncacheable");
        let mut ls = fetch(&["include-tag"]);
        ls.command = "ls-refs".into();
        assert!(!CloneKey::needs_tags(NET, &ls), "ls-refs");
    }

    #[test]
    fn include_tag_keys_on_the_tag_refs_and_nothing_else_does() {
        let t1 = tags_digest([("refs/tags/v1", "aa"), ("refs/heads/main", "bb")]);
        let t2 = tags_digest([("refs/heads/main", "cc"), ("refs/tags/v1", "aa")]);
        assert_eq!(t1, t2, "branches do not count, order does not matter");
        let t3 = tags_digest([("refs/tags/v1", "aa"), ("refs/tags/v2", "dd")]);
        let w = want('a');
        let args = [w.as_str(), "include-tag", "done"];
        let k = |t| CloneKey::of(NET, &fetch(&args), t).unwrap().digest();
        assert_ne!(k(t1), k(t3), "a new tag changes an include-tag answer");
        let plain = [w.as_str(), "done"];
        let k = |t| CloneKey::of(NET, &fetch(&plain), t).unwrap().digest();
        assert_eq!(k(t1), k(t3), "without include-tag the tags do not matter");
    }
}
