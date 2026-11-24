//! `graftyard sites add` and `sites add-spare` (`design/DESIGN.md`, "Install operations", Genesis step 2; Shape,
//! "Members reach each other through Durable Object bindings" and "Sites and placement").
//!
//! Order, for the sites being added:
//!
//! 1. Each new site's bucket, with the site's location hint, and its `location` read back. A misplaced bucket is
//!    deleted while still empty and recreated under a new name ("Install operations", Placement failures).
//! 2. Phase one: a site script that does not exist yet is created binding only its own classes, because a class must
//!    exist before another script binds it (`workers/site/deploy.mjs`).
//! 3. Phase two for the new sites: each binds every site's member, registry, and bucket, with `script_name` for the
//!    others, and carries the trust anchor (`GRAFTYARD_INSTALL`, `GRAFTYARD_CUSTOMER_KEY`).
//! 4. The latency check from each new site: a member object on the new site joins a probe chain and takes the median
//!    of 20 binding round trips to each other voter (the site script's own `/join`). A pair under `MIN_PAIR_RTT` is
//!    measured once more with a new member generation, and then refuses the site, whose script and bucket are
//!    deleted. Nothing else has changed yet.
//! 5. Phase two for the existing sites, then the pairs no new site is in.
//!
//! Each version carries what `workers/site/deploy.mjs` deploys: the `GitShard` container class with `--image` (bound
//! as `SHARD_<own id>` only, migration `v2`), the CPU limit, and the sweeper's Cron Trigger; after the upload the
//! class's container application is created if it is missing.
//!
//! A script whose bindings, variables, bundle, and image already match is not uploaded again, so a rerun creates no
//! version.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use graftyard_types::{ChainId, DbId, MIN_PAIR_RTT, Region, RepoId, Site, SiteId, SiteLink};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::cf::{Cf, Module};
use crate::config::{InstallToml, Res, RttRow, hex, now, random};

const COMPAT_DATE: &str = "2026-10-01";
/// `v1` created the member and registry classes, `v2` the git cache shard class (as `workers/site/deploy.mjs`).
const MIGRATION_TAG: &str = "v2";
/// The site script's sweeper Cron Trigger, the same as `workers/site/deploy.mjs`'s.
const SWEEP_CRON: &str = "*/15 * * * *";
/// The CPU limit of a site script invocation, as `workers/site/deploy.mjs` sets it.
const CPU_MS: u64 = 300_000;
/// The site bundle as `worker-build --release` writes it in `workers/site/build`.
const BUNDLE_FILES: [(&str, &str); 2] =
    [("index.js", "application/javascript+module"), ("index_bg.wasm", "application/wasm")];

/// What the operator asked for, one site.
pub struct SiteSpec {
    pub script: String,
    pub region: Region,
    pub jurisdiction: Option<graftyard_types::Jurisdiction>,
    pub spare: bool,
}

pub struct Bundle {
    files: Vec<(&'static str, &'static str, Vec<u8>)>,
    /// The `GitShard` container's digest-pinned image in the account's registry.
    image: String,
    /// SHA-256 of the files and the image, carried as `GRAFTYARD_BUNDLE` so a rerun can tell the deployed code and
    /// container are the same.
    hash: String,
}

impl Bundle {
    pub fn read(dir: &Path, image: &str) -> Res<Bundle> {
        if !image.contains("@sha256:") {
            return Err(format!("--image {image:?}: name the git image by digest (<registry>/<name>@sha256:<hex>)"));
        }
        let mut h = Sha256::new();
        let mut files = Vec::new();
        for (name, ct) in BUNDLE_FILES {
            let p = dir.join(name);
            let b =
                fs::read(&p).map_err(|e| format!("{}: {e} (build it with `worker-build --release`)", p.display()))?;
            h.update((name.len() as u32).to_be_bytes());
            h.update(name);
            h.update((b.len() as u64).to_be_bytes());
            h.update(&b);
            files.push((name, ct, b));
        }
        h.update((image.len() as u32).to_be_bytes());
        h.update(image);
        Ok(Bundle { files, image: image.into(), hash: hex(&h.finalize()) })
    }
}

/// Whether two sites can be voters of one group, so their round trip must clear `MIN_PAIR_RTT`. Two sites in one
/// region never can (`Placement::new` refuses them; a residency install with spares has more sites than regions).
/// Active sites of different regions can (`Na3` and `Eu2w` pair sites of one continent). A spare stands in for a lost
/// site of its own continent, so it votes beside the active sites of the other continents, and never beside another
/// spare.
pub fn may_share_a_group(a: &Site, b: &Site) -> bool {
    if a.region == b.region {
        return false;
    }
    match (a.spare, b.spare) {
        (false, false) => true,
        (true, true) => false,
        _ => a.region.continent() != b.region.continent(),
    }
}

