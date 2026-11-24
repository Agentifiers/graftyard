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

    /// The continent a spare replaces a lost site on: a spare never votes beside an active site of its own continent
    /// (`design/DESIGN.md`, journey 5: the spare is "on A's continent").
    pub fn continent(self) -> u8 {
        match self {
            Region::Enam | Region::Wnam => 0,
            Region::Weur | Region::Eeur => 1,
            Region::Apac | Region::Oc => 2,
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

    /// The spare a `ReplaceMember` of `lost` moves its members onto: the first spare, by id, on `lost`'s continent.
    /// `None` for a spare or unknown `lost`, or a continent with no spare.
    pub fn spare_for(&self, lost: SiteId) -> Option<SiteId> {
        let l = self.get(lost).filter(|s| !s.spare)?;
        self.0.iter().filter(|s| s.spare && s.region.continent() == l.region.continent()).map(|s| s.id).min()
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

    fn site(id: u8, region: Region, spare: bool) -> Site {
        Site {
            id: SiteId(id),
            script: format!("s{id}"),
            account: "a".into(),
            region,
            jurisdiction: None,
            bucket: format!("b{id}"),
            release: None,
            link: SiteLink::Binding { member_class: "Member".into() },
            spare,
        }
    }

    /// The spare is on the lost site's continent, not merely another spare: an install with spares in eeur and wnam
    /// replaces enam with wnam and weur with eeur, and a site with no spare on its continent has none.
    #[test]
    fn a_lost_site_is_replaced_by_the_spare_on_its_continent() {
        let t = SiteTable(vec![
            site(0, Region::Enam, false),
            site(1, Region::Weur, false),
            site(2, Region::Apac, false),
            site(3, Region::Eeur, true),
            site(4, Region::Wnam, true),
            site(5, Region::Wnam, true),
        ]);
        assert_eq!(t.spare_for(SiteId(0)), Some(SiteId(4)), "the lowest wnam spare");
        assert_eq!(t.spare_for(SiteId(1)), Some(SiteId(3)));
        assert_eq!(t.spare_for(SiteId(2)), None, "no spare in apac or oc");
        assert_eq!(t.spare_for(SiteId(3)), None, "a spare is never lost");
        assert_eq!(t.spare_for(SiteId(9)), None);
    }
}
