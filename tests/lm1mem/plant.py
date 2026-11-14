#!/usr/bin/env python3
"""lm1-memory: planted bugs in this branch's new code (the lookup's overload answer and `suspect` damping, the member's
heavy-read cap with streamed answers, and the packed-dump cache; learner-dumps: one dump per state shared by every
read, learners exempt from HEAVY_READS, and a learner read that waits for a due rebuild). Plants one mutant at a time, runs the
tests that gate it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run
from the worktree root:

    python3 tests/lm1mem/plant.py          # every mutant
    python3 tests/lm1mem/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LK = 'crates/lookup/src/lib.rs'
MEM = 'workers/site/src/member.rs'
FAM = 'workers/site/src/family.rs'
HOST = 'workers/site/src/host.rs'
API = 'workers/edge/src/api.rs'
EDGE_LIB = 'workers/edge/src/lib.rs'
RAFT_INSTALL = 'crates/raft/src/member/install.rs'
LOOKUP = ['cargo', '-p', 'graftyard-lookup', '--lib', 'tests']
SITE = ['cargo', '-p', 'graftyard-site', '--lib', '']
EDGE = ['cargo', '-p', 'graftyard-edge', '--lib', 'a_failed_read']
EDGE_GATE = ['cargo', '-p', 'graftyard-edge', '--lib', 'a_shed_authz']
FOLD = ['cargo', '-p', 'graftyard-fold', '--test', 'install', 'a_learner_behind']

GO = 'containers/git/server/main.go'
CLOCK = 'tests/am1/rest/clock.mjs'
A1_SELF = ['node', 'tests/am1/rest/a1.mjs', '--selftest']
EDGE_T = ['cargo', '-p', 'graftyard-edge', '--lib', 'tests::a_']
SITE_SUM = ['cargo', '-p', 'graftyard-site', '--lib', 'a_summary_ranks']

MUTANTS = [
    ('lookup: an overloaded answer is taken as a missed member (suspect feeds the storm)', LK,
     '        return Answer::Overloaded(o.get("retryAfterMs").and_then(Value::as_u64).unwrap_or(250));',
     '        let _ = o;\n        return Answer::Missed("overloaded".into());', LOOKUP),
    ('lookup: an overloaded answer is returned to the caller as the read', LK,
     '    if let Some(o) = v.get("overloaded") {', '    if let Some(o) = v.get("overloaded").filter(|_| false) {', LOOKUP),
    ('lookup: a 5xx is no longer reported', LK,
     'if status >= 500 { Answer::Missed(status.to_string()) }', 'if status >= 600 { Answer::Missed(status.to_string()) }',
     LOOKUP),
    ('lookup: the overload reply drops `busy`, so an older lookup returns it as the read', LK,
     'json!({ "busy": true, "overloaded": { "retryAfterMs": retry_after_ms } })',
     'json!({ "overloaded": { "retryAfterMs": retry_after_ms } })', LOOKUP),
    ('damping: keyed without the term, so a new term\'s report is dropped', LK,
     '        match r.get(&(chain, about, term, all)) {', '        match r.get(&(chain, about, 0, all)) {', LOOKUP),
    ('damping: the gap never ends', LK, '            Some(at) if now < *at + gap => false,',
     '            Some(_) => false,', LOOKUP),
    ('cap: a plain read on a follower is shed instead of answering NotLeader', MEM,
     '(heavy && (leading || index) && !learner && heavies >= HEAVY_READS)', '(heavy && !learner && heavies >= HEAVY_READS)', SITE),
    ('cap: narrow reads are never capped (heavy only, as before)', MEM, '    if reads >= READS_MAX || (heavy',
     '    if (reads >= READS_MAX && heavy) || (heavy', SITE),
    ('cap: READS_MAX is off by one', MEM, '    if reads >= READS_MAX ||', '    if reads > READS_MAX ||', SITE),
    ('follower: HEAVY_READS does not hold on a follower', MEM, '(heavy && (leading || index) && !learner && heavies',
     '(heavy && leading && !learner && heavies', SITE),
    ('follower: a follower serves a read it may not (no NotLeader gate)', MEM,
     '    if index && !leading && !servable {', '    if index && !leading && !servable && false {', SITE),
    ('follower: the NotLeader gate runs after the caps, so a capped follower is retried, not left', MEM,
     '    if index && !leading && !servable {\n        return Gate::NotLeader;\n    }\n    if reads >= READS_MAX',
     '    if reads >= READS_MAX', SITE),
    ('follower: a late read-index read answers busy like the leader (lookup skips it, fine) - wait too long', MEM,
     '        (FOLLOWER_WAIT, graftyard_lookup::overloaded_reply(OVERLOAD_RETRY_MS))',
     '        (raft::CALL_DEADLINE * 3, graftyard_lookup::overloaded_reply(OVERLOAD_RETRY_MS))', SITE),
    ('follower: a late read-index read answers a stale value', MEM,
     '        (FOLLOWER_WAIT, graftyard_lookup::overloaded_reply(OVERLOAD_RETRY_MS))',
     '        (FOLLOWER_WAIT, json!({ "value": null, "index": 0 }))', SITE),
    ('follower: refs pages go to followers', FAM, '        None => false,\n    }\n}',
     '        None => q.get("refs").is_some(),\n    }\n}', SITE),
    ('follower: whole-tier rows are served while the metadata tier loads', FAM,
     '        Some(Value::Bool(true)) => meta_loaded,', '        Some(Value::Bool(true)) => true,', SITE),
    ('lookup: the cached leader is asked as a follower first', LK, ".filter(|(s, _)| Some(*s) != leader).collect()",
     ".filter(|(s, _)| Some(*s) != leader || true).collect()", LOOKUP),
    ('lookup: a follower that names the leader is taken as the answer', LK,
     '&& r.is_some_and(|(_, v)| v.pointer("/error/notLeader").is_none()',
     '&& r.is_some_and(|(_, v)| v.pointer("/error/notLeaderX").is_none()', LOOKUP),
    ('lookup: no rotation, every caller hits the same follower', LK, '            out.rotate_left(n);', '            let _ = n;', LOOKUP),
    ('round 4: a read the members shed falls back to the leader', LK,
     'if answers.iter().all(declined) { Spread::Leader } else { Spread::Shed }',
     'if answers.iter().all(declined) { Spread::Leader } else { Spread::Leader }', LOOKUP),
    ('round 4: a member that did not answer counts as declining, so the read goes to the leader', LK,
     '        None => false,\n    };\n    if answers', '        None => true,\n    };\n    if answers', LOOKUP),
    ('round 4: a 5xx counts as declining', LK, '        Some((s, _)) => (400..500).contains(s),',
     '        Some((s, _)) => (400..600).contains(s),', LOOKUP),
    ('round 4: learners are not asked', LK, '    let mut out = rotated(learners);',
     '    let mut out: Vec<Target> = Vec::new();\n    let _ = rotated(learners);', LOOKUP),
    ('round 4: followers before learners', LK,
     '    let mut out = rotated(learners);\n    out.extend(rotated(voters.into_iter().filter(|(s, _)| Some(*s) != leader).collect()));',
     '    let mut out = rotated(voters.into_iter().filter(|(s, _)| Some(*s) != leader).collect());\n    out.extend(rotated(learners));',
     LOOKUP),
    ('round 4: every learner is asked, however many', LK, '    out.truncate(SPREAD_TRIES);\n', '', LOOKUP),
    ('round 4: a config\'s learners are not learned', LK, '            e.learners = learners;',
     '            e.learners = Vec::new();\n            let _: &Vec<(SiteId, String)> = &learners;', LOOKUP),
    ('round 4: a shed read is 503 without Retry-After', API,
     '    Reply::error(503, &format!("{why} ({e})"), "rest").with("retry-after", retry)',
     '    let _ = retry;\n    Reply::error(503, &format!("{why} ({e})"), "rest")', EDGE),
    ('round 4: a shed read waits the 5 s of a failure, not 1 s', API,
     'if e.contains(graftyard_lookup::OVERLOADED) { "1" } else { "5" }', '{ "5" }', EDGE),
    ('round 4: a learner behind grants a read from its older fold', RAFT_INSTALL,
     '        for id in failed {\n            self.out.resolutions.push(Resolution::Read { id, result: Err(nl.clone()) });',
     '        for id in failed {\n            let g = grant(self);\n            self.out.resolutions.push(Resolution::Read { id, result: Ok(g) });', FOLD),
    ('round 5: the read gate drops a shed Authz read\'s reason (Retry-After 5, not 1)', EDGE_LIB,
     '        Err(e) => Err(format!("the owner\'s Authz chain did not answer: {e}")),',
     '        Err(_) => Err("the owner\'s Authz chain did not answer".into()),', EDGE_GATE),
    ('round 5: a shed Authz read is taken as a missing group', EDGE_LIB,
     '        Err(e) => Err(format!("the owner\'s Authz chain did not answer: {e}")),',
     '        Err(_) => Ok(graftyard_identity::AuthzFold::default()),', EDGE_GATE),
    ('shared: every read builds its own answer', HOST, '    if let Some(b) = built.get(&key) {',
     '    if let Some(b) = built.get(&key).filter(|_| false) {', SITE),
    ('shared: answers keyed by nothing, so another query gets the first answer', HOST,
     '    let key = q.to_string();', '    let key = String::new();\n    let _ = q;', SITE),
    ('cap: narrow row reads count as heavy', MEM, '    q.get("rows").is_some_and(Value::is_boolean)',
     '    q.get("rows").is_some()', SITE),
    ('stream: the shared value is skipped', MEM, '                1 => &shared,', '                1 => &[],', SITE),
    ('stream: the read is released at the first piece', MEM, '        if sent >= total {', '        if sent > 0 {', SITE),
    ('cap: a dropped read stays counted', MEM, '        self.0.set(self.0.get().saturating_sub(1));',
     '        let _ = self.0;', SITE),
    ('rebuilds: a valid cached dump still waits for the next slot', FAM,
     '    if DUMPS.with(|d| d.borrow().get(&repo).is_some_and(|(k, _)| *k == key)) {\n        return None;\n    }',
     '    let _ = key;', SITE),
    ('rebuilds: a let-through rebuild is not recorded, so every stale read rebuilds', FAM,
     '                r.insert(repo, now);\n                None', '                None', SITE),
    ('cache: a hit is served without comparing its key, so an applied entry is not in the next read', FAM,
     '.filter(|(at, _)| *at == key)', '.filter(|(at, _)| *at == key || true)', SITE),
    ('cache: the key is a constant', FAM, '    (c.applied.0, c.sum, fold.meta.loaded(), m.applied.0, m.sum)',
     '    let _ = (c, m);\n    (0, [0; 32], false, 0, [0; 32])', SITE),
    ('round 6: the trailer is the pack\'s first 20 bytes, so the shard picks another fan-out shard', EDGE_LIB,
     '.checked_sub(20).and_then(|at| <[u8; 20]>::try_from(&pack[at..]).ok())',
     '.checked_sub(20).and_then(|_| <[u8; 20]>::try_from(&pack[..20]).ok())', EDGE_T),
    ('round 6: a busy git cache refuses an agent push as an unpacker error', EDGE_LIB,
     '        ShardRefusal::Busy(s) => Refusal::agent_full(*s),',
     '        ShardRefusal::Busy(s) => Refusal::unpack_failed(&s.to_string()),', EDGE_T),
    ('round 6: an agent validate is not marked, so the container never sheds it', EDGE_LIB,
     '    if agent {\n        url.push_str("&class=agent");', '    if agent && false {\n        url.push_str("&class=agent");', EDGE_T),
    ('round 6: the summary ranks samples in arrival order', MEM, '    s.sort_unstable();\n    let at', '    let at', SITE_SUM),
    ('round 6: one FIFO lane, an agent queued first goes before a human', GO,
     '\tif len(g.humans) > 0 {\n\t\tw = g.humans[0]\n\t\tg.humans = g.humans[1:]\n\t} else if len(g.agents) > 0 {',
     '\tif len(g.agents) > 0 {\n\t\tw = g.agents[0]\n\t\tg.agents = g.agents[1:]\n\t} else if len(g.humans) > 0 {\n\t\tw = g.humans[0]\n\t\tg.humans = g.humans[1:]\n\t} else if false {',
     ['go', 'TestGate']),
    ('round 6: a waiter granted as its deadline fires drops the slot (leak)', GO,
     '\t\tif w.granted {\n', '\t\tif w.granted && false {\n', ['go', 'TestGate']),
    ('round 6: no refusal at once past 8 per slot', GO, '\tif agent && len(g.agents) >= agentQueuePerSlot*g.slots {',
     '\tif agent && len(g.agents) >= 1<<30 {', ['go', 'TestGate']),
    # learner-dumps: one dump per state shared by every read, learners without HEAVY_READS, waits for a due rebuild.
    ('learner: a follower is exempt from HEAVY_READS too', MEM,
     '    let learner = graftyard_types::learner_slot(me).is_some();\n    if index',
     '    let learner = !leading;\n    if index', SITE),
    ('learner: HEAVY_READS still holds on a learner', MEM, '(heavy && (leading || index) && !learner && heavies',
     '(heavy && (leading || index) && heavies', SITE),
    ('learner: READS_MAX does not hold on a learner', MEM, '    if reads >= READS_MAX || (heavy',
     '    if (reads >= READS_MAX && !learner) || (heavy', SITE),
    ('learner: a voter waits for a rebuild too', MEM, '    (learner || narrow) && index && now + ms < until',
     '    index && now + ms < until', SITE),
    ('learner: a read waits for a rebuild past its deadline', MEM, '    (learner || narrow) && index && now + ms < until',
     '    (learner || narrow) && index && now < until', SITE),
    ('dump: every read builds its own copy', FAM,
     '        && let Some(b) = DUMPS.with(|d| d.borrow().get(&repo)',
     '        && let Some(b) = DUMPS.with(|d| d.borrow().get(&repo).filter(|_| false)', SITE),
    ('dump: the cache keeps the bytes of another repository', FAM,
     '            d.insert(repo, (key, bytes.clone()));', '            d.insert(RepoId(DbId(0)), (key, bytes.clone()));', SITE),
    ('dump: a narrow rows read is answered with the dump', FAM,
     '    if !q.get("rows").is_some_and(Value::is_boolean) ||', '    if q.get("rows").is_none() ||', SITE),
    ('dump: learners space rebuilds as voters do', FAM,
     '    if graftyard_types::learner_slot(me).is_some() { LEARNER_DUMP_EVERY_MS } else { DUMP_EVERY_MS }',
     '    let _ = me;\n    DUMP_EVERY_MS', SITE),
    ('dump: voters lose their spacing', FAM,
     '    if graftyard_types::learner_slot(me).is_some() { LEARNER_DUMP_EVERY_MS } else { DUMP_EVERY_MS }',
     '    let _ = me;\n    LEARNER_DUMP_EVERY_MS', SITE),
    ('dump: the spacing ignores the member\'s own interval', FAM,
     '            Some(at) if now < at + every => Some(at + every - now),',
     '            Some(at) if now < at + DUMP_EVERY_MS => Some(at + DUMP_EVERY_MS - now),', SITE),
    ('host: the shared dump is skipped, so every grant serializes its own', HOST,
     '    if let Some(b) = F::read_bytes(fold, q) {', '    if let Some(b) = F::read_bytes(fold, q).filter(|_| false) {',
     SITE),
    # Round 7: request bodies are read before the network's lock (TestValidateBodyHoldsNoLock).
    ('round 7: validate spools its pack under the network lock', GO,
     '\tfile, sum, n, err := spool(filepath.Join(data, "spool"), body)\n\tif err != nil {\n\t\tfail(w, 500, "%v", err)\n\t\treturn\n\t}\n\tdefer os.Remove(file)\n\t// Shared: a validate writes only its own quarantine and reads the repository\'s objects and refs, so validates of\n\t// one network run side by side, and only the writers (ingest, refs, repack, init, evict), which take the lock\n\t// alone, wait for them. The one shared write, setting the quarantine aside under the pack\'s SHA-256, takes\n\t// `aside` (finishValidate).\n\tl := lock(net)\n\tl.RLock()\n\tdefer l.RUnlock()\n\tif _, err := os.Stat(rp); err != nil {\n\t\tfail(w, 404, "no repository for network %s", net)\n\t\treturn\n\t}\n\tsweepValidated(rp)\n',
     '\t// Shared: a validate writes only its own quarantine and reads the repository\'s objects and refs, so validates of\n\t// one network run side by side, and only the writers (ingest, refs, repack, init, evict), which take the lock\n\t// alone, wait for them. The one shared write, setting the quarantine aside under the pack\'s SHA-256, takes\n\t// `aside` (finishValidate).\n\tl := lock(net)\n\tl.RLock()\n\tdefer l.RUnlock()\n\tif _, err := os.Stat(rp); err != nil {\n\t\tfail(w, 404, "no repository for network %s", net)\n\t\treturn\n\t}\n\tsweepValidated(rp)\n\tfile, sum, n, err := spool(filepath.Join(data, "spool"), body)\n\tif err != nil {\n\t\tfail(w, 500, "%v", err)\n\t\treturn\n\t}\n\tdefer os.Remove(file)\n',
     ['go', 'TestValidateBodyHoldsNoLock']),
    ('round 7: POST /refs reads its body under the lock', GO,
     '\twant, order, err := readRefLines(r.Body, !put)\n\tif err != nil {\n\t\tfail(w, 400, "%v", err)\n\t\treturn\n\t}\n\tl := lock(net)\n\tl.Lock()\n\tdefer l.Unlock()\n',
     '\tl := lock(net)\n\tl.Lock()\n\tdefer l.Unlock()\n\twant, order, err := readRefLines(r.Body, !put)\n\tif err != nil {\n\t\tfail(w, 400, "%v", err)\n\t\treturn\n\t}\n',
     ['go', 'TestValidateBodyHoldsNoLock']),
    ('round 7: the repack holds the network lock alone again', GO,
     '\t\tif err := runRepack(rp); err != nil {\n',
     '\t\tl := lock(net)\n\t\tl.Lock()\n\t\tdefer l.Unlock()\n\t\tif err := runRepack(rp); err != nil {\n',
     ['go', 'TestRepackHoldsNoNetworkLock']),
    ('round 7: evict no longer waits out a running repack', GO,
     '\t// A repack holds no network lock (startRepack), so evict also refuses while one runs.\n\tif !repacking.TryLock() {\n',
     '\t// A repack holds no network lock (startRepack), so evict also refuses while one runs.\n\tif !repacking.TryLock() && false {\n',
     ['go', 'TestRepackHoldsNoNetworkLock']),
    ('round 7: every v2 request reads the tags (ls-refs reads the leader again)', 'crates/git/src/clone.rs',
     '        CloneKey::of(network, req, Digest([0; 32])).is_some_and(|k| k.tags.is_some())',
     '        let _ = (network, req);\n        true', ['cargo', '-p', 'graftyard-git', '--lib', 'only_a_cacheable']),
    ('round 7: a cacheable fetch without include-tag reads the tags', 'crates/git/src/clone.rs',
     '        CloneKey::of(network, req, Digest([0; 32])).is_some_and(|k| k.tags.is_some())',
     '        CloneKey::of(network, req, Digest([0; 32])).is_some()', ['cargo', '-p', 'graftyard-git', '--lib', 'only_a_cacheable']),
    ('round 7: a first poll leaves the ring unstarted (cursors before it are served as holds)', 'crates/git/src/feed.rs',
     '        if self.first.is_none() {\n            self.skip(repo, owner, core, (to, at));',
     '        if self.first.is_none() && false {\n            self.skip(repo, owner, core, (to, at));', ['cargo', '-p', 'graftyard-git', '--test', 'lease_gates', 'a_first_poll']),
    ('round 7: a later owner read restarts a serving ring', 'crates/git/src/feed.rs',
     '        if self.first.is_none() {\n            self.skip(repo, owner, core, (to, at));',
     '        if true {\n            self.skip(repo, owner, core, (to, at));', ['cargo', '-p', 'graftyard-git', '--test', 'lease_gates', 'a_first_poll']),
    ('round 7: lease rows drop the scan value and read nothing', 'crates/git/src/lease.rs',
     '            match v.first() {\n                Some(Val::Blob(b)) => Some(postcard::from_bytes(b).expect("a fold row this release wrote")),',
     '            match v.get(1) {\n                Some(Val::Blob(b)) => Some(postcard::from_bytes(b).expect("a fold row this release wrote")),', ['cargo', '-p', 'graftyard-git', '--test', 'lease_gates', 'a_first_poll']),
    ('round 7: a REST merge by number reads no pull request rows', 'crates/api/src/hosted.rs',
     '                MergeOf::Number(n) => Some(n),', '                MergeOf::Number(_) => None,',
     ['cargo', '-p', 'graftyard-api', '--test', 'surfaces', 'a_merge_plans']),
    ('round 7: a REST merge by number reads pull request 1, whatever it names', 'crates/api/src/hosted.rs',
     '                MergeOf::Number(n) => Some(n),', '                MergeOf::Number(_) => Some(IssueNumber(2)),',
     ['cargo', '-p', 'graftyard-api', '--test', 'surfaces', 'a_merge_plans']),
    ('round 7: a follower sends a REST merge read to the leader', 'workers/site/src/family.rs',
     '        Some(r) if graftyard_api::hosted::MergeOf::of_query(r).is_some() => meta_loaded,',
     '        Some(r) if r.get("merge").is_some() => meta_loaded,', ['cargo', '-p', 'graftyard-site', '--lib', 'a_follower_serves']),
    ('round 7: a merge waits for any catch-up in flight before it tries (2 s merge tails)', 'crates/git/src/merge_cache.rs',
     '    up().await?;\n    if let Some(a) = merge().await? {\n        return Ok(a);\n    }\n    in_flight().await;',
     '    up().await?;\n    in_flight().await;\n    if let Some(a) = merge().await? {\n        return Ok(a);\n    }',
     ['cargo', '-p', 'graftyard-git', '--lib', 'a_merge_waits']),
    ('round 7: a merge that misses catches up at once, racing the catch-up in flight', 'crates/git/src/merge_cache.rs',
     '        return Ok(a);\n    }\n    in_flight().await;', '        return Ok(a);\n    }',
     ['cargo', '-p', 'graftyard-git', '--lib', 'a_merge_waits']),
    ('round 8: every deadline miss of one follower stall counts (no dedupe)', 'crates/raft/src/member/health.rs',
     '                excused = true;\n', '                excused = false;\n',
     ['cargo', '-p', 'graftyard-raft', '--test', 'leadership', 'one_follower_stall']),
    ('round 8: misses are deduped forever, so a follower that keeps failing never trips', 'crates/raft/src/member/health.rs',
     'sent_at >= *l && sent_at < after(*l, CALL_DEADLINE)', 'sent_at >= *l',
     ['cargo', '-p', 'graftyard-raft', '--test', 'leadership', 'one_follower_stall']),
    ('merge-p99: a merge\'s rows read is spaced as a dump rebuild again', FAM,
     'let dump = q.get("rows").is_some_and(Value::is_boolean) && q.get("packed")', 'let dump = q.get("packed")',
     ['cargo', '-p', 'graftyard-site', '--lib', 'a_merge_rows_read']),
    ('merge-p99: a whole-tier dump is no longer spaced', FAM,
     'let dump = q.get("rows").is_some_and(Value::is_boolean) && q.get("packed")',
     'let dump = q.get("rows").is_some_and(|r| !r.is_boolean()) && q.get("packed")',
     ['cargo', '-p', 'graftyard-site', '--lib', 'a_merge_rows_read']),
    ('merge-p99: mergeSplit counts tier and authz in series, so the untimed gap reads short', 'tests/am1/rest/a1.mjs',
     '- Math.max(d.tier ?? 0, d.authz ?? 0) -', '- (d.tier ?? 0) - (d.authz ?? 0) -',
     ['node', 'tests/am1/rest/a1.mjs', '--selftest']),
    ('merge-p99: a read ahead is kept for another repository under the name', API,
     '(Some(Ok(Some(r))), Some(g), Some((rows, authz))) if r == g =>',
     '(Some(Ok(Some(r))), Some(g), Some((rows, authz))) if r.owner == g.owner =>',
     ['cargo', '-p', 'graftyard-edge', '--lib', 'a_write_keeps']),
    ('merge-p99: a read ahead is kept when the placement moved', API,
     '(Some(Ok(Some(r))), Some(g), Some((rows, authz))) if r == g =>',
     '(Some(Ok(Some(r))), Some(g), Some((rows, authz))) if r.id == g.id =>',
     ['cargo', '-p', 'graftyard-edge', '--lib', 'a_write_keeps']),
    ('merge-p99: a REST merge path read on followers matches any PUT under pulls', API,
     '(Method::Put, ["pulls", n, "merge"]) => {\n            n.parse().ok().map(|n| MergeOf::Number',
     '(Method::Put, ["pulls", n, ..]) => {\n            n.parse().ok().map(|n| MergeOf::Number',
     ['cargo', '-p', 'graftyard-edge', '--lib', 'only_a_rest_merge']),
    # L0-12's clock probe (tests/am1/rest/clock.mjs), gated by a1.mjs --selftest.
    ('clock: a slow sample is widened, not rejected (rows-m19 read 3.2 s)', CLOCK,
     'return xs.filter((c) => c.rtt <= best.get(c.site) + CLOCK_RTT_OVER);', 'return xs;', A1_SELF),
    ('clock: a flat 200 ms cap, not one over each site\'s floor (rejects every apac sample)', CLOCK,
     'c.rtt <= best.get(c.site) + CLOCK_RTT_OVER', 'c.rtt <= CLOCK_RTT_OVER', A1_SELF),
    ('clock: the interval is the whole round trip, not half of it', CLOCK,
     'const hi = (c) => c.offset + c.rtt / 2, lo = (c) => c.offset - c.rtt / 2;',
     'const hi = (c) => c.offset + c.rtt, lo = (c) => c.offset - c.rtt;', A1_SELF),
    ('clock: a member\'s own interval counts as spread', CLOCK,
     'r.filter((b) => b.site !== a.site)', 'r.filter(() => true)', A1_SELF),
    ('clock: a round with one member counts', CLOCK,
     'new Set(r.map((c) => c.site)).size > 1', 'r.length > 0', A1_SELF),
    # Round 10's queue split (`workers/site/src/host.rs`, `queue_split`).
    ('queue split: the slot runs to the append, not to the apply that freed it', HOST,
     '    let freed = last_apply.clamp(admitted, appended);', '    let freed = appended.max(admitted);',
     ['cargo', '-p', 'graftyard-site', '--lib', 'a_queue_splits']),
    ('queue split: an apply before admission counts as slot wait', HOST,
     '    let freed = last_apply.clamp(admitted, appended);', '    let freed = last_apply.min(appended).max(1_000);',
     ['cargo', '-p', 'graftyard-site', '--lib', 'a_queue_splits']),
    # Round 12's confirm split (`workers/site/src/host.rs`, `confirm_split_summary`, `stmt_bytes`).
    ('confirm split: a transaction at a group\'s bound stays in the group below', HOST,
     'xs.iter().filter(|c| c.bytes >= lo && c.bytes < hi)', 'xs.iter().filter(|c| c.bytes > lo && c.bytes <= hi)',
     ['cargo', '-p', 'graftyard-site', '--lib', 'a_confirm_split']),
    ('confirm split: storage is p99(sync) less p99(turn), not per sample', HOST,
     '"storage": f(|c| c.sync.saturating_sub(c.turn)),',
     '"storage": json!({ "p99": f(|c| c.sync)["p99"].as_u64().unwrap_or(0).saturating_sub(f(|c| c.turn)["p99"].as_u64().unwrap_or(0)) }),',
     ['cargo', '-p', 'graftyard-site', '--lib', 'a_confirm_split']),
    ('confirm split: integer arguments count no bytes', HOST,
     '            _ => 8,\n        })', '            _ => 0,\n        })',
     ['cargo', '-p', 'graftyard-site', '--lib', 'a_transaction_counts']),
]

ENV = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0',
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', '/home/user/workspace/repo/.worktrees/target-rows'))


def run(t):
    if t[0] == 'node':
        r = subprocess.run(t, cwd=ROOT, capture_output=True, text=True, timeout=120)
        out = r.stdout + r.stderr
        return r.returncode, out, 'SyntaxError' in out
    if t[0] == 'go':
        r = subprocess.run(['go', 'test', '-count=1', '-run', t[1], '.'], cwd=os.path.join(ROOT, 'containers/git/server'),
                           capture_output=True, text=True, timeout=600)
        out = r.stdout + r.stderr
        return r.returncode, out, 'build failed' in out or 'syntax error' in out or 'undefined:' in out
    _, _, pkg, kind, name, *rest = t
    if kind == '--lib':
        args = ['cargo', 'test', '-q', '-p', pkg, '--lib', name]
    else:
        args = ['cargo', 'test', '-q', '-p', pkg, '--test', name] + [f for f in rest if f]
    r = subprocess.run(args, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1200)
    out = r.stdout + r.stderr
    return r.returncode, out, 'error[E' in out or 'could not compile' in out


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, t = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            code, out, broke = run(t)
        finally:
            open(p, 'wb').write(orig)
        if broke:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif code != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ') or l.startswith('--- FAIL')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