fn region_name(r: Region) -> &'static str {
    r.hint()
}

fn jurisdiction_name(j: graftyard_types::Jurisdiction) -> &'static str {
    match j {
        graftyard_types::Jurisdiction::Eu => "eu",
        graftyard_types::Jurisdiction::Us => "us",
    }
}

/// The bindings and variables of one site script, in a form the settings API's answer reduces to as well.
fn desired_bindings(t: &InstallToml, site: &Site, targets: &[&Site], bundle: &Bundle) -> Vec<Value> {
    let mut b = vec![
        json!({ "type": "plain_text", "name": "SITE_ID", "text": site.id.0.to_string() }),
        json!({ "type": "plain_text", "name": "GRAFTYARD_SITES", "text": serde_json::to_string(&t.sites).expect("sites") }),
        json!({ "type": "plain_text", "name": "GRAFTYARD_INSTALL", "text": t.install }),
        json!({ "type": "plain_text", "name": "GRAFTYARD_CUSTOMER_KEY", "text": t.customer_key }),
        json!({ "type": "plain_text", "name": "GRAFTYARD_BUNDLE", "text": bundle.hash }),
    ];
    for s in targets {
        for (prefix, class) in [("MEMBER", "Member"), ("REGISTRY", "Registry")] {
            let mut d = json!({ "type": "durable_object_namespace", "name": format!("{prefix}_{}", s.id.0), "class_name": class });
            if s.id != site.id {
                d["script_name"] = json!(s.script);
            }
            b.push(d);
        }
        // A site binds only its own `GitShard`: Cloudflare attaches the container config to the first binding of the
        // class, which must be the script's own (`workers/site/deploy.mjs`).
        if s.id == site.id {
            b.push(json!({ "type": "durable_object_namespace", "name": format!("SHARD_{}", s.id.0), "class_name": "GitShard" }));
        }
        let mut r = json!({ "type": "r2_bucket", "name": format!("BUCKET_{}", s.id.0), "bucket_name": s.bucket });
        if let Some(j) = s.jurisdiction {
            r["jurisdiction"] = json!(jurisdiction_name(j));
        }
        b.push(r);
    }
    b
}

/// The comparable part of a binding: what the version holds, without ids the API adds.
fn binding_key(b: &Value) -> String {
    let f = |k: &str| b[k].as_str().unwrap_or("").to_string();
    match f("type").as_str() {
        "plain_text" => format!("text {} {}", f("name"), f("text")),
        "durable_object_namespace" => format!("do {} {} {}", f("name"), f("class_name"), f("script_name")),
        "r2_bucket" => format!("r2 {} {} {}", f("name"), f("bucket_name"), f("jurisdiction")),
        other => format!("{other} {}", f("name")),
    }
}

/// The migration from the script's applied tag to `MIGRATION_TAG`, or `None` when it is there.
fn migration(script: &str, tag: Option<&str>) -> Res<Option<Value>> {
    match tag {
        Some(MIGRATION_TAG) => Ok(None),
        None => Ok(Some(json!({
            "new_tag": MIGRATION_TAG,
            "steps": [{ "new_sqlite_classes": ["Member", "Registry"] }, { "new_sqlite_classes": ["GitShard"] }],
        }))),
        Some("v1") => Ok(Some(json!({
            "old_tag": "v1",
            "new_tag": MIGRATION_TAG,
            "steps": [{ "new_sqlite_classes": ["GitShard"] }],
        }))),
        Some(other) => Err(format!("{script} has migration tag {other:?}, not v1 or {MIGRATION_TAG:?}")),
    }
}

/// The upload metadata of one site script version: the bindings, the migration, the `GitShard` container with its
/// image, and the CPU limit, as `workers/site/deploy.mjs` deploys them.
fn metadata(site: &Site, bindings: Vec<Value>, migration: Option<Value>, image: &str) -> Value {
    let mut meta = json!({
        "main_module": "index.js",
        "compatibility_date": COMPAT_DATE,
        "bindings": bindings,
        "observability": { "enabled": true },
        "containers": [{ "name": container_name(site), "class_name": "GitShard", "images": { "git": image } }],
        "limits": { "cpu_ms": CPU_MS },
    });
    if let Some(m) = migration {
        meta["migrations"] = m;
    }
    meta
}

/// The container application's name, as wrangler names it: `<script>-<class, lower case>`.
fn container_name(site: &Site) -> String {
    format!("{}-gitshard", site.script)
}

fn same_bindings(want: &[Value], have: &[Value]) -> bool {
    let w: BTreeSet<String> = want.iter().map(binding_key).collect();
    let h: BTreeSet<String> = have.iter().map(binding_key).collect();
    w == h
}

