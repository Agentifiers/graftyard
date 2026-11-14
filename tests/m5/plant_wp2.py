#!/usr/bin/env python3
"""M5-WP2's planted bugs, in plant.py's style: breaks one rule of the cached `Release` view, pin-only pointers, the
roll steps, or uncommitted `StaleMode` at a time in crates/release/src, runs the crate's tests, restores the file, and
prints which test caught the plant. Every plant must be caught. plant_wp2.py [I] runs only plant I.

Edits the work tree while it runs, so run it on a clean tree."""
import os, sys

sys.path.insert(0, os.path.dirname(__file__))
import plant  # noqa: E402

G = "crates/release/src/group.rs"
V = "crates/release/src/view.rs"

plant.PLANTS = [
    ("a replica re-applies entries a member behind it sends",
     V, "for (bytes, at) in entries.iter().skip(skip) {", "for (bytes, at) in entries.iter().skip(skip * 0) {"),
    ("a replica takes a page past its count (a gap)",
     V, "if start > self.fold.count {", "if start > self.fold.count + 1_000 {"),
    ("a replica applies entries without the group's check",
     V, "self.fold.check(anchor, *at, &ev)?;", "let _ = (anchor, at);"),
    ("the next roll of a pinned script keeps the pin",
     G, "                    if let Some(slot) = self.pointers.slot(s) {\n                        *slot = None;\n                    }",
     "                    let _ = self.pointers.slot(s);"),
    ("an undo leaves the deployment as the roll left it, so the next roll may drop the pinned release",
     G, "                        self.deployed.insert(script, (Some(rolled), pin));", "                        let _ = rolled;"),
    ("a roll steps past its ROLL_STEPS",
     G, "&& usize::from(*step) >= steps.len()", "&& usize::from(*step) > steps.len()"),
    ("a FrontRoll carries an undo",
     G, "Intent::FrontRoll { .. } if !i.undo.is_empty() => {", "Intent::FrontRoll { .. } if !i.undo.is_empty() && false => {"),
    ("graft-front stays silent when a pinned call reports no version",
     V, "Some(v) => ran.and_then(|r| r.parse::<VersionId>().ok()) != Some(v),",
     "Some(v) => ran.and_then(|r| r.parse::<VersionId>().ok()).is_some_and(|r| r != v),"),
    ("the version override header is not a Structured Fields string",
     V, 'format!("{script}=\\"{v}\\"")', 'format!("{script}={v}")'),
    ("a request waits on a group whose refresh failed",
     V, "        if self.failing {\n            return Need::Refresh;\n        }", "        let _ = self.failing;"),
    ("a request waits behind a refresh beside requests that hangs",
     V, "Some((r, false)) if now < r + ABANDON_MS => return Need::Use,", "Some((r, false)) if now < r + RETRY_MS => return Need::Wait,"),
    ("a view is refreshed only once it is POINTER_TTL old, so a pin can take longer to land",
     V, "self.next = start + POINTER_TTL_MS - RETRY_MS;", "self.next = start + POINTER_TTL_MS;"),
    ("an uncommitted StaleMode is taken from a key other than the customer's",
     V, "    if *key != keys.customer {\n        return Err(Refusal::WrongSigner(\"StaleMode\"));",
     "    if !keys.vendor.iter().chain([&keys.customer]).any(|k| k == key) {\n        return Err(Refusal::WrongSigner(\"StaleMode\"));"),
    ("an uncommitted StaleMode whose until has passed is honored",
     V, "    if until <= now {", "    if until < now {"),
    ("an uncommitted StaleMode bound to a parent intent is taken",
     V, "    if e.bound.parent.is_some() {", "    if e.bound.parent.is_some() && false {"),
    ("an uncommitted StaleMode for another install is taken",
     V, "    if e.bound.install != g.install {\n        return Err(Refusal::WrongInstall { want: g.install, got: e.bound.install });\n    }\n    if e.bound.parent",
     "    if e.bound.parent"),
    ("an uncommitted flag is honored past STALE_MAX",
     V, "Timestamp(self.until.0.min(self.arrived.0.saturating_add(stale_max)))", "Timestamp(self.until.0.max(self.arrived.0.saturating_add(stale_max)))"),
    ("a later member's arrival moves an uncommitted flag's bound later",
     V, "f.arrived = Timestamp(f.arrived.0.min(arrived.0));", "f.arrived = arrived;"),
    ("a committed StaleMode is not honored",
     V, "let committed = self.fold.stale.iter().map(|(s, u)| (*s, *u));", "let committed = self.fold.stale.iter().take(0).map(|(s, u)| (*s, *u));"),
    ("a Repo flag covers every repository",
     V, "(Scope::Repo(r), ChainId::Repo(c) | ChainId::RepoMeta(c)) => r == c,", "(Scope::Repo(_), ChainId::Repo(_) | ChainId::RepoMeta(_)) => true,"),
    ("an Owner flag covers another owner's chains",
     V, "(Scope::Owner(a), ChainId::Authz(o) | ChainId::Identity(o) | ChainId::Actions(o, _)) => a == o,",
     "(Scope::Owner(_), ChainId::Authz(_) | ChainId::Identity(_) | ChainId::Actions(_, _)) => true,"),
    ("the edge's precheck passes a signer outside the cached Keys",
     V, "if !e.body.undo_only() && !known(k) && !extra {", "if !e.body.undo_only() && !known(k) && !extra && false {"),
    ("the edge's precheck skips signatures",
     V, "            crate::group::verify_raw(k, &msg, sig)?;", "            let _ = (&msg, sig);"),
]

if __name__ == "__main__":
    which = [int(sys.argv[1])] if len(sys.argv) > 1 else range(len(plant.PLANTS))
    out = [plant.run(i) for i in which]
    print("\n".join(out))
    sys.exit(0 if all("caught by" in l for l in out) else 1)
