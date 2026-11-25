//! `graftyard oidc`: the install's Actions OIDC issuer as relying parties reach it (`design/SHARED.md`, "Actions",
//! OIDC; `decisions/OPEN-QUESTIONS.md`, F130). Cloudflare's Browser Integrity Check, on by default in every zone,
//! answers a stock Python or Perl client with 403 "error code: 1010" before the edge runs, so a relying party on one
//! cannot read the issuer's discovery document or JWKS, which GHES never refuses.
//!
//! - `bic-rule` puts one configuration rule in the zone's `http_config_settings` entrypoint that turns Browser
//!   Integrity Check off for `/_services/token/*` only, and reads it back. Idempotent: a rule already as written is
//!   left alone, one with the same `ref` and another body is rewritten, and every other rule in the entrypoint is kept.
//! - `check` fetches the JWKS as `Python-urllib` and fails on anything but 200.
//!
//! Both are install setup the operator runs; no request path makes these calls.

use serde_json::{Value, json};

use crate::cf::Cf;
use crate::config::Res;

/// The rule's stable id in the entrypoint, so a second run finds it.
pub const RULE_REF: &str = "graftyard-oidc-bic";
/// The issuer's path (`graftyard_actions::oidc::ISSUER_PATH`); its two documents are public and static, so Browser
/// Integrity Check protects nothing there.
const ISSUER_PATH: &str = "/_services/token";
const PHASE: &str = "http_config_settings";
/// The agent a stock Python relying party sends (`urllib.request`'s default), which BIC refuses [measured, F130].
pub const CHECK_AGENT: &str = "Python-urllib/3.12";

/// What `bic_rule` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Done {
    /// No entrypoint: created with the rule.
    Created,
    /// The entrypoint had no such rule: added beside the others.
    Added,
    Kept,
    Rewritten,
}

/// The rule for `host`: BIC off on the issuer's path of the install's own host only.
pub fn rule(host: &str) -> Value {
    json!({
        "ref": RULE_REF,
        "description": "Graftyard: the Actions OIDC issuer's public documents for every relying party (F130)",
        "expression": format!(
            "(http.host eq \"{host}\" and starts_with(http.request.uri.path, \"{ISSUER_PATH}/\"))"
        ),
        "action": "set_config",
        "action_parameters": { "bic": false },
        "enabled": true,
    })
}

/// Whether `r` is the rule as written, ignoring the fields Cloudflare adds (`id`, `version`, `last_updated`).
fn same(r: &Value, want: &Value) -> bool {
    ["ref", "expression", "action", "action_parameters", "enabled"].iter().all(|k| r[*k] == want[*k])
}

/// The fields of a rule a PUT sends back: what Cloudflare reads, without what it writes itself.
fn writable(r: &Value) -> Value {
    let mut r = r.clone();
    if let Some(o) = r.as_object_mut() {
        for k in ["version", "last_updated", "categories"] {
            o.remove(k);
        }
    }
    r
}

/// Why a `workers.dev` host can never serve the issuer to Python's default agent: Cloudflare's own zone runs Browser
/// Integrity Check on every `workers.dev` request, and "You cannot turn off this check for a `workers.dev` hostname"
/// [doc: https://developers.cloudflare.com/workers/configuration/routing/workers-dev/].
pub const WORKERS_DEV: &str = "the host is on workers.dev, whose Browser Integrity Check cannot be turned off \
     (https://developers.cloudflare.com/workers/configuration/routing/workers-dev/), so a relying party on Python's \
     default agent is refused 1010 before the edge runs; serve the edge on a custom host in the install's zone";

fn on_workers_dev(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    h == "workers.dev" || h.ends_with(".workers.dev")
}

pub fn bic_rule(cf: &Cf, zone: &str, host: &str) -> Res<Done> {
    if on_workers_dev(host) {
        return Err(format!("refused for {host}: {WORKERS_DEV}"));
    }
    let want = rule(host);
    let done = match cf.phase_entrypoint(zone, PHASE)? {
        None => {
            cf.create_zone_ruleset(
                zone,
                &json!({ "name": "default", "kind": "zone", "phase": PHASE, "rules": [want] }),
            )?;
            Done::Created
        }
        Some(rs) => {
            let rules = rs["rules"].as_array().cloned().unwrap_or_default();
            let mine = rules.iter().position(|r| r["ref"].as_str() == Some(RULE_REF));
            match mine {
                Some(i) if same(&rules[i], &want) => return read_back(cf, zone, &want).map(|_| Done::Kept),
                _ => {
                    let mut out: Vec<Value> = rules.iter().map(writable).collect();
                    let done = match mine {
                        Some(i) => {
                            // Keep the rule's id, so the rule is rewritten in place, not replaced.
                            let mut w = want.clone();
                            w["id"] = rules[i]["id"].clone();
                            out[i] = w;
                            Done::Rewritten
                        }
                        None => {
                            out.push(want.clone());
                            Done::Added
                        }
                    };
                    let id = rs["id"].as_str().ok_or("the entrypoint has no id")?;
                    cf.put_zone_ruleset(zone, id, &json!({ "rules": out }))?;
                    done
                }
            }
        }
    };
    read_back(cf, zone, &want)?;
    Ok(done)
}