pub struct Deployer<'a> {
    pub cf: &'a Cf,
    pub bundle: &'a Bundle,
    /// Scripts in the account, with their migration tags.
    listed: Vec<(String, Option<String>)>,
}

impl<'a> Deployer<'a> {
    pub fn new(cf: &'a Cf, bundle: &'a Bundle) -> Res<Deployer<'a>> {
        Ok(Deployer { cf, bundle, listed: cf.scripts()? })
    }

    fn exists(&self, script: &str) -> bool {
        self.listed.iter().any(|(s, _)| s == script)
    }

    fn note(&self, line: String) {
        println!("  {line}");
    }

    /// Uploads one version of the site script binding `targets`, unless the deployed version already matches.
    fn deploy(&mut self, t: &InstallToml, site: &Site, targets: &[&Site], phase: u8) -> Res<()> {
        let want = desired_bindings(t, site, targets, self.bundle);
        let tag = self.listed.iter().find(|(s, _)| *s == site.script).and_then(|(_, tag)| tag.clone());
        let migration = migration(&site.script, tag.as_deref())?;
        if self.exists(&site.script) && migration.is_none() && same_bindings(&want, &self.cf.bindings(&site.script)?) {
            self.note(format!("phase {phase}: {} unchanged, no new version", site.script));
            return Ok(());
        }
        let meta = metadata(site, want, migration, &self.bundle.image);
        let modules: Vec<Module> =
            self.bundle.files.iter().map(|(n, ct, b)| Module { name: n, content_type: ct, bytes: b }).collect();
        self.cf.put_script(&site.script, &meta, &modules)?;
        self.cf.put_schedules(&site.script, &[SWEEP_CRON])?;
        self.ensure_container_app(site)?;
        if !self.exists(&site.script) {
            self.listed.push((site.script.clone(), Some(MIGRATION_TAG.into())));
        }
        self.listed
            .iter_mut()
            .filter(|(s, _)| *s == site.script)
            .for_each(|(_, tag)| *tag = Some(MIGRATION_TAG.into()));
        // A site script has no public route.
        if self.cf.workers_dev(&site.script)? {
            self.cf.set_workers_dev(&site.script, false)?;
        }
        let names: Vec<&str> = targets.iter().map(|s| s.script.as_str()).collect();
        self.note(format!("phase {phase}: deployed {} binding [{}]", site.script, names.join(", ")));
        Ok(())
    }

    /// The `GitShard` class's Durable Object-managed container application, created once the upload made its
    /// namespace, as wrangler does (`POST /containers/applications`, keyed by the namespace id).
    fn ensure_container_app(&self, site: &Site) -> Res<()> {
        let shard = format!("SHARD_{}", site.id.0);
        let bindings = self.cf.bindings(&site.script)?;
        let ns = bindings
            .iter()
            .find(|b| b["name"] == json!(shard))
            .and_then(|b| b["namespace_id"].as_str())
            .ok_or_else(|| format!("{}: the uploaded version has no {shard} namespace", site.script))?;
        match self.cf.container_app(ns)? {
            Some(app) if app["name"] == json!(container_name(site)) => Ok(()),
            Some(app) => Err(format!(
                "{}: the container application of namespace {ns} is named {}, not {}",
                site.script,
                app["name"],
                container_name(site)
            )),
            None => {
                self.cf.create_container_app(&json!({
                    "name": container_name(site),
                    "scheduling_policy": "durable_object",
                    "durable_objects": { "namespace_id": ns },
                }))?;
                self.note(format!("{}: created container application {}", site.script, container_name(site)));
                Ok(())
            }
        }
    }
}

/// Creates the bucket with the site's hint, or finds it, and checks its `location`. A misplaced bucket is empty here
/// (no member has run), so it is deleted and recreated under the next name, at most three times. Inside a
/// jurisdiction R2 ignores the hint (an `eu` bucket hinted `weur` or `oc` read back EEUR, a `us` one hinted `wnam`
/// read back ENAM, 2026-11-01), so a jurisdiction bucket that lands elsewhere refuses the site at once: a new name
/// cannot move it.
fn ensure_bucket(cf: &Cf, site: &mut Site) -> Res<()> {
    let j = site.jurisdiction.map(jurisdiction_name);
    let want = region_name(site.region).to_ascii_uppercase();
    let base = format!("{}-packs", site.script);
    let note = |line: String| println!("  {line}");
    for n in 0..3 {
        let name = if n == 0 { base.clone() } else { format!("{base}-{n}") };
        let location = match cf.bucket_location(&name, j)? {
            Some(l) => l,
            None => cf.create_bucket(&name, region_name(site.region), j)?,
        };
        if location == want {
            site.bucket = name.clone();
            note(format!("bucket {name}: location {location}"));
            return Ok(());
        }
        note(format!("bucket {name}: location {location}, wanted {want}; deleting it while empty"));
        retry(|| cf.delete_bucket(&name, j))?;
        if let Some(j) = j {
            return Err(format!(
                "{}: R2 placed the {j} bucket in {location}, not {want}; inside a jurisdiction the location hint is not \
                 honoured, so this site cannot have its own-region bucket",
                site.script
            ));
        }
    }
    Err(format!("{}: three buckets landed outside {want}; refusing the site", site.script))
}

