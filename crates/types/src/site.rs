//! Sites (`design/DESIGN.md`, Shape, "Sites, not services, are the failure unit" and "Sites and placement").

use serde::{Deserialize, Serialize};

use crate::ids::ReleaseId;

/// DO and R2 hint names that spawn locally.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Region {
    Enam,
    Wnam,
    Weur,
    Eeur,
    Apac,
    Oc,
}

impl Region {
    /// The Durable Object `locationHint` and R2 location hint for this region. Only the first `get()` of an object
    /// respects it, so callers pass it on every `get()` of a member, from the site table compiled into them.
    pub fn hint(self) -> &'static str {
        match self {
            Region::Enam => "enam",
            Region::Wnam => "wnam",
            Region::Weur => "weur",
            Region::Eeur => "eeur",
            Region::Apac => "apac",
            Region::Oc => "oc",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Jurisdiction {
    Eu,
    Us,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SiteId(pub u8);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteLink {
    /// Default: same account, DO binding with `script_name`.
    Binding { member_class: String },
    /// Account-separated option and managed front-to-cell: Ed25519-signed HTTPS through the site's Worker.
    Https { ring_host: String, key: [u8; 32] },
}

/// One Worker script pinned to one region. Its DO classes, container classes, bucket, and release are its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Site {
    pub id: SiteId,
    pub script: String,
    pub account: String,
    pub region: Region,
    pub jurisdiction: Option<Jurisdiction>,
    pub bucket: String,
    pub release: Option<ReleaseId>,
    pub link: SiteLink,
    /// Pre-created per continent: only a canary group, no warm containers.
    pub spare: bool,
}

/// A set of sites, as `PackRef::Stored` and `BlobRef` name the buckets that hold a copy. Site ids are below 64.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SiteSet(pub u64);

impl SiteSet {
    pub const EMPTY: SiteSet = SiteSet(0);

    pub fn of(sites: impl IntoIterator<Item = SiteId>) -> SiteSet {
        sites.into_iter().fold(SiteSet::EMPTY, SiteSet::with)
    }

    pub fn with(self, site: SiteId) -> SiteSet {
        assert!(site.0 < 64, "site id {} is not below 64", site.0);
        SiteSet(self.0 | 1 << site.0)
    }

    pub fn contains(self, site: SiteId) -> bool {
        site.0 < 64 && self.0 & (1 << site.0) != 0
    }

    pub fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = SiteId> {
        (0..64u8).filter(move |i| self.0 & (1 << i) != 0).map(SiteId)
    }
}

/// Every active, spare, and residency site of the install. Each script carries it in its own version (the
/// `GRAFTYARD_SITES` variable), so the hint table is compiled into callers and a change to it is a site day.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SiteTable(pub Vec<Site>);

impl SiteTable {
    pub fn get(&self, id: SiteId) -> Option<&Site> {
        self.0.iter().find(|s| s.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_set_holds_each_site_once() {
        let s = SiteSet::of([SiteId(0), SiteId(2), SiteId(2), SiteId(63)]);
        assert_eq!(s.len(), 3);
        assert!(s.contains(SiteId(2)) && !s.contains(SiteId(1)) && !s.contains(SiteId(64)));
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![SiteId(0), SiteId(2), SiteId(63)]);
        assert!(SiteSet::EMPTY.is_empty());
        assert_eq!(serde_json::to_string(&SiteSet::of([SiteId(1)])).unwrap(), "2");
    }
}
