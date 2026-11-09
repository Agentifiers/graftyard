//! Fragment spreads expand at most once per object, as the GraphQL spec's CollectFields `visitedFragments` says: a
//! self-referencing fragment ends instead of overflowing the stack, and a chain of doubled spreads runs in linear
//! time instead of exponential.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use graftyard_api::exec::{Args, Obj, Out, Request};
use serde_json::{Value, json};

/// How long a document of a few hundred bytes may take; the unfixed executor needs hours for the doubling chain.
const PROMPT: Duration = Duration::from_secs(10);

struct Root;

impl Obj for Root {
    fn type_name(&self) -> &'static str {
        "Query"
    }
    fn field<'s>(&'s self, _: &str, _: &Args) -> Option<Out<'s>> {
        None
    }
}

/// Runs `query` on another thread and fails the test if it does not answer within `PROMPT`.
fn run_promptly(query: String) -> Value {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let reply = Request::parse(&query, None, None).expect("the document parses").run(&Root);
        let _ = tx.send(reply);
    });
    rx.recv_timeout(PROMPT).unwrap_or_else(|e| panic!("the document did not answer within {PROMPT:?}: {e}"))
}

#[test]
fn a_self_referencing_fragment_ends() {
    let reply = run_promptly("query { ...A } fragment A on Query { __typename ...A }".into());
    assert_eq!(reply, json!({ "data": { "__typename": "Query" } }));
}

#[test]
fn a_doubling_fragment_chain_expands_each_fragment_once() {
    const LEVELS: usize = 30;
    let mut query = String::from("query { ...F0 }");
    for i in 0..LEVELS {
        query.push_str(&format!(" fragment F{i} on Query {{ ...F{n} ...F{n} }}", n = i + 1));
    }
    query.push_str(&format!(" fragment F{LEVELS} on Query {{ __typename t: __typename }}"));
    let reply = run_promptly(query);
    assert_eq!(reply, json!({ "data": { "__typename": "Query", "t": "Query" } }));
}

#[test]
fn a_repeated_spread_answers_as_one_spread_does() {
    let once = run_promptly("query { ...A } fragment A on Query { __typename }".into());
    let twice = run_promptly("query { ...A ...A t: __typename ...A } fragment A on Query { __typename }".into());
    assert_eq!(once, json!({ "data": { "__typename": "Query" } }));
    assert_eq!(twice, json!({ "data": { "__typename": "Query", "t": "Query" } }));
}