/// Cloudflare's API answers some deletes with a transient 500 (seen on an R2 bucket delete, 2026-11-01).
fn retry(f: impl Fn() -> Res<()>) -> Res<()> {
    let mut last = String::new();
    for i in 0..4 {
        match f() {
            Ok(()) => return Ok(()),
            Err(e) if e.contains("HTTP 5") => last = e,
            Err(e) => return Err(e),
        }
        std::thread::sleep(std::time::Duration::from_secs(1 << i));
    }
    Err(last)
}

// ---- The latency check ----

/// A throwaway Worker bound to every site's member class. It forwards one `/join` to a member object created with
/// the site's hint, which measures 20 binding round trips to each peer itself (`workers/site/src/member.rs`).
const PROBE_JS: &str = r#"export default {
  async fetch(req, env) {
    if (req.headers.get("authorization") !== `Bearer ${env.TOKEN}`) return new Response("unauthorized", { status: 401 });
    const { site, hint, join } = await req.json();
    const ns = env[`MEMBER_${site}`];
    const stub = ns.get(ns.idFromName(join.me.do_name), { locationHint: hint });
    const r = await stub.fetch("https://member/join", { method: "POST", body: JSON.stringify(join) });
    return new Response(await r.text(), { status: r.status });
  },
};
"#;

/// The chain the latency check joins. Id 0 is never assigned ("0 < id < 2^53"), so no real group shares its members.
pub const PROBE_CHAIN: ChainId = ChainId::Repo(RepoId(DbId(0)));

struct Probe<'a> {
    cf: &'a Cf,
    sites: Vec<Site>,
    script: String,
    url: String,
    token: String,
}

impl<'a> Probe<'a> {
    fn start(cf: &'a Cf, script: String, sites: &[Site]) -> Res<Probe<'a>> {
        let token = hex(&random::<24>());
        let mut bindings = vec![json!({ "type": "secret_text", "name": "TOKEN", "text": token })];
        for s in sites {
            bindings.push(json!({
                "type": "durable_object_namespace", "name": format!("MEMBER_{}", s.id.0),
                "class_name": "Member", "script_name": s.script,
            }));
        }
        let meta = json!({ "main_module": "probe.js", "compatibility_date": COMPAT_DATE, "bindings": bindings });
        cf.put_script(
            &script,
            &meta,
            &[Module { name: "probe.js", content_type: "application/javascript+module", bytes: PROBE_JS.as_bytes() }],
        )?;
        cf.set_workers_dev(&script, true)?;
        let url = format!("https://{script}.{}.workers.dev", cf.subdomain()?);
        let p = Probe { cf, sites: sites.to_vec(), script, url, token };
        // A new workers.dev route takes a while to answer; wait for the probe's own 401.
        for _ in 0..60 {
            match ureq::get(&p.url).call() {
                Err(ureq::Error::Status(401, _)) => return Ok(p),
                _ => std::thread::sleep(std::time::Duration::from_secs(2)),
            }
        }
        Err(format!("{} did not answer on {} within 120 s", p.script, p.url))
    }

    /// Joins member `{PROBE_CHAIN}#{generation}#f0` on `from` with a `Tri` record of `[from, a, b]`, and returns the
    /// member's `Rtt` rows.
    fn join(&self, from: SiteId, a: SiteId, b: SiteId, generation: u32) -> Res<Vec<RttRow>> {
        let record = graftyard_types::PlacementRecord {
            name: graftyard_types::PlacementName::Tri,
            sites: [from, a, b].into_iter().collect(),
            home: from,
            generation,
            moving_from: None,
            at: now(),
        };
        let me = record.bootstrap_members(PROBE_CHAIN).remove(0);
        let hint = self.sites.iter().find(|s| s.id == from).expect("known site").region.hint();
        let body =
            json!({ "site": from.0, "hint": hint, "join": { "chain": PROBE_CHAIN, "me": me, "record": record } });
        let r = ureq::post(&format!("{}/join", self.url))
            .set("authorization", &format!("Bearer {}", self.token))
            .timeout(std::time::Duration::from_secs(180))
            .send_json(body);
        let text = match r {
            Ok(r) => r.into_string().map_err(|e| e.to_string())?,
            Err(ureq::Error::Status(c, r)) => {
                return Err(format!("join on site {}: HTTP {c}: {}", from.0, r.into_string().unwrap_or_default()));
            }
            Err(e) => return Err(format!("join on site {}: {e}", from.0)),
        };
        let rows: Vec<(SiteId, graftyard_types::Rtt)> =
            serde_json::from_str(&text).map_err(|e| format!("join on site {}: {e}: {text}", from.0))?;
        Ok(rows.into_iter().map(|(f, r)| RttRow { from: f, peer: r.peer, median_ms: r.median_ms, at: r.at }).collect())
    }
}

impl Drop for Probe<'_> {
    fn drop(&mut self) {
        if let Err(e) = self.cf.delete_script(&self.script) {
            eprintln!("warning: could not delete the probe script {}: {e}", self.script);
        }
    }
}

