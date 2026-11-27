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
MC = 'crates/git/src/merge_cache.rs'
SH = 'crates/git/src/shard.rs'
LA = 'crates/git/src/log_archive.rs'
CU = 'crates/git/src/catchup.rs'
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
    # report_due keys a report by (chain, member, term) now; the `all` flag went with the per-call key.
    ("damping: keyed without the term, so a new term's report is dropped", LK,
     '        match r.get(&(chain, about, term)) {',
     '        match r.get(&(chain, about, 0)) {', LOOKUP),
    ('damping: the gap never ends', LK, '            Some(at) if now < *at + gap => false,',
     '            Some(_) => false,', LOOKUP),
    # gate() returns one Shed per cap now (ShedGate), and the NotLeader gate also turns off-leader reads away.
    ('cap: a plain read on a follower is shed instead of answering NotLeader', MEM,
     '    if heavy && (leading || index) && !learner && heavies >= HEAVY_READS {',
     '    if heavy && !learner && heavies >= HEAVY_READS {', SITE),
    ('cap: narrow reads are never capped (heavy only, as before)', MEM,
     '    if reads >= READS_MAX {',
     '    if reads >= READS_MAX && heavy {', SITE),
    ('cap: READS_MAX is off by one', MEM,
     '    if reads >= READS_MAX {',
     '    if reads > READS_MAX {', SITE),
    ('follower: HEAVY_READS does not hold on a follower', MEM,
     '    if heavy && (leading || index) && !learner && heavies >= HEAVY_READS {',
     '    if heavy && leading && !learner && heavies >= HEAVY_READS {', SITE),
    ('follower: a follower serves a read it may not (no NotLeader gate)', MEM,
     '    if index && (if leading { off_leader } else { !servable }) {',
     '    if index && (if leading { off_leader } else { false }) {', SITE),
    ('follower: the NotLeader gate runs after the caps, so a capped follower is retried, not left', MEM,
     '    if index && (if leading { off_leader } else { !servable }) {\n        return Gate::NotLeader;\n    }\n    if reads >= READS_MAX {\n        return Gate::Shed(ShedGate::ReadsMax);\n    }\n',
     '    if reads >= READS_MAX {\n        return Gate::Shed(ShedGate::ReadsMax);\n    }\n    if index && (if leading { off_leader } else { !servable }) {\n        return Gate::NotLeader;\n    }\n', SITE),
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
    # timed_spread_order (learner scores, ROUTE_MARGIN) replaced spread_order; its rules are the same.
    ('lookup: the cached leader is asked as a follower first', LK,
     '        voters.into_iter().filter(|(s, _)| Some(*s) != leader).map(|(site, do_name)| Target { site, do_name });',
     '        voters.into_iter().filter(|(s, _)| Some(*s) != leader || true).map(|(site, do_name)| Target { site, do_name });', LOOKUP),
    ('lookup: a follower that names the leader is taken as the answer', LK,
     '&& r.is_some_and(|(_, v)| v.pointer("/error/notLeader").is_none()',
     '&& r.is_some_and(|(_, v)| v.pointer("/error/notLeaderX").is_none()', LOOKUP),
    ('lookup: no rotation, every caller hits the same follower', LK,
     '            v.rotate_left(n);',
     '            let _ = n;', LOOKUP),
    # spread_verdict replaced the inline verdict; a read the members shed is Spread::Shed with a wait.
    ('round 4: a read the members shed falls back to the leader', LK,
     '    Spread::Shed(wait.max().unwrap_or(OVERLOAD_RETRY_MS).clamp(50, 2_000))',
     '    let _ = wait;\n    Spread::Leader', LOOKUP),
    ('round 4: a member that did not answer counts as declining, so the read goes to the leader', LK,
     'if (400..500).contains(s));',
     'if (400..500).contains(s)) || r.is_none();', LOOKUP),
    ('round 4: a 5xx counts as declining', LK,
     'if (400..500).contains(s));',
     'if (400..600).contains(s));', LOOKUP),
    ('round 4: learners are not asked', LK,
     '        rotated(near, pick).into_iter().chain(far).map(|(site, do_name, _)| Target { site, do_name }).collect();',
     '        rotated(near, pick).into_iter().chain(far).map(|(site, do_name, _)| Target { site, do_name }).take(0).collect();', LOOKUP),
    ('round 4: followers before learners', LK,
     '    out.extend(rotated(followers.collect(), pick));',
     '    out.splice(0..0, rotated(followers.collect(), pick));', LOOKUP),
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
     '        for id in failed {\n            let g = grant(self, applied);\n            self.out.resolutions.push(Resolution::Read { id, result: Ok(g) });', FOLD),
    ('round 5: the read gate drops a shed Authz read\'s reason (Retry-After 5, not 1)', EDGE_LIB,
     '        Err(e) => Err(format!("the owner\'s Authz chain did not answer: {e}")),',
     '        Err(_) => Err("the owner\'s Authz chain did not answer".into()),', EDGE_GATE),
    ('round 5: a shed Authz read is taken as a missing group', EDGE_LIB,
     '        Err(e) => Err(format!("the owner\'s Authz chain did not answer: {e}")),',
     '        Err(_) => Ok(graftyard_identity::AuthzFold::default()),', EDGE_GATE),
    ('shared: every read builds its own answer', HOST, '    if let Some(b) = built.get(&key) {',
     '    if let Some(b) = built.get(&key).filter(|_| false) {', SITE),
    # shared_answer keys by floor and query now; the floor stays so only the query is dropped.
    ('shared: answers keyed by nothing, so another query gets the first answer', HOST,
     '    let key = format!("{floor} {q}");',
     '    let key = format!("{floor}");\n    let _ = q;', SITE),
    ('cap: narrow row reads count as heavy', MEM, '    q.get("rows").is_some_and(Value::is_boolean)',
     '    q.get("rows").is_some()', SITE),
    ('stream: the shared value is skipped', MEM, '                1 => &shared,', '                1 => &[],', SITE),
    ('stream: the read is released at the first piece', MEM, '        if sent >= total {', '        if sent > 0 {', SITE),
    ('cap: a dropped read stays counted', MEM, '        self.0.set(self.0.get().saturating_sub(1));',
     '        let _ = self.0;', SITE),
    # The packed-dump cache is keyed by DumpAt (applied, loaded, child) and a hit is one it covers (DumpAt::covers).
    ('rebuilds: a valid cached dump still waits for the next slot', FAM,
     '    if dump_hit(fold, floor).is_some() {\n        return None;\n    }\n    REBUILT',
     '    REBUILT', SITE),
    ('rebuilds: a let-through rebuild is not recorded, so every stale read rebuilds', FAM,
     '                r.insert(repo, now);\n                None', '                None', SITE),
    ('cache: a hit is served without comparing its key, so an applied entry is not in the next read', FAM,
     '.filter(|(at, _)| at.covers(fold, floor))',
     '.filter(|(_, _)| true)', SITE),
    # DumpAt replaced the hashed key: a key that always matches is a covers() that always holds.
    ('cache: the key is a constant', FAM,
     '        self.loaded == now.loaded && self.child == now.child && self.applied >= floor',
     '        let _ = (now, floor);\n        true', SITE),
    ('round 6: the trailer is the pack\'s first 20 bytes, so the shard picks another fan-out shard', EDGE_LIB,
     '.checked_sub(20).and_then(|at| <[u8; 20]>::try_from(&pack[at..]).ok())',
     '.checked_sub(20).and_then(|_| <[u8; 20]>::try_from(&pack[..20]).ok())', EDGE_T),
    ('round 6: a busy git cache refuses an agent push as an unpacker error', EDGE_LIB,
     '        ShardRefusal::Busy(s) => Some(Refusal::agent_full(*s)),',
     '        ShardRefusal::Busy(s) => Some(Refusal::unpack_failed(&s.to_string())),', EDGE_T),
    ('round 6: an agent validate is not marked, so the container never sheds it', EDGE_LIB,
     '    if agent {\n        url.push_str("&class=agent");', '    if agent && false {\n        url.push_str("&class=agent");', EDGE_T),
    ('round 6: the summary ranks samples in arrival order', MEM, '    s.sort_unstable();\n    let at', '    let at', SITE_SUM),
    ('round 6: one FIFO lane, an agent queued first goes before a human', GO,
     '\tif len(g.humans) > 0 {\n\t\tw = g.humans[0]\n\t\tg.humans = g.humans[1:]\n\t} else if len(g.agents) > 0 {',
     '\tif len(g.agents) > 0 {\n\t\tw = g.agents[0]\n\t\tg.agents = g.agents[1:]\n\t} else if len(g.humans) > 0 {\n\t\tw = g.humans[0]\n\t\tg.humans = g.humans[1:]\n\t} else if false {',
     ['go', 'TestGate']),
    ('round 6: a waiter granted as its deadline fires drops the slot (leak)', GO,
     '\t\tif w.granted {\n', '\t\tif w.granted && false {\n', ['go', 'TestGate']),
    # G40 added the drain bound beside the per-slot limit; this plants the limit alone.
    ('round 6: no refusal at once past 8 per slot', GO,
     '\tif agent && (len(g.agents) >= agentQueuePerSlot*g.slots || g.drainLocked(len(g.agents)) > wait) {',
     '\tif agent && (len(g.agents) >= 1<<30 || g.drainLocked(len(g.agents)) > wait) {', ['go', 'TestGate']),
    # learner-dumps: one dump per state shared by every read, learners without HEAVY_READS, waits for a due rebuild.
    ('learner: a follower is exempt from HEAVY_READS too', MEM,
     '    let learner = graftyard_types::learner_slot(me).is_some();\n    if index',
     '    let learner = !leading;\n    if index', SITE),
    ('learner: HEAVY_READS still holds on a learner', MEM,
     '    if heavy && (leading || index) && !learner && heavies >= HEAVY_READS {',
     '    if heavy && (leading || index) && heavies >= HEAVY_READS {', SITE),
    ('learner: READS_MAX does not hold on a learner', MEM, '    if reads >= READS_MAX {',
     '    if reads >= READS_MAX && !learner {', SITE),
    ('learner: a voter waits for a rebuild too', MEM, '    (learner || narrow) && index && now + ms < until',
     '    index && now + ms < until', SITE),
    ('learner: a read waits for a rebuild past its deadline', MEM, '    (learner || narrow) && index && now + ms < until',
     '    (learner || narrow) && index && now < until', SITE),
    ('dump: every read builds its own copy', FAM,
     '    if let Some(b) = dump_hit(fold, floor) {\n        return Some(b);\n    }\n    let at = DumpAt::of(fold);',
     '    let at = DumpAt::of(fold);', SITE),
    ('dump: the cache keeps the bytes of another repository', FAM,
     '            d.insert(repo, (at, bytes.clone()));',
     '            d.insert(RepoId(DbId(0)), (at, bytes.clone()));', SITE),
    # is_dump() now decides both dump_bytes and the rebuild spacing, so #55, #76 and #77 plant it.
    ('dump: a narrow rows read is answered with the dump', FAM,
     '    q.get("rows").is_some_and(Value::is_boolean) && q.get("packed").and_then(Value::as_bool) == Some(true)\n}',
     '    q.get("rows").is_some() && q.get("packed").and_then(Value::as_bool) == Some(true)\n}', SITE),
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
     '    if let Some(b) = F::read_bytes(fold, q, floor) {',
     '    if let Some(b) = F::read_bytes(fold, q, floor).filter(|_| false) {', SITE),
    # Round 7: request bodies are read before the network's lock (TestValidateBodyHoldsNoLock).
    # Validates and the additive writers share the lock now (writeLock), so "under the lock" is the lock taken alone,
    # which the ingest and ls-refs behind it wait on, as the shared lock did when writers took it alone.
    ('round 7: validate spools its pack under the network lock', GO,
     '\t\tvar err error\n\t\tif file, sum, n, err = spool(filepath.Join(data, "spool"), r.Body); err != nil {',
     '\t\tvar err error\n\t\tl := lock(net)\n\t\tl.RLock()\n\t\tl.RUnlock()\n\t\tl.Lock()\n\t\tdefer l.Unlock()\n\t\tif file, sum, n, err = spool(filepath.Join(data, "spool"), r.Body); err != nil {', ['go', 'TestValidateBodyHoldsNoLock']),
    # /refs takes writeLock (shared) after reading its body; the plant takes the lock alone while the body arrives.
    ('round 7: POST /refs reads its body under the lock', GO,
     '\twant, order, err := readRefLines(r.Body, !put)\n\tif err != nil {\n\t\tfail(w, 400, "%v", err)\n\t\treturn\n\t}\n',
     '\tl0 := lock(net)\n\tl0.Lock()\n\twant, order, err := readRefLines(r.Body, !put)\n\tl0.Unlock()\n\tif err != nil {\n\t\tfail(w, 400, "%v", err)\n\t\treturn\n\t}\n', ['go', 'TestValidateBodyHoldsNoLock']),
    # runRepack takes the maintenance ctx (maint-yield).
    ('round 7: the repack holds the network lock alone again', GO,
     '\t\tif err := runRepack(ctx, rp); err != nil {\n',
     '\t\tl := lock(net)\n\t\tl.Lock()\n\t\tdefer l.Unlock()\n\t\tif err := runRepack(ctx, rp); err != nil {\n', ['go', 'TestRepackHoldsNoNetworkLock']),
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
    # catchup-pieces (32e7a48c) replaced in_flight/catch_up with `turn`, one piece of the catch-ups at a time.
    ('round 7: a merge waits for any catch-up in flight before it tries (2 s merge tails)', MC,
     '        up().await?;\n        if let Some(a) = merge().await? {\n            return Ok(a);\n        }\n        loop {',
     '        up().await?;\n        turn().await?;\n        if let Some(a) = merge().await? {\n            return Ok(a);\n        }\n        loop {', ['cargo', '-p', 'graftyard-git', '--lib', 'merge_cache']),
    # Dropped: "a merge that misses catches up at once, racing the catch-up in flight". catchup-pieces
    # (32e7a48c) made the merge's own catch-up the shard's piece turn (`turn`): a miss no longer starts a catch-up
    # beside the one in flight, so there is no race to plant. #72 plants waiting before the first try.
    ('round 8: every deadline miss of one follower stall counts (no dedupe)', 'crates/raft/src/member/health.rs',
     '                excused = true;\n', '                excused = false;\n',
     ['cargo', '-p', 'graftyard-raft', '--test', 'leadership', 'one_follower_stall']),
    ('round 8: misses are deduped forever, so a follower that keeps failing never trips', 'crates/raft/src/member/health.rs',
     'sent_at >= *l && sent_at < after(*l, CALL_DEADLINE)', 'sent_at >= *l',
     ['cargo', '-p', 'graftyard-raft', '--test', 'leadership', 'one_follower_stall']),
    ("merge-p99: a merge's rows read is spaced as a dump rebuild again", FAM,
     '        is_dump(q).then(|| dump_wait(fold, floor, now, every)).flatten()',
     '        q.get("packed").and_then(Value::as_bool).filter(|p| *p).and_then(|_| dump_wait(fold, floor, now, every))', ['cargo', '-p', 'graftyard-site', '--lib', 'a_merge_rows_read']),
    ('merge-p99: a whole-tier dump is no longer spaced', FAM,
     '        is_dump(q).then(|| dump_wait(fold, floor, now, every)).flatten()',
     '        (!is_dump(q)).then(|| dump_wait(fold, floor, now, every)).flatten()', ['cargo', '-p', 'graftyard-site', '--lib', 'a_merge_rows_read']),
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
    # Round 14's admission and large-transaction instruments (`workers/site/src/host.rs`).
    ('admitted bytes: an Agent command is priced with its inline pack', HOST,
     '            A::Agent => ("agent", len.saturating_sub(inline)),', '            A::Agent => ("agent", len),',
     ['cargo', '-p', 'graftyard-site', '--lib', 'admitted_bytes_price']),
    ('big transaction: the table is the word after the verb, not after INTO or FROM', HOST,
     '''.position(|x| x.eq_ignore_ascii_case("INTO") || x.eq_ignore_ascii_case("FROM"))''',
     '''.position(|_| true)''',
     ['cargo', '-p', 'graftyard-site', '--lib', 'a_big_transaction']),
    ('readsBy: a rows read is named by the query\'s first key, so a narrow read after `packed` reads as `packed`', MEM,
     '        Some(r) => first(r).map_or_else(|| "rows".into(), |sub| format!("rows.{sub}")),',
     '        Some(_) => first(q).unwrap_or_else(|| "rows".into()),',
     ['cargo', '-p', 'graftyard-site', '--lib', 'every_read_is_shed']),
    ('readsBy: a rows read with a selector is counted as a whole-tier dump', MEM,
     '        Some(r) => first(r).map_or_else(|| "rows".into(), |sub| format!("rows.{sub}")),',
     '        Some(r) => first(r).map_or_else(|| "rows".into(), |_| "rows".into()),',
     ['cargo', '-p', 'graftyard-site', '--lib', 'every_read_is_shed']),
    # Round 18c: a clone the client cancels releases the network's lock (TestCancelledCloneReleasesTheLock).
    # Since 6939f78f the stop is `kill`, a SIGKILL to upload-pack's process group, which a failed copy and a cancel
    # both run. The old anchor, `cmd.Process.Kill()`, matched once but in an unrelated cat-file parser.
    ('round 18c: upload-pack is not stopped when its response write fails (the lock stays held)', GO,
     '\tkill := func() { syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL) }\n',
     '\tkill := func() {}\n', ['go', 'TestCancelledCloneReleasesTheLock']),
    # The copy writes to `out` (the answer or its spool file) since composed-miss; a cancel also kills at once now.
    ('round 18c: the failed copy is ignored, so nothing stops upload-pack', GO,
     '\t} else if _, err := io.CopyBuffer(out, stdout, make([]byte, 64<<10)); err != nil {\n',
     '\t} else if _, err := io.CopyBuffer(out, stdout, make([]byte, 64<<10)); err != nil && false {\n', ['go', 'TestCancelledCloneReleasesTheLock']),
    ('round 18c: a streamed pack goes to the home whatever its probe answers', EDGE_LIB,
     '            (200, _) => return (site, tries),\n', '            (200, _) | (_, _) => return (site, tries),\n',
     ['cargo', '-p', 'graftyard-edge', '--lib', 'a_streamed_pack_goes']),
    ('round 18c: the last site is probed too, so a streamed pack can find no shard', EDGE_LIB,
     '    let (last, rest) = sites.split_last().expect("validate_sites always holds the home site");\n    for (i, site) in rest.iter()',
     '    let (last, _rest) = sites.split_last().expect("validate_sites always holds the home site");\n    for (i, site) in sites.iter()',
     ['cargo', '-p', 'graftyard-edge', '--lib', 'a_streamed_pack_goes']),
    # pool-warm: the step up is reserved and warmed ahead, waits for a cold slot, and a lapsed slot is torn down.
    ('pool-warm: no reservation on the high line (the step pays a cold rehydrate)', 'crates/git/src/shard.rs',
     '    match (high && k < FANOUT_MAX).then_some(k) {', '    match (false && high && k < FANOUT_MAX).then_some(k) {', ['cargo', '-p', 'graftyard-git', '--test', 'agent_load', 'the_step_up']),
    ('pool-warm: a step commits to a cold slot', 'crates/git/src/shard.rs',
     '    if ready(0) && ready(1) { next } else { k }', '    if ready(0) || true { next } else { k }', ['cargo', '-p', 'graftyard-git', '--test', 'agent_load', 'the_step_up']),
    ('pool-warm: a waited step restarts the high line (commits FANOUT_UP_FOR late)', 'crates/git/src/shard.rs',
     '        self.f = prior;', '        let _ = prior;', ['cargo', '-p', 'graftyard-git', '--test', 'agent_load', 'the_step_up']),
    ('pool-warm: a lapsed lease leaves its slots running', 'crates/git/src/shard.rs',
     '        self.idle.extend(lapsed);', '        drop(lapsed);', ['cargo', '-p', 'graftyard-git', '--test', 'agent_load', 'a_lapsed_lease']),
    ('pool-warm: a pool warm starts no stopped shard', SH,
     '        Caller::PoolWarm | Caller::Start => Serve::Run { touch: false },',
     '        Caller::Start => Serve::Run { touch: false },\n        Caller::PoolWarm => Serve::Skip,', ['cargo', '-p', 'graftyard-git', '--test', 'shard', 'only_a_clients_call']),
    # m30: a step waiting on cold slots keeps them (a forced k 4 trimmed its grants to k - 1 = 0 and never stepped).
    ('pool-keep: a waiting step trims its slots to the k served', 'crates/git/src/shard.rs',
     '    if next < granted { served.max(granted - 1) } else { served }', '    served',
     ['cargo', '-p', 'graftyard-git', '--test', 'agent_load', 'a_waiting_step']),
    ('pool-keep: a waiting step keeps every slot the ledgers granted, not the ones it would serve from', 'crates/git/src/shard.rs',
     '    if next < granted { served.max(granted - 1) } else { served }', '    if next < granted { served.max(want) } else { served }',
     ['cargo', '-p', 'graftyard-git', '--test', 'agent_load', 'a_waiting_step']),
    ('pool-keep: a granted step keeps k, one too many', 'crates/git/src/shard.rs',
     '    let served = if reserve { want.min(next) } else { next.saturating_sub(1) };', '    let served = if reserve { want.min(next) } else { next };',
     ['cargo', '-p', 'graftyard-git', '--test', 'agent_load', 'a_waiting_step']),
    # promote-stream: a first entry over PROMOTE_MAX promotes alone, and the site streams the shard's pack.
    # plan() reads the log in a labelled loop and knows the first entry by `first`.
    ('promote-stream: a first entry over PROMOTE_MAX is cut to nothing', LA,
     '            if !first && packed + b > promote_max {',
     '            if packed + b > promote_max {', ['cargo', '-p', 'graftyard-git', '--test', 'log_archive', 'promotions_fit']),
    ('promote-stream: an entry over PROMOTE_MAX takes the next entries with it', LA,
     '            if !first && packed + b > promote_max {',
     '            if !first && packed > promote_max {', ['cargo', '-p', 'graftyard-git', '--test', 'log_archive', 'promotions_fit']),
    ('promote-stream: the promotion holds a third part', 'crates/git/src/store.rs',
     '                        None if made < 2 => {', '                        None if made < 3 => {',
     ['cargo', '-p', 'graftyard-git', '--lib', 'a_promotion_streams']),
    ('promote-stream: the .idx is not stored beside the promoted pack', 'crates/git/src/store.rs',
     '            self.put_idx(network, id, meta, &b, range, *sites).await;', '            let _ = (network, id, meta, b, range, sites);',
     ['cargo', '-p', 'graftyard-git', '--lib', 'a_promotion_streams']),
    ('promote-stream: the promoted .idx drops its range', 'crates/git/src/store.rs',
     '            self.put_idx(network, id, meta, &b, range, *sites).await;', '            self.put_idx(network, id, meta, &b, range.and(None), *sites).await;',
     ['cargo', '-p', 'graftyard-git', '--lib', 'a_promotion_streams']),
    ('promote-stream: the .idx is asked for under another sum', 'crates/git/src/store.rs',
     '            && let Some(b) = idx(*checksum).await', '            && let Some(b) = idx(Digest([checksum.0[0]; 32])).await',
     ['cargo', '-p', 'graftyard-git', '--lib', 'a_promotion_streams']),
    ('promote-stream: a refused promotion still asks for its .idx', 'crates/git/src/store.rs',
     '            Ok((d, _)) => d.into_pack(),\n            Err(e) => return Ok(Err(e)),',
     '            Ok((d, _)) => d.into_pack(),\n            Err(e) => {\n                let _ = idx(Digest([0; 32])).await;\n                return Ok(Err(e));\n            }',
     ['cargo', '-p', 'graftyard-git', '--lib', 'a_promotion_streams']),

    # connected-after: a rehydrate that skipped covered packs starts its repack after the backstop, not before.
    # put_refs_page also names the HEAD ref (G26).
    ("connected-after: the last ref page starts the repack though the check follows (main's order)", CU,
     'io.put_refs_page(&refs, r.pages == 0, done, state.skipped.is_empty(), head_ref.as_deref()).await?;',
     'io.put_refs_page(&refs, r.pages == 0, done, true, head_ref.as_deref()).await?;', ['cargo', '-p', 'graftyard-git', '--test', 'catchup', 'a_rehydrate_of_promoted_packs_loads']),
    ("connected-after: no rehydrate's last ref page starts the repack", CU,
     'io.put_refs_page(&refs, r.pages == 0, done, state.skipped.is_empty(), head_ref.as_deref()).await?;',
     'io.put_refs_page(&refs, r.pages == 0, done, false, head_ref.as_deref()).await?;', ['cargo', '-p', 'graftyard-git', '--test', 'catchup', 'a_rehydrate_reads_the_log_only']),
    ('connected-after: the backstop never starts the repack', 'crates/git/src/catchup.rs',
     '        && let Err(why) = io.connected(true).await?', '        && let Err(why) = io.connected(false).await?',
     ['cargo', '-p', 'graftyard-git', '--test', 'catchup', 'a_rehydrate_of_promoted_packs_loads']),
    ('connected-after: the container ignores repack=0 on the last ref page', 'containers/git/server/main.go',
     '\t\tif q.Get("repack") != "0" {', '\t\tif true {', ['go', 'TestConnectedRunsBeforeTheRehydratesRepack']),
    ('connected-after: /connected starts the repack before its walk', 'containers/git/server/main.go',
     '\tout, err := connectedWalk(net, rp, r.URL.Query().Get("objects"))\n\tif r.URL.Query().Get("repack") == "1" {\n\t\tstarted := startRepack(net, rp)',
     '\tstarted := r.URL.Query().Get("repack") == "1" && startRepack(net, rp)\n\tout, err := connectedWalk(net, rp, r.URL.Query().Get("objects"))\n\tif r.URL.Query().Get("repack") == "1" {',
     ['go', 'TestConnectedRunsBeforeTheRehydratesRepack']),
    ('connected-after: a failed walk starts no repack', 'containers/git/server/main.go',
     '\tif r.URL.Query().Get("repack") == "1" {\n\t\tstarted := startRepack(net, rp)',
     '\tif r.URL.Query().Get("repack") == "1" && out != nil && out["connected"] == true {\n\t\tstarted := startRepack(net, rp)',
     ['go', 'TestConnectedRunsBeforeTheRehydratesRepack']),

    # promoted packs: exclude bases --fix-thin appended, and name delta candidates by their tree entries.
    ('record: every indexed object is kept, including appended thin bases', GO,
     '\t\tif sent <= 0 || e.offset < sent-20 {', '\t\tif sent <= 0 || e.offset >= 0 {',
     ['go', 'TestRecordKeeps|TestPromoted']),
    ('record: the sent-length filter is off by one', GO,
     '\t\tif sent <= 0 || e.offset < sent-20 {', '\t\tif sent <= 0 || e.offset <= sent-20 {',
     ['go', 'TestRecordKeeps|TestPromoted']),
    ('record: the filter includes the sent pack trailer offset', GO,
     '\t\tif sent <= 0 || e.offset < sent-20 {', '\t\tif sent <= 0 || e.offset < sent {',
     ['go', 'TestRecordKeeps|TestPromoted']),
    ('promote: pack-objects receives bare object ids, without names', GO,
     '\t\tif name := names[oid]; name != "" {', '\t\tif name := names[oid]; false && name != "" {',
     ['go', 'TestRecordKeeps|TestPromoted']),
    ('objectNames: each child name is mapped to its parent tree', GO,
     '\t\t\t\tnames[oid] = strings.ReplaceAll(string(raw[sp+1:nul]), "\\n", " ")',
     '\t\t\t\tnames[f[0]] = strings.ReplaceAll(string(raw[sp+1:nul]), "\\n", " ")',
     ['go', 'TestRecordKeeps|TestPromoted']),
    ('adopt: the validated sent length is always zero', GO,
     '\treturn name, sent, true, nil', '\treturn name, 0, true, nil',
     ['go', 'TestRecordKeeps|TestPromoted']),
    ('fetch-ahead: the byte cap again holds back every large pack past the first', 'crates/git/src/catchup.rs',
     '&& (self.started - self.taken < FETCH_AT_ONCE || self.held', '&& (self.started == self.taken || self.held',
     ['cargo', 'test', 'graftyard-git', '--test', 'catchup', 'large_promoted']),
]

# A target dir of its own, so a mutant's build never touches the workspace's; under the git-ignored target/.
ENV = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0',
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, 'target', 'plants')))


def run(t):
    if t[0] == 'node':
        r = subprocess.run(t, cwd=ROOT, capture_output=True, text=True, timeout=120)
        out = r.stdout + r.stderr
        return r.returncode, out, 'SyntaxError' in out
    if t[0] == 'go':
        r = subprocess.run(['go', 'test', '-count=1', '-timeout', '120s', '-run', t[1], '.'], cwd=os.path.join(ROOT, 'containers/git/server'),
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
