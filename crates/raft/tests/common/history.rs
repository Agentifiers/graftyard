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

/// `out: None` is an operation that got no reply: a write may have taken effect any time after its call.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub client: usize,
    pub call: u64,
    pub ret: u64,
    pub op: HOp,
    pub out: Option<HOut>,
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
        out.push_str(&format!(
            "{{\"client\":{},\"call\":{},\"ret\":{},\"op\":{op},\"ok\":{ok},\"out\":{res}}}\n",
            r.client + client_base,
            r.call,
            r.ret
        ));
    }
    out
}