fn reading(t: &InstallToml, a: SiteId, b: SiteId) -> Option<u16> {
    t.rtt.iter().filter(|r| (r.from == a && r.peer == b) || (r.from == b && r.peer == a)).map(|r| r.median_ms).min()
}

/// The pairs that may share a group and have no reading yet, each with the side that measures it.
fn missing_pairs(t: &InstallToml, measurer_ok: impl Fn(&Site, &Site) -> bool) -> Vec<(SiteId, SiteId)> {
    let mut out = Vec::new();
    for (i, a) in t.sites.iter().enumerate() {
        for b in &t.sites[i + 1..] {
            if !may_share_a_group(a, b) || reading(t, a.id, b.id).is_some() {
                continue;
            }
            if measurer_ok(a, b) {
                out.push((a.id, b.id));
            } else if measurer_ok(b, a) {
                out.push((b.id, a.id));
            }
        }
    }
    out
}

/// Measures `pairs` (measuring side first), two peers per join, and records the rows. Returns the pairs that read
/// under `MIN_PAIR_RTT` twice, the second time from members of a new generation.
/// `join(from, a, b, generation)` joins a member of `generation` on `from` with peers `a` and `b` and returns its rows.
type Join<'a> = dyn Fn(SiteId, SiteId, SiteId, u32) -> Res<Vec<RttRow>> + 'a;

fn measure(
    t: &mut InstallToml,
    join: &Join,
    pairs: &[(SiteId, SiteId)],
    generation: &mut u32,
) -> Res<Vec<(SiteId, SiteId, u16)>> {
    let sites = t.sites.clone();
    let mut by_from: Vec<(SiteId, Vec<SiteId>)> = Vec::new();
    for &(f, p) in pairs {
        match by_from.iter_mut().find(|(x, _)| *x == f) {
            Some((_, v)) => v.push(p),
            None => by_from.push((f, vec![p])),
        }
    }
    let run = |t: &mut InstallToml, from: SiteId, peers: &[SiteId], generation: &mut u32| -> Res<()> {
        for chunk in peers.chunks(2) {
            let a = chunk[0];
            // A record needs three distinct sites: pad a lone peer with any other site, which is measured too.
            let b = chunk.get(1).copied().or_else(|| sites.iter().map(|s| s.id).find(|s| *s != from && *s != a));
            let Some(b) = b else { return Err("a latency check needs three sites".into()) };
            *generation += 1;
            for row in join(from, a, b, *generation)? {
                println!("  rtt {} -> {}: {} ms", row.from.0, row.peer.0, row.median_ms);
                t.put_rtt(row);
            }
        }
        Ok(())
    };
    for (from, peers) in &by_from {
        run(t, *from, peers, generation)?;
    }
    let min = MIN_PAIR_RTT.as_millis() as u16;
    let mut close = Vec::new();
    for &(f, p) in pairs {
        if reading(t, f, p).is_none() {
            return Err(format!("the member on site {} returned no reading for site {}", f.0, p.0));
        }
        if reading(t, f, p).is_some_and(|ms| ms < min) {
            println!("  rtt {} -> {} under {min} ms; measuring again with a new member generation", f.0, p.0);
            t.rtt.retain(|r| !((r.from == f && r.peer == p) || (r.from == p && r.peer == f)));
            run(t, f, &[p], generation)?;
            if let Some(ms) = reading(t, f, p).filter(|ms| *ms < min) {
                close.push((f, p, ms));
            }
        }
    }
    Ok(close)
}

