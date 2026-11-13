#!/usr/bin/env python3
"""M5-WP1's planted bugs: removes one `Release` group refusal at a time in crates/release/src, runs the crate's tests,
restores the file, and prints which plant was caught. Every plant must be caught. plant.py [I] runs only plant I.

Edits the work tree while it runs, so run it on a clean tree."""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../.."))
G = "crates/release/src/group.rs"
env = dict(os.environ, PATH=os.path.expanduser("~/.cargo/bin") + ":" + os.environ["PATH"])

PLANTS = [
    ("unknown keys: a signer outside the latest Keys is taken as a customer",
     G, "        None\n    }\n}\n\nfn verify_raw", "        Some(Role::Customer)\n    }\n}\n\nfn verify_raw"),
    ("wrong generations: any Keys generation commits",
     G, "if new.generation != want {", "if new.generation != want && false {"),
    ("intents past not_after commit",
     G, "if at > i.not_after {", "if at > i.not_after && false {"),
    ("an intent naming a release with no ReleaseUploaded commits",
     G, "self.uploads.get(&r.0).ok_or(Refusal::NoUpload(*r))", "Ok(self.uploads.get(&r.0).unwrap_or_else(|| self.uploads.values().next().expect(\"one upload\")))"),
    ("a release that cannot read an enabled format is accepted",
     G, "match self.enabled.iter().find(|f| !u.reads.contains(f)) {", "match self.enabled.iter().find(|f| !u.reads.contains(f) && false) {"),
    ("a second current intent commits",
     G, "return Err(Refusal::IntentCurrent(c.intent.id));", "let _ = c;"),
    ("the drain key drains a second site",
     G, "return Err(Refusal::DrainGuard(SiteId(*other)));", "let _ = other;"),
    ("an EdgeRoll drops an undo's version",
     G, "return Err(Refusal::DropsUndoVersion(pin));", "let _ = pin;"),
    ("a stale prev commits",
     G, "if b.prev != Some(last) {", "if b.prev.is_none() && b.prev != Some(last) {"),
    ("an undo outside the live list commits",
     G, "if !c.undo.iter().any(|(u, s)| *u == e.body && s == sig) || *key != c.undo_key {", "if *key != c.undo_key && false {"),
    ("an undo replays",
     G, "return Err(Refusal::UndoUsed(e.body.kind()));", "let _ = 0;"),
    ("the executor key signs a ReplaceMember of an undrained site",
     G, "return Err(Refusal::NotLost(*lost));", "let _ = lost;"),
    ("the executor key signs intents",
     G, "_ => need(&[Vendor, Customer]),", "_ => need(&[Vendor, Customer, Executor]),"),
    ("a forged ReleaseUploaded: the record signature is not checked",
     G, "verify_raw(&self.keys().release, &u.record, &u.record_sig)", "Ok::<(), Refusal>(())"),
    ("a vendor intent resumes after Keys { vendor: None } with no re-signed undo",
     G, "&& undo.is_empty() {", "&& undo.is_empty() && false {"),
    ("FormatsEnable without the release-signing half",
     G, "verify_raw(&r.0, &formats_enable_message(*format), &r.1)", "Ok(())"),
    ("steps out of order",
     G, "if *step != c.next {", "if *step != c.next && false {"),
]


def run(i):
    name, path, old, new = PLANTS[i]
    full = os.path.join(ROOT, path)
    src = open(full).read()
    if src.count(old) != 1:
        return f"plant {i} ({name}): pattern found {src.count(old)} times"
    open(full, "w").write(src.replace(old, new))
    try:
        r = subprocess.run(["cargo", "test", "-q", "-p", "graftyard-release"], cwd=ROOT, env=env, capture_output=True, text=True)
    finally:
        open(full, "w").write(src)
    # `-q` prints the failing tests as indented names under "failures:", after their output.
    lines = r.stdout.splitlines()
    tail = lines[len(lines) - 1 - lines[::-1].index("failures:"):] if "failures:" in lines else []
    failed = [l.strip().removeprefix("tests::") for l in tail if l.startswith("    tests::")]
    built = "error[" not in r.stderr
    caught = r.returncode != 0 and built and bool(failed)
    return f"plant {i} ({name}): {'caught by ' + ', '.join(failed) if caught else 'NOT CAUGHT' if built else 'does not build'}"


if __name__ == "__main__":
    which = [int(sys.argv[1])] if len(sys.argv) > 1 else range(len(PLANTS))
    out = [run(i) for i in which]
    print("\n".join(out))
    sys.exit(0 if all("caught by" in l for l in out) else 1)