/// The entrypoint read again holds the rule as written, once.
fn read_back(cf: &Cf, zone: &str, want: &Value) -> Res<()> {
    let rs = cf.phase_entrypoint(zone, PHASE)?.ok_or("read back: no http_config_settings entrypoint")?;
    let n = rs["rules"].as_array().map_or(0, |r| r.iter().filter(|r| same(r, want)).count());
    if n == 1 { Ok(()) } else { Err(format!("read back: {n} rules named {RULE_REF} as written, want 1")) }
}

/// Why the fetch of `url` failed: on a workers.dev host, the cause that no rule can remove; elsewhere, the rule to run.
fn refusal(url: &str, status: u16, text: &str) -> String {
    let host = url.split("://").nth(1).unwrap_or(url).split(['/', ':']).next().unwrap_or("");
    if on_workers_dev(host) {
        format!("GET {url} as {CHECK_AGENT}: HTTP {status}: {WORKERS_DEV}")
    } else {
        let body: String = text.chars().take(80).collect();
        format!(
            "GET {url} as {CHECK_AGENT}: HTTP {status}: {body} (Browser Integrity Check? run `graftyard oidc bic-rule`)"
        )
    }
}

/// The issuer's JWKS fetched as a stock Python client: Ok with the number of keys on 200, Err on anything else.
pub fn check(edge: &str) -> Res<usize> {
    let url = format!("{}{ISSUER_PATH}/.well-known/jwks", edge.trim_end_matches('/'));
    let (status, text) = match ureq::get(&url).set("user-agent", CHECK_AGENT).call() {
        Ok(r) => (r.status(), r.into_string().map_err(|e| e.to_string())?),
        Err(ureq::Error::Status(code, r)) => (code, r.into_string().unwrap_or_default()),
        Err(e) => return Err(format!("GET {url}: {e}")),
    };
    if status != 200 {
        return Err(refusal(&url, status, &text));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("GET {url}: not JSON: {e}"))?;
    v["keys"].as_array().map(Vec::len).ok_or_else(|| format!("GET {url}: no keys"))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};

    use super::{CHECK_AGENT, Done, RULE_REF, bic_rule, check, rule};
    use crate::cf::Cf;
    use crate::genesis::tests::serve;

    /// A stub zone `z` whose `http_config_settings` entrypoint is `entry` (null: none yet). PUT and POST replace it, as
    /// Cloudflare does, adding an id to each rule without one; every request is logged as "METHOD path body".
    fn stub(
        entry: Value,
        log: Arc<Mutex<Vec<String>>>,
    ) -> (Cf, std::sync::mpsc::Receiver<crate::genesis::tests::Seen>) {
        let entry = Arc::new(Mutex::new(entry));
        let (url, rx) = serve(16, move |s| {
            let mut it = s.line.split(' ');
            let (m, p) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
            log.lock().unwrap().push(format!("{m} {p} {}", String::from_utf8_lossy(&s.body)));
            let ok = |r: Value| (200, json!({ "success": true, "result": r }).to_string());
            let stamp = |mut v: Value| {
                for (i, r) in v["rules"].as_array_mut().into_iter().flatten().enumerate() {
                    if r.get("id").is_none() {
                        r["id"] = json!(format!("new-{i}"));
                    }
                    r["version"] = json!("1");
                }
                v
            };
            let mut e = entry.lock().unwrap();
            match (m, p) {
                ("GET", "/zones/z/rulesets/phases/http_config_settings/entrypoint") if e.is_null() => (
                    404,
                    json!({ "success": false, "errors": [{ "code": 10003, "message": "not found" }] }).to_string(),
                ),
                ("GET", "/zones/z/rulesets/phases/http_config_settings/entrypoint") => ok(e.clone()),
                ("POST", "/zones/z/rulesets") => {
                    let mut v: Value = serde_json::from_slice(&s.body).unwrap();
                    v["id"] = json!("rs1");
                    *e = stamp(v);
                    ok(e.clone())
                }
                ("PUT", "/zones/z/rulesets/rs1") => {
                    let v: Value = serde_json::from_slice(&s.body).unwrap();
                    e["rules"] = v["rules"].clone();
                    *e = stamp(e.clone());
                    ok(e.clone())
                }
                _ => (404, json!({ "success": false, "errors": [{ "code": 7003, "message": p }] }).to_string()),
            }
        });
        (Cf::with(&url, "a", "t"), rx)
    }

    fn writes(log: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        log.lock().unwrap().iter().filter(|l| !l.starts_with("GET")).cloned().collect()
    }

    /// No entrypoint: one is created holding the rule, which is BIC off on the issuer's path of this host only, and
    /// a second run writes nothing.
    #[test]
    fn the_rule_is_created_once_and_scoped_to_the_issuer() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (cf, _rx) = stub(Value::Null, log.clone());
        assert_eq!(bic_rule(&cf, "z", "git.example").unwrap(), Done::Created);
        let w = writes(&log);
        assert_eq!(w.len(), 1, "{w:?}");
        let body: Value = serde_json::from_str(w[0].splitn(3, ' ').nth(2).unwrap()).unwrap();
        assert_eq!((body["kind"].as_str(), body["phase"].as_str()), (Some("zone"), Some("http_config_settings")));
        let r = &body["rules"][0];
        assert_eq!(r["action_parameters"], json!({ "bic": false }));
        assert_eq!(
            r["expression"],
            json!("(http.host eq \"git.example\" and starts_with(http.request.uri.path, \"/_services/token/\"))")
        );
        log.lock().unwrap().clear();
        assert_eq!(bic_rule(&cf, "z", "git.example").unwrap(), Done::Kept);
        assert!(writes(&log).is_empty(), "{:?}", log.lock().unwrap());
    }

    /// An entrypoint with the customer's own rules: the rule is added after them and theirs are sent back unchanged;
    /// a rule of ours with another host is rewritten in place under its id, and the others still kept.
    #[test]
    fn other_rules_are_kept_and_a_stale_rule_is_rewritten_in_place() {
        let theirs = json!({ "id": "c1", "ref": "customer", "expression": "true", "action": "set_config",
            "action_parameters": { "email_obfuscation": true }, "enabled": true, "version": "3" });
        let log = Arc::new(Mutex::new(Vec::new()));
        let (cf, _rx) =
            stub(json!({ "id": "rs1", "phase": "http_config_settings", "rules": [theirs.clone()] }), log.clone());
        assert_eq!(bic_rule(&cf, "z", "git.example").unwrap(), Done::Added);
        let put = writes(&log).pop().unwrap();
        assert!(put.starts_with("PUT /zones/z/rulesets/rs1 "), "{put}");
        let rules: Value = serde_json::from_str(put.splitn(3, ' ').nth(2).unwrap()).unwrap();
        let mut kept = theirs.clone();
        kept.as_object_mut().unwrap().remove("version");
        assert_eq!(rules["rules"][0], kept);
        assert_eq!(rules["rules"][1]["ref"], json!(RULE_REF));

        let mut stale = rule("old.example");
        stale["id"] = json!("mine");
        let log = Arc::new(Mutex::new(Vec::new()));
        let (cf, _rx) = stub(json!({ "id": "rs1", "rules": [theirs.clone(), stale] }), log.clone());
        assert_eq!(bic_rule(&cf, "z", "git.example").unwrap(), Done::Rewritten);
        let put = writes(&log).pop().unwrap();
        let rules: Value = serde_json::from_str(put.splitn(3, ' ').nth(2).unwrap()).unwrap();
        assert_eq!(rules["rules"].as_array().unwrap().len(), 2);
        assert_eq!(rules["rules"][1]["id"], json!("mine"));
        assert!(rules["rules"][1]["expression"].as_str().unwrap().contains("\"git.example\""));
    }

    /// A workers.dev host is refused before any API call, naming the cause.
    #[test]
    fn bic_rule_refuses_a_workers_dev_host() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (cf, _rx) = stub(Value::Null, log.clone());
        let e = bic_rule(&cf, "z", "graft-m4a-edge.acme.workers.dev").unwrap_err();
        assert!(e.contains("workers.dev") && e.contains("cannot be turned off"), "{e}");
        assert!(log.lock().unwrap().is_empty(), "{:?}", log.lock().unwrap());
        // A custom host whose name merely contains or ends in the string, in another domain, is not refused.
        assert_eq!(bic_rule(&cf, "z", "git.notworkers.dev").unwrap(), Done::Created);
    }

    /// On a workers.dev edge the check's failure says why it can never pass, not only "403"; on a custom host it names
    /// the rule to run instead.
    #[test]
    fn the_check_names_workers_dev_as_the_cause() {
        let wd = super::refusal(
            "https://graft-m4a-edge.acme.workers.dev/_services/token/.well-known/jwks",
            403,
            "error code: 1010",
        );
        assert!(wd.contains("cannot be turned off") && wd.contains("custom host") && wd.contains("HTTP 403"), "{wd}");
        let own = super::refusal("https://git.example/_services/token/.well-known/jwks", 403, "error code: 1010");
        assert!(own.contains("bic-rule") && !own.contains("cannot be turned off"), "{own}");
    }

    /// The check sends Python's agent and passes only a 200 with keys; BIC's 403 fails it naming the cause.
    #[test]
    fn the_check_fetches_the_jwks_as_python_and_fails_on_bics_403() {
        let (url, rx) = serve(1, |_| (200, json!({ "keys": [{ "kid": "a" }, { "kid": "b" }] }).to_string()));
        assert_eq!(check(&url).unwrap(), 2);
        let seen = rx.recv().unwrap();
        assert!(seen.line.starts_with("GET /_services/token/.well-known/jwks "), "{}", seen.line);
        assert!(seen.headers.iter().any(|(k, v)| k == "user-agent" && v == CHECK_AGENT));
        let (url, _rx403) = serve(1, |_| (403, "error code: 1010".into()));
        let e = check(&url).unwrap_err();
        assert!(e.contains("HTTP 403") && e.contains("error code: 1010") && e.contains("bic-rule"), "{e}");
    }
}