/// Checks the specs against the install before anything is created.
fn plan(t: &InstallToml, specs: &[SiteSpec]) -> Res<Vec<Site>> {
    if t.genesis.is_some() {
        return Err(
            "the install has a genesis: a new site binding changes every site script, which only a site day may \
                    ship (AGENTS.md invariants), and site days are Milestone 5"
                .into(),
        );
    }
    let mut new = Vec::new();
    let mut next = t.sites.iter().map(|s| s.id.0 + 1).max().unwrap_or(0);
    for spec in specs {
        if !spec.script.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            || spec.script.len() > 54
        {
            return Err(format!(
                "{:?}: a script name is lower-case letters, digits, and '-', at most 54 long",
                spec.script
            ));
        }
        if let Some(s) = t.site_by_script(&spec.script) {
            if s.region != spec.region || s.jurisdiction != spec.jurisdiction || s.spare != spec.spare {
                return Err(format!(
                    "{} is already site {} in {}; a site's region does not change",
                    s.script,
                    s.id.0,
                    s.region.hint()
                ));
            }
            println!("  {} is already site {}: converging its script and bindings", s.script, s.id.0);
            continue;
        }
        if next >= 64 {
            return Err("an install has at most 64 sites".into());
        }
        new.push(Site {
            id: SiteId(next),
            script: spec.script.clone(),
            account: t.account.clone(),
            region: spec.region,
            jurisdiction: spec.jurisdiction,
            bucket: String::new(),
            release: None,
            link: SiteLink::Binding { member_class: "Member".into() },
            spare: spec.spare,
        });
        next += 1;
    }
    Ok(new)
}

