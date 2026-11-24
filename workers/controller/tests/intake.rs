//! The controller's webhook intake (`design/AGENT-LAYER.md`, journey A3 step 2): the signature before anything else,
//! a repeated delivery id dropped, and a bounded `Seen`.

use graftyard_controller::{Intake, SEEN_MAX, Seen, intake, verify};
use graftyard_identity::sealed::hmac_sha256;

const SECRET: &[u8] = b"webhook secret";

fn sign(body: &[u8]) -> String {
    let hex: String = hmac_sha256(SECRET, body).iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256={hex}")
}

fn headers(pairs: Vec<(&'static str, String)>) -> impl Fn(&str) -> Option<String> {
    move |name| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone())
}

fn delivery(body: &[u8], id: &str) -> impl Fn(&str) -> Option<String> {
    headers(vec![
        ("x-hub-signature-256", sign(body)),
        ("x-github-event", "issues".into()),
        ("x-github-delivery", id.into()),
    ])
}

const BODY: &[u8] = br#"{"action":"assigned","issue":{"number":42}}"#;

#[test]
fn verify_accepts_only_the_exact_sha256_mac() {
    let good = sign(BODY);
    assert!(verify(SECRET, BODY, Some(&good)));
    assert!(!verify(SECRET, BODY, None));
    assert!(!verify(b"other secret", BODY, Some(&good)));
    assert!(!verify(SECRET, b"{}", Some(&good)));
    // A compare that zips without checking length would accept any prefix, the empty one included.
    assert!(!verify(SECRET, BODY, Some(&good[..good.len() - 1])));
    assert!(!verify(SECRET, BODY, Some(&good[..20])));
    assert!(!verify(SECRET, BODY, Some("sha256=")));
    assert!(!verify(SECRET, BODY, Some(&format!("{good}00"))));
    // Same hex under another scheme.
    assert!(!verify(SECRET, BODY, Some(&good.replacen("sha256=", "sha1=", 1))));
    let mut flipped = good.into_bytes();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == b'0' { b'1' } else { b'0' };
    assert!(!verify(SECRET, BODY, Some(std::str::from_utf8(&flipped).unwrap())));
}

#[test]
fn signature_is_checked_before_the_body_is_parsed() {
    let mut seen = Seen::default();
    let junk = b"not json at all";
    let h = headers(vec![
        ("x-hub-signature-256", "sha256=00".into()),
        ("x-github-event", "issues".into()),
        ("x-github-delivery", "d1".into()),
    ]);
    assert_eq!(intake(SECRET, &h, junk, &mut seen), Intake::Refused(401, "bad signature"));
    // Unsigned and missing the GitHub headers: still 401, not 400.
    assert_eq!(intake(SECRET, &headers(vec![]), junk, &mut seen), Intake::Refused(401, "bad signature"));
    // Signed but not JSON: now the parser speaks.
    assert!(matches!(intake(SECRET, &delivery(junk, "d1"), junk, &mut seen), Intake::Refused(400, _)));
}

#[test]
fn signed_but_malformed_is_400() {
    let mut seen = Seen::default();
    for pairs in [
        vec![("x-hub-signature-256", sign(BODY)), ("x-github-delivery", "d".into())],
        vec![("x-hub-signature-256", sign(BODY)), ("x-github-event", "issues".into())],
        vec![("x-hub-signature-256", sign(BODY)), ("x-github-event", "".into()), ("x-github-delivery", "d".into())],
    ] {
        assert!(matches!(intake(SECRET, &headers(pairs), BODY, &mut seen), Intake::Refused(400, _)));
    }
    for body in [&b"[1,2]"[..], b"\"s\"", b"null", b"{"] {
        assert!(matches!(intake(SECRET, &delivery(body, "d"), body, &mut seen), Intake::Refused(400, _)));
    }
}

#[test]
fn accepted_carries_event_action_and_delivery() {
    let mut seen = Seen::default();
    assert_eq!(
        intake(SECRET, &delivery(BODY, "d1"), BODY, &mut seen),
        Intake::Accepted { event: "issues".into(), action: Some("assigned".into()), delivery: "d1".into() }
    );
    let ping = br#"{"zen":"Keep it logically awesome."}"#;
    assert_eq!(
        intake(SECRET, &delivery(ping, "d2"), ping, &mut seen),
        Intake::Accepted { event: "issues".into(), action: None, delivery: "d2".into() }
    );
}

#[test]
fn a_repeated_delivery_id_is_dropped() {
    let mut seen = Seen::default();
    assert!(matches!(intake(SECRET, &delivery(BODY, "d1"), BODY, &mut seen), Intake::Accepted { .. }));
    assert_eq!(intake(SECRET, &delivery(BODY, "d1"), BODY, &mut seen), Intake::Duplicate);
    // A redelivery carries the same GUID with a possibly different body: still a duplicate.
    let other = br#"{"action":"unassigned"}"#;
    assert_eq!(intake(SECRET, &delivery(other, "d1"), other, &mut seen), Intake::Duplicate);
    assert!(matches!(intake(SECRET, &delivery(BODY, "d2"), BODY, &mut seen), Intake::Accepted { .. }));
}

#[test]
fn a_refused_delivery_is_not_remembered() {
    let mut seen = Seen::default();
    let bad = headers(vec![
        ("x-hub-signature-256", "sha256=00".into()),
        ("x-github-event", "issues".into()),
        ("x-github-delivery", "d1".into()),
    ]);
    assert!(matches!(intake(SECRET, &bad, BODY, &mut seen), Intake::Refused(401, _)));
    assert!(matches!(intake(SECRET, &delivery(BODY, "d1"), BODY, &mut seen), Intake::Accepted { .. }));
}

#[test]
fn seen_is_bounded_and_evicts_the_oldest() {
    let mut seen = Seen::default();
    for i in 0..=SEEN_MAX {
        let id = format!("d{i}");
        assert!(matches!(intake(SECRET, &delivery(BODY, &id), BODY, &mut seen), Intake::Accepted { .. }));
    }
    // SEEN_MAX + 1 ids pushed the first out; the newest SEEN_MAX are still dropped.
    assert_eq!(intake(SECRET, &delivery(BODY, &format!("d{SEEN_MAX}")), BODY, &mut seen), Intake::Duplicate);
    assert_eq!(intake(SECRET, &delivery(BODY, "d2"), BODY, &mut seen), Intake::Duplicate);
    assert!(matches!(intake(SECRET, &delivery(BODY, "d0"), BODY, &mut seen), Intake::Accepted { .. }));
}
