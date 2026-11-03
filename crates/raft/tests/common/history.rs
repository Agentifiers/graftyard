//! Client histories and a linearizability checker for the register (Wing and Gong's search with Lowe's cache, the
//! algorithm Porcupine runs), partitioned by key. `jsonl` writes the rows in the format
//! `tests/m0/porcupine/main.go` reads, so the Go checker can check the same histories.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Debug, PartialEq)]
pub enum HOp {
    Get { key: String },
    Put { key: String, value: String },
    Cas { key: String, expect: Option<String>, value: String },
}

impl HOp {
    pub fn key(&self) -> &str {
        match self {
            HOp::Get { key } | HOp::Put { key, .. } | HOp::Cas { key, .. } => key,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum HOut {
    Value(Option<String>),
    Ok(bool),
}

/// `out: None` is an operation that got no reply: a write may have taken effect any time after its call. `ack` is the
/// answering member's evidence for an acknowledged write (`Ack`).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub client: usize,
    pub call: u64,
    pub ret: u64,
    pub op: HOp,
    pub out: Option<HOut>,
    pub ack: Option<Ack>,
}

/// An acknowledged write's evidence, all on the answering member's clock, in the shape the lin install's members
/// answer (`workers/site/src/host.rs`, `outcome_json`) and `tests/m1/lin/ackcheck.ts` checks: the member, the write's
/// entry, the commit its `QuorumProof` is for, when that entry was built and committed, when the answer was built,
/// when the host's storage confirmed the commit durable on the answering site, and each counted site's durable index
/// and when the leader learned it.
#[derive(Clone, Debug, PartialEq)]
pub struct Ack {
    pub site: u8,
    pub index: u64,
    pub commit_index: u64,
    pub entry_at: u64,
    pub commit_at: u64,
    pub sent_at: u64,
    /// The host's own record, not the crate's: when its storage first confirmed the log durable through
    /// `commit_index`.
    pub self_synced_at: Option<u64>,
    pub acks: Vec<(u8, u64, u64)>,
}

/// The answering member's evidence for one acknowledged write, checked on its own clock alone: the write's entry is
/// at or below the proof's commit index; the reply was built at or after the commit; at least `quorum` distinct
/// sites, the answering one among them, are counted; each counted site's durable index reaches the commit index; and
/// each ack was learned no earlier than the entry was built and no later than the commit; and the host's own storage
/// confirm of the commit came at or before it. `tests/m1/lin/ackcheck.ts`
/// applies the same rules to the lin install's history.
pub fn ack_check(a: &Ack, quorum: usize) -> Result<(), String> {
    if a.index > a.commit_index {
        return Err(format!("entry {} above the proof's commit {}", a.index, a.commit_index));
    }
    if a.sent_at < a.commit_at {
        return Err(format!("sent at {} before its commit at {}", a.sent_at, a.commit_at));
    }
    let mut sites: Vec<u8> = a.acks.iter().map(|x| x.0).collect();
    sites.sort_unstable();
    sites.dedup();
    if sites.len() < quorum || sites.len() != a.acks.len() {
        return Err(format!("{} distinct sites counted, {} needed", sites.len(), quorum));
    }
    if !sites.contains(&a.site) {
        return Err(format!("the answering site {} is not counted", a.site));
    }
    match a.self_synced_at {
        None => return Err("no host storage confirm covers the commit".into()),
        Some(t) if t > a.commit_at => {
            return Err(format!("the host confirmed the commit durable at {t}, after the commit at {}", a.commit_at));
        }
        Some(_) => {}
    }
    for (site, durable, at) in &a.acks {
        if *durable < a.commit_index {
            return Err(format!("site {site} durable through {durable}, below the commit {}", a.commit_index));
        }
        if *at > a.commit_at {
            return Err(format!("site {site}'s ack learned at {at}, after the commit at {}", a.commit_at));
        }
        if *at < a.entry_at {
            return Err(format!("site {site}'s ack learned at {at}, before the entry was built at {}", a.entry_at));
        }
    }
    Ok(())
}

fn step(state: &Option<String>, op: &HOp, out: &Option<HOut>) -> Option<Option<String>> {
    match (op, out) {
        (HOp::Get { .. }, Some(HOut::Value(v))) => (v == state).then(|| state.clone()),
        (HOp::Get { .. }, _) => None,
        (HOp::Put { value, .. }, _) => Some(Some(value.clone())),
        (HOp::Cas { expect, value, .. }, out) => {
            let matches = expect == state;
            match out {
                None => Some(if matches { Some(value.clone()) } else { state.clone() }),
                Some(HOut::Ok(true)) => matches.then(|| Some(value.clone())),
                Some(HOut::Ok(false)) => (!matches).then(|| state.clone()),
                Some(HOut::Value(_)) => None,
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Illegal { key: String },
}

/// Checks every key's history. Reads with no reply carry no information and are dropped.
pub fn check(rows: &[Row]) -> Verdict {
    let mut by_key: BTreeMap<&str, Vec<&Row>> = BTreeMap::new();
    for r in rows {
        if matches!(r.op, HOp::Get { .. }) && r.out.is_none() {
            continue;
        }
        by_key.entry(r.op.key()).or_default().push(r);
    }
    for (k, ops) in by_key {
        if !check_one(&ops) {
            return Verdict::Illegal { key: k.to_owned() };
        }
    }
    Verdict::Ok
}

const NONE: usize = usize::MAX;

fn check_one(ops: &[&Row]) -> bool {
    let n = ops.len();
    // Events: (time, is_return, op). A call sorts before a return at the same time, as Porcupine does.
    let mut events: Vec<(u64, bool, usize)> = Vec::with_capacity(2 * n);
    for (i, r) in ops.iter().enumerate() {
        let ret = if r.out.is_none() { u64::MAX } else { r.ret };
        events.push((r.call, false, i));
        events.push((ret, true, i));
    }
    events.sort();
    // Node 0 is the head; node j + 1 is events[j].
    let len = events.len() + 1;
    let mut next = vec![NONE; len];
    let mut prev = vec![NONE; len];
    for j in 0..len {
        next[j] = if j + 1 < len { j + 1 } else { NONE };
        prev[j] = if j == 0 { NONE } else { j - 1 };
    }
    let mut call_node = vec![0; n];
    let mut ret_node = vec![0; n];
    for (j, (_, is_ret, i)) in events.iter().enumerate() {
        if *is_ret { ret_node[*i] = j + 1 } else { call_node[*i] = j + 1 }
    }
    let unlink = |next: &mut Vec<usize>, prev: &mut Vec<usize>, x: usize| {
        let (p, nx) = (prev[x], next[x]);
        next[p] = nx;
        if nx != NONE {
            prev[nx] = p;
        }
    };
    let relink = |next: &mut Vec<usize>, prev: &mut Vec<usize>, x: usize| {
        let (p, nx) = (prev[x], next[x]);
        next[p] = x;
        if nx != NONE {
            prev[nx] = x;
        }
    };
    let words = n.div_ceil(64);
    let mut lin = vec![0u64; words];
    let mut cache: HashMap<Vec<u64>, Vec<Option<String>>> = HashMap::new();
    let mut stack: Vec<(usize, Option<String>)> = Vec::new();
    let mut state: Option<String> = None;
    let mut entry = next[0];
    while next[0] != NONE {
        if entry == NONE {
            return false;
        }
        let (_, is_ret, i) = events[entry - 1];
        if !is_ret {
            if let Some(s2) = step(&state, &ops[i].op, &ops[i].out) {
                lin[i / 64] |= 1 << (i % 64);
                let seen = cache.entry(lin.clone()).or_default();
                if !seen.contains(&s2) {
                    seen.push(s2.clone());
                    stack.push((i, state));
                    state = s2;
                    unlink(&mut next, &mut prev, call_node[i]);
                    unlink(&mut next, &mut prev, ret_node[i]);
                    entry = next[0];
                    continue;
                }
                lin[i / 64] &= !(1 << (i % 64));
            }
            entry = next[entry];
        } else {
            let Some((i, s)) = stack.pop() else { return false };
            state = s;
            lin[i / 64] &= !(1 << (i % 64));
            relink(&mut next, &mut prev, ret_node[i]);
            relink(&mut next, &mut prev, call_node[i]);
            entry = next[call_node[i]];
        }
    }
    true
}

fn js(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn jopt(v: &Option<String>) -> String {
    v.as_deref().map_or("null".into(), js)
}

/// One JSON line per row, as `tests/m0/porcupine/main.go` reads them. `client` is offset so histories of several
/// seeds can go into one file with distinct clients and keys.
pub fn jsonl(rows: &[Row], prefix: &str, client_base: usize) -> String {
    let mut out = String::new();
    for r in rows {
        let key = format!("{prefix}{}", r.op.key());
        let op = match &r.op {
            HOp::Get { .. } => format!("{{\"kind\":\"get\",\"key\":{}}}", js(&key)),
            HOp::Put { value, .. } => format!("{{\"kind\":\"put\",\"key\":{},\"value\":{}}}", js(&key), js(value)),
            HOp::Cas { expect, value, .. } => {
                format!("{{\"kind\":\"cas\",\"key\":{},\"expect\":{},\"value\":{}}}", js(&key), jopt(expect), js(value))
            }
        };
        let (ok, res) = match &r.out {
            None => (false, "null".to_owned()),
            Some(HOut::Value(v)) => (true, format!("{{\"value\":{}}}", jopt(v))),
            Some(HOut::Ok(b)) => (true, format!("{{\"ok\":{b}}}")),
        };
        let ack = r.ack.as_ref().map_or(String::new(), |a| {
            let acks: Vec<String> =
                a.acks.iter().map(|(s, d, t)| format!("{{\"site\":{s},\"durable\":{d},\"at\":{t}}}")).collect();
            let synced = a.self_synced_at.map_or("null".to_owned(), |t| t.to_string());
            format!(
                ",\"ack\":{{\"site\":{},\"index\":{},\"commitIndex\":{},\"entryAt\":{},\"commitAt\":{},\"sentAt\":{},\
                 \"selfSyncedAt\":{synced},\"acks\":[{}]}}",
                a.site,
                a.index,
                a.commit_index,
                a.entry_at,
                a.commit_at,
                a.sent_at,
                acks.join(",")
            )
        });
        out.push_str(&format!(
            "{{\"client\":{},\"call\":{},\"ret\":{},\"op\":{op},\"ok\":{ok},\"out\":{res}{ack}}}\n",
            r.client + client_base,
            r.call,
            r.ret
        ));
    }
    out
}