pub fn add(config: &Path, bundle_dir: &Path, image: &str, specs: Vec<SiteSpec>) -> Res<()> {
    let mut t = InstallToml::load(config)?;
    let new = plan(&t, &specs)?;
    let cf = Cf::from_env(&t.account)?;
    let bundle = Bundle::read(bundle_dir, image)?;

    // Undoes what this run created for the new sites: their scripts, if any, and their buckets.
    let rollback = |new: &[Site]| {
        for s in new.iter().filter(|s| !s.bucket.is_empty()) {
            if let Err(e) =
                retry(|| cf.delete_script(&s.script)).or_else(|e| if e.contains("HTTP 404") { Ok(()) } else { Err(e) })
            {
                eprintln!("warning: could not delete {}: {e}", s.script);
            }
            match retry(|| cf.delete_bucket(&s.bucket, s.jurisdiction.map(jurisdiction_name))) {
                Ok(()) => println!("  rolled back {} and bucket {}", s.script, s.bucket),
                Err(e) => eprintln!("warning: could not delete bucket {}: {e}", s.bucket),
            }
        }
    };

    // 1. Buckets.
    let mut new = new;
    for i in 0..new.len() {
        if let Err(e) = ensure_bucket(&cf, &mut new[i]) {
            rollback(&new[..i]);
            return Err(format!("refused: {e}"));
        }
    }
    let new_ids: Vec<SiteId> = new.iter().map(|s| s.id).collect();
    let existing: Vec<Site> = t.sites.clone();
    t.sites.extend(new.iter().cloned());

    let mut d = match Deployer::new(&cf, &bundle) {
        Ok(d) => d,
        Err(e) => {
            rollback(&new);
            return Err(e);
        }
    };
    // Set once an existing site may bind a new one: from then a rollback would leave a binding to a deleted script.
    let committed = std::cell::Cell::new(false);
    let result = (|| -> Res<Vec<(SiteId, SiteId, u16)>> {
        // 2. Phase one: scripts that do not exist yet bind only their own classes.
        let missing: Vec<Site> = t.sites.iter().filter(|s| !d.exists(&s.script)).cloned().collect();
        for s in &missing {
            d.deploy(&t, s, &[s], 1)?;
        }
        // 3. Phase two for the new sites.
        let all: Vec<Site> = t.sites.clone();
        let refs: Vec<&Site> = all.iter().collect();
        for s in all.iter().filter(|s| new_ids.contains(&s.id)) {
            d.deploy(&t, s, &refs, 2)?;
        }
        // 4. The latency check from each new site. A record names three distinct sites, so with fewer the check waits
        // for the third site's `sites add`, which measures every pair still missing.
        let probe_script = format!("{}-probe", specs.first().map(|s| s.script.as_str()).unwrap_or("graft"));
        let mut generation = (now().0 / 1000) as u32;
        let first = missing_pairs(&t, |a, _| new_ids.contains(&a.id));
        let rest_needed = !missing_pairs(&t, |_, _| true).is_empty();
        let probe = if t.sites.len() < 3 {
            println!("  latency check: fewer than three sites, so no pair is measured yet");
            None
        } else if first.is_empty() && !rest_needed {
            println!("  latency check: every pair already measured");
            None
        } else {
            Some(Probe::start(&cf, probe_script, &t.sites)?)
        };
        if let Some(p) = &probe {
            let close = measure(&mut t, &|f, a, b, g| p.join(f, a, b, g), &first, &mut generation)?;
            if !close.is_empty() {
                return Err(close
                    .iter()
                    .map(|(a, b, ms)| {
                        format!("sites {} and {} are {ms} ms apart, under MIN_PAIR_RTT: misplaced", a.0, b.0)
                    })
                    .collect::<Vec<_>>()
                    .join("; "));
            }
        }
        // 5. Phase two for the existing sites, then pairs between them.
        committed.set(true);
        for s in all.iter().filter(|s| !new_ids.contains(&s.id)) {
            d.deploy(&t, s, &refs, 2)?;
        }
        match &probe {
            Some(p) => {
                let pairs = missing_pairs(&t, |_, _| true);
                measure(&mut t, &|f, a, b, g| p.join(f, a, b, g), &pairs, &mut generation)
            }
            None => Ok(Vec::new()),
        }
    })();
    match result {
        Err(e) if committed.get() => {
            t.save(config)?;
            Err(format!(
                "{e}\nthe new sites are recorded and some existing sites may bind them: rerun the same command, which converges"
            ))
        }
        Err(e) => {
            rollback(&new);
            t.sites = existing;
            Err(format!("refused: {e}"))
        }
        Ok(close) => {
            t.save(config)?;
            for s in &new {
                println!(
                    "recorded site {} {} ({}{}) bucket {}",
                    s.id.0,
                    s.script,
                    s.region.hint(),
                    if s.spare { ", spare" } else { "" },
                    s.bucket
                );
            }
            if !close.is_empty() {
                let pairs: Vec<String> = close.iter().map(|(a, b, ms)| format!("{}-{} at {ms} ms", a.0, b.0)).collect();
                return Err(format!("existing sites read under MIN_PAIR_RTT: {}", pairs.join(", ")));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn spares_pair_only_with_other_continents() {
        let a = site(0, Region::Enam, false);
        let b = site(1, Region::Weur, false);
        let e = site(3, Region::Eeur, true);
        let w = site(4, Region::Wnam, true);
        assert!(may_share_a_group(&a, &b));
        assert!(!may_share_a_group(&b, &e), "eeur spare replaces weur, never votes beside it");
        assert!(may_share_a_group(&a, &e));
        assert!(!may_share_a_group(&e, &w));
        assert!(may_share_a_group(&site(5, Region::Wnam, false), &a), "Na3 pairs enam and wnam");
        assert!(!may_share_a_group(&site(6, Region::Enam, false), &a), "one region never shares a group");
    }

    fn install(sites: Vec<Site>) -> InstallToml {
        InstallToml {
            install: "01K8ZZZZZZZZZZZZZZZZZZZZZZ".into(),
            account: "a".into(),
            zone: "z".into(),
            customer_key: graftyard_release::public_key(&[7; 32]).hex(),
            evaluation: false,
            genesis: None,
            sites,
            rtt: Vec::new(),
            default_placement: None,
        }
    }

    type JoinLog = std::rc::Rc<std::cell::RefCell<Vec<(u8, u32)>>>;

    /// A fake member: reads `ms(from, peer, generation)` for every peer it joins with, and logs each join in `log`.
    fn fake(
        ms: impl Fn(SiteId, SiteId, u32) -> u16,
        log: JoinLog,
    ) -> impl Fn(SiteId, SiteId, SiteId, u32) -> Res<Vec<RttRow>> {
        move |from, a, b, g| {
            log.borrow_mut().push((from.0, g));
            Ok([a, b].iter().map(|&p| RttRow { from, peer: p, median_ms: ms(from, p, g), at: now() }).collect())
        }
    }

    #[test]
    fn a_pair_under_min_pair_rtt_is_measured_again_with_a_new_generation_then_refused() {
        // Site 3 claims weur but its member answers from enam's neighbourhood: 8 ms to site 0.
        let mut t = install(vec![
            site(0, Region::Enam, false),
            site(1, Region::Apac, false),
            site(2, Region::Wnam, false),
            site(3, Region::Weur, false),
        ]);
        let log = JoinLog::default();
        let join = fake(|f, p, _| if (f.0, p.0) == (3, 0) { 8 } else { 90 }, log.clone());
        let pairs = missing_pairs(&t, |a, _| a.id == SiteId(3));
        assert_eq!(pairs, vec![(SiteId(3), SiteId(0)), (SiteId(3), SiteId(1)), (SiteId(3), SiteId(2))]);
        let mut g = 10;
        let close = measure(&mut t, &join, &pairs, &mut g).unwrap();
        assert_eq!(close, vec![(SiteId(3), SiteId(0), 8)]);
        // Two joins for the three peers, then one retry at a higher generation.
        assert_eq!(*log.borrow(), vec![(3, 11), (3, 12), (3, 13)]);
    }

    #[test]
    fn a_retry_that_reads_far_enough_passes() {
        let mut t =
            install(vec![site(0, Region::Enam, false), site(1, Region::Weur, false), site(2, Region::Apac, false)]);
        // The first member object landed near site 0; the retry's new object did not.
        let join = fake(|f, p, g| if (f.0, p.0) == (2, 0) && g == 1 { 5 } else { 150 }, JoinLog::default());
        let pairs = missing_pairs(&t, |a, _| a.id == SiteId(2));
        let mut g = 0;
        assert_eq!(measure(&mut t, &join, &pairs, &mut g).unwrap(), vec![]);
        assert!(t.rtt.iter().all(|r| r.median_ms == 150), "{:?}", t.rtt);
    }

    #[test]
    fn spare_pairs_on_one_continent_are_not_measured() {
        let t = install(vec![
            site(0, Region::Enam, false),
            site(1, Region::Weur, false),
            site(2, Region::Apac, false),
            site(3, Region::Eeur, true),
        ]);
        let pairs = missing_pairs(&t, |a, _| a.id == SiteId(3));
        assert_eq!(pairs, vec![(SiteId(3), SiteId(0)), (SiteId(3), SiteId(2))]);
    }

    #[test]
    fn plan_refuses_a_changed_site_and_plans_nothing_on_a_rerun() {
        let t = install(vec![site(0, Region::Enam, false)]);
        let spec = |script: &str, region, spare| SiteSpec { script: script.into(), region, jurisdiction: None, spare };
        // A second site in a region is allowed (residency cells and spares outnumber regions); `Placement::new`
        // refuses to put two of them in one group.
        assert_eq!(plan(&t, &[spec("x", Region::Enam, true)]).unwrap()[0].id, SiteId(1));
        assert!(plan(&t, &[spec("s0", Region::Weur, false)]).is_err_and(|e| e.contains("does not change")));
        assert!(plan(&t, &[spec("s0", Region::Enam, false)]).unwrap().is_empty(), "a rerun plans nothing new");
        assert!(plan(&t, &[spec("Bad_Name", Region::Weur, false)]).is_err());
    }

    #[test]
    fn a_site_binds_only_its_own_git_shard_and_uploads_its_container() {
        let sites = vec![site(0, Region::Enam, false), site(1, Region::Weur, false), site(2, Region::Apac, false)];
        let t = install(sites.clone());
        let image = "registry.cloudflare.com/acct/git@sha256:00";
        let bundle = Bundle { files: Vec::new(), image: image.into(), hash: "h".into() };
        let refs: Vec<&Site> = sites.iter().collect();
        let b = desired_bindings(&t, &sites[1], &refs, &bundle);
        let shards: Vec<&Value> = b.iter().filter(|x| x["class_name"] == json!("GitShard")).collect();
        assert_eq!(shards.len(), 1, "{shards:?}");
        assert_eq!(shards[0]["name"], json!("SHARD_1"));
        assert!(shards[0].get("script_name").is_none(), "its own class, never another script's");
        let meta = metadata(&sites[1], b, migration("s1", None).unwrap(), image);
        assert_eq!(
            meta["containers"][0],
            json!({ "name": "s1-gitshard", "class_name": "GitShard", "images": { "git": image } })
        );
        assert_eq!(meta["limits"]["cpu_ms"], json!(CPU_MS));
        assert_eq!(meta["migrations"]["new_tag"], json!("v2"));
        assert_eq!(meta["migrations"]["steps"][1]["new_sqlite_classes"], json!(["GitShard"]));
    }

    #[test]
    fn a_v1_script_migrates_to_v2_by_adding_only_the_git_shard() {
        let m = migration("s0", Some("v1")).unwrap().unwrap();
        assert_eq!(m, json!({ "old_tag": "v1", "new_tag": "v2", "steps": [{ "new_sqlite_classes": ["GitShard"] }] }));
        assert_eq!(migration("s0", Some("v2")).unwrap(), None);
        assert!(migration("s0", Some("v9")).is_err());
        assert!(Bundle::read(Path::new("/nonexistent"), "git:latest").is_err_and(|e| e.contains("by digest")));
    }

    #[test]
    fn bindings_compare_without_api_ids_and_in_any_order() {
        let want = vec![
            json!({ "type": "plain_text", "name": "SITE_ID", "text": "0" }),
            json!({ "type": "durable_object_namespace", "name": "MEMBER_1", "class_name": "Member", "script_name": "s1" }),
        ];
        let have = vec![
            json!({ "type": "durable_object_namespace", "name": "MEMBER_1", "class_name": "Member", "script_name": "s1", "namespace_id": "x" }),
            json!({ "type": "plain_text", "name": "SITE_ID", "text": "0" }),
        ];
        assert!(same_bindings(&want, &have));
        let changed = vec![have[0].clone(), json!({ "type": "plain_text", "name": "SITE_ID", "text": "1" })];
        assert!(!same_bindings(&want, &changed));
    }
}
