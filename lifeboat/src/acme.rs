//! The lifeboat's certificate through ACME DNS-01 (`design/DESIGN.md`, "Stack", DNS: "the CNAME that delegates its
//! `_acme-challenge` name to a validation zone in a separate account"; "Deployment modes"; `decisions/DECISIONS.md`,
//! "`lifeboat-acme`").
//!
//! The production zone holds `_acme-challenge.<host>` as a CNAME to `<label>.<validation zone>`, set once by the
//! customer. The lifeboat holds only a DNS token for the validation zone, so it can answer challenges for its own name
//! and cannot repoint the production zone. This module is the schedule and the record, over a `Dns` trait; the ACME
//! client is the bundle's.
//!
//! - **When.** Renew at `RENEW_BEFORE` before expiry, then retry every `RETRY_EVERY` while it fails; past
//!   `ALARM_BEFORE` before expiry, alarm.
//! - **The record.** The TXT value is the challenge's key authorization digest; it is put under the delegated name,
//!   waited for at the validation zone's own servers (`propagated`), and deleted once the order ends, pass or fail.
//!
//! Real ACME needs the validation-zone account, which only the operator can create; until then the bundle runs on a
//! local CA (`decisions/OPEN-QUESTIONS.md`).

/// Renew this long before expiry [guess: Let's Encrypt's 90-day certificates, renewed with 30 days left].
pub const RENEW_BEFORE_MS: u64 = 30 * 86_400_000;
/// A failed renewal is tried again after this [guess].
pub const RETRY_EVERY_MS: u64 = 6 * 3_600_000;
/// Still not renewed this close to expiry: alarm [guess].
pub const ALARM_BEFORE_MS: u64 = 14 * 86_400_000;

/// What the schedule says at a moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renew {
    /// Nothing to do until this time.
    Wait(u64),
    /// Start an order now.
    Now { alarm: bool },
}

/// The certificate's state as the lifeboat keeps it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cert {
    /// `notAfter`, in ms; 0 with no certificate.
    pub expires: u64,
    /// The last failed order's start, if the last order failed.
    pub failed_at: Option<u64>,
}

pub fn renew(c: &Cert, now: u64) -> Renew {
    let due = c.expires.saturating_sub(RENEW_BEFORE_MS);
    if now < due {
        return Renew::Wait(due);
    }
    let alarm = now + ALARM_BEFORE_MS >= c.expires;
    match c.failed_at {
        Some(f) if now < f + RETRY_EVERY_MS => Renew::Wait(f + RETRY_EVERY_MS),
        _ => Renew::Now { alarm },
    }
}

/// The name the challenge's TXT record goes under: the CNAME target in the validation zone. `label` is the one the
/// customer's CNAME names, which `graftyard lifeboat init` prints.
pub fn challenge_name(label: &str, validation_zone: &str) -> String {
    format!("{label}.{}", validation_zone.trim_end_matches('.'))
}

/// The DNS calls one order makes, in the validation zone only.
pub trait Dns {
    fn put_txt(&mut self, name: &str, value: &str) -> Result<String, String>;
    fn delete(&mut self, id: &str) -> Result<(), String>;
    /// Whether the zone's own servers answer `value` for `name`.
    fn propagated(&self, name: &str, value: &str) -> bool;
}

/// One order's DNS part: put the record, wait for it (up to `polls` checks, `poll` called between them), run `answer`
/// (the ACME client's challenge response and finalize), and delete the record whatever happened.
pub fn order<D: Dns, T>(
    dns: &mut D,
    name: &str,
    value: &str,
    polls: usize,
    mut poll: impl FnMut(),
    answer: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let id = dns.put_txt(name, value)?;
    let mut seen = false;
    for _ in 0..polls {
        if dns.propagated(name, value) {
            seen = true;
            break;
        }
        poll();
    }
    let r = if seen { answer() } else { Err(format!("{name} did not propagate")) };
    let deleted = dns.delete(&id);
    let r = r?;
    deleted?;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400_000;

    #[test]
    fn renewal_starts_thirty_days_out_retries_and_alarms_at_fourteen() {
        let exp = 100 * DAY;
        let c = Cert { expires: exp, failed_at: None };
        assert_eq!(renew(&c, 0), Renew::Wait(70 * DAY));
        assert_eq!(renew(&c, 70 * DAY - 1), Renew::Wait(70 * DAY));
        assert_eq!(renew(&c, 70 * DAY), Renew::Now { alarm: false });
        let failed = Cert { failed_at: Some(70 * DAY), ..c };
        assert_eq!(renew(&failed, 70 * DAY + 1), Renew::Wait(70 * DAY + RETRY_EVERY_MS));
        assert_eq!(renew(&failed, 70 * DAY + RETRY_EVERY_MS), Renew::Now { alarm: false });
        assert_eq!(renew(&Cert { failed_at: Some(85 * DAY), ..c }, 86 * DAY), Renew::Now { alarm: true });
        assert_eq!(renew(&Cert::default(), 5), Renew::Now { alarm: true }, "no certificate: order now");
    }

    #[derive(Default)]
    struct Stub {
        records: Vec<(String, String, String)>,
        visible_after: usize,
        checks: std::cell::Cell<usize>,
        next: u32,
    }

    impl Dns for Stub {
        fn put_txt(&mut self, name: &str, value: &str) -> Result<String, String> {
            self.next += 1;
            let id = format!("r{}", self.next);
            self.records.push((id.clone(), name.into(), value.into()));
            Ok(id)
        }
        fn delete(&mut self, id: &str) -> Result<(), String> {
            self.records.retain(|r| r.0 != id);
            Ok(())
        }
        fn propagated(&self, name: &str, value: &str) -> bool {
            self.checks.set(self.checks.get() + 1);
            self.checks.get() > self.visible_after && self.records.iter().any(|r| r.1 == name && r.2 == value)
        }
    }

    #[test]
    fn an_order_answers_once_the_record_propagates_and_always_deletes_it() {
        let name = challenge_name("lb-acme", "validate.example.");
        assert_eq!(name, "lb-acme.validate.example");
        let mut dns = Stub { visible_after: 2, ..Default::default() };
        let mut polled = 0;
        let r = order(&mut dns, &name, "tok", 5, || polled += 1, || Ok("cert"));
        assert_eq!((r, polled), (Ok("cert"), 2));
        assert!(dns.records.is_empty());
        // The challenge failed: the record still goes.
        let r: Result<(), _> = order(&mut dns, &name, "tok", 5, || {}, || Err("invalid".into()));
        assert_eq!(r, Err("invalid".into()));
        assert!(dns.records.is_empty());
        // Never propagated: the ACME client is never asked.
        let mut dns = Stub { visible_after: 99, ..Default::default() };
        let r: Result<(), _> = order(&mut dns, &name, "tok", 3, || {}, || panic!("answered before propagation"));
        assert!(r.is_err() && dns.records.is_empty());
    }
}
