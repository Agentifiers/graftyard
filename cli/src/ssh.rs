//! `graftyard ssh spectrum`: the install's Spectrum TCP application on port 22, whose origin is the edge script's
//! `connect()` handler (`design/DESIGN.md`, Shape, "SSH front"; `build/BUILD-PLAN.md`, M6-WP2). Idempotent: an app
//! for the hostname that already names the edge is left alone, one that names another origin is rewritten, and
//! `--delete` removes it. A request path never makes these calls; the operator runs them once per install.
//!
//! The body carries no `tls` field, which a Pro zone refuses with 10012, and no edge IPs, which a pay-as-you-go zone
//! does not take [measured, `tests/m0/results/m0-wp7.md`, B0-24c]. A pay-as-you-go zone holds one Worker app per
//! transport, so the zone's TCP Worker app is this one (11034 otherwise) [doc].

use serde_json::{Value, json};

use crate::cf::Cf;
use crate::config::Res;

/// What `spectrum` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Done {
    Created(String),
    Kept(String),
    Rewritten(String),
    Deleted(String),
    Absent,
}

pub fn app_body(hostname: &str, worker_tag: &str) -> Value {
    json!({
        "protocol": "tcp/22",
        "dns": { "type": "CNAME", "name": hostname },
        "traffic_type": "worker",
        "origin_worker_id": worker_tag,
    })
}

pub fn spectrum(cf: &Cf, zone: &str, hostname: &str, edge: &str, delete: bool) -> Res<Done> {
    let apps = cf.spectrum_apps(zone)?;
    let app = apps.iter().find(|a| a["dns"]["name"].as_str() == Some(hostname));
    let id = |a: &Value| a["id"].as_str().unwrap_or_default().to_string();
    if delete {
        return match app {
            Some(a) => cf.delete_spectrum_app(zone, &id(a)).map(|_| Done::Deleted(id(a))),
            None => Ok(Done::Absent),
        };
    }
    let tag = cf.script_tag(edge)?;
    let body = app_body(hostname, &tag);
    match app {
        Some(a) if a["origin_worker_id"].as_str() == Some(&tag) && a["protocol"] == json!("tcp/22") => {
            Ok(Done::Kept(id(a)))
        }
        Some(a) => cf.update_spectrum_app(zone, &id(a), &body).map(|_| Done::Rewritten(id(a))),
        None => {
            let r = cf.create_spectrum_app(zone, &body)?;
            Ok(Done::Created(r["id"].as_str().unwrap_or_default().to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};

    use super::{Done, spectrum};
    use crate::cf::Cf;
    use crate::genesis::tests::serve;

    /// A stub zone holding `apps` and an account whose `graft-edge` script has tag `tag-edge`; every request is logged
    /// as "METHOD path body".
    fn stub(apps: Value, log: Arc<Mutex<Vec<String>>>) -> (Cf, std::sync::mpsc::Receiver<crate::genesis::tests::Seen>) {
        let apps = Arc::new(Mutex::new(apps));
        let (url, rx) = serve(16, move |s| {
            let mut it = s.line.split(' ');
            let (m, p) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
            log.lock().unwrap().push(format!("{m} {p} {}", String::from_utf8_lossy(&s.body)));
            let ok = |r: Value| (200, json!({ "success": true, "result": r }).to_string());
            match (m, p) {
                ("GET", "/accounts/a/workers/scripts") => {
                    ok(json!([{ "id": "graft-other", "tag": "tag-other" }, { "id": "graft-edge", "tag": "tag-edge" }]))
                }
                ("GET", "/zones/z/spectrum/apps") => ok(apps.lock().unwrap().clone()),
                ("POST", "/zones/z/spectrum/apps") => {
                    let body: Value = serde_json::from_slice(&s.body).unwrap();
                    if body.get("tls").is_some() {
                        return (400, json!({ "success": false, "errors": [{ "code": 10012, "message": "unknown field \"tls\"" }] }).to_string());
                    }
                    ok(json!({ "id": "new-app" }))
                }
                _ => ok(json!({})),
            }
        });
        (Cf::with(&url, "a", "t"), rx)
    }

    /// A new app names the edge's tag as its Worker origin on tcp/22 with the hostname's CNAME and no `tls`; a second
    /// run leaves it alone; an app with another origin is rewritten; `--delete` removes it and is a no-op after.
    #[test]
    fn the_spectrum_app_is_created_once_rewritten_when_wrong_and_deleted() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (cf, _rx) =
            stub(json!([{ "id": "keep", "dns": { "name": "other.example" }, "origin_worker_id": "x" }]), log.clone());
        assert_eq!(spectrum(&cf, "z", "ssh.example", "graft-edge", false).unwrap(), Done::Created("new-app".into()));
        let post = log.lock().unwrap().iter().find(|l| l.starts_with("POST")).cloned().unwrap();
        let body: Value = serde_json::from_str(post.splitn(3, ' ').nth(2).unwrap()).unwrap();
        assert_eq!(
            body,
            json!({ "protocol": "tcp/22", "dns": { "type": "CNAME", "name": "ssh.example" }, "traffic_type": "worker",
                    "origin_worker_id": "tag-edge" })
        );

        let present = json!([{ "id": "app1", "protocol": "tcp/22", "dns": { "name": "ssh.example" }, "origin_worker_id": "tag-edge" }]);
        let log = Arc::new(Mutex::new(Vec::new()));
        let (cf, _rx) = stub(present.clone(), log.clone());
        assert_eq!(spectrum(&cf, "z", "ssh.example", "graft-edge", false).unwrap(), Done::Kept("app1".into()));
        assert!(log.lock().unwrap().iter().all(|l| l.starts_with("GET")), "{:?}", log.lock().unwrap());

        let log = Arc::new(Mutex::new(Vec::new()));
        let wrong = json!([{ "id": "app1", "protocol": "tcp/22", "dns": { "name": "ssh.example" }, "origin_worker_id": "tag-other" }]);
        let (cf, _rx) = stub(wrong, log.clone());
        assert_eq!(spectrum(&cf, "z", "ssh.example", "graft-edge", false).unwrap(), Done::Rewritten("app1".into()));
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|l| l.starts_with("PUT /zones/z/spectrum/apps/app1 ") && l.contains("tag-edge"))
        );

        let log = Arc::new(Mutex::new(Vec::new()));
        let (cf, _rx) = stub(present, log.clone());
        assert_eq!(spectrum(&cf, "z", "ssh.example", "graft-edge", true).unwrap(), Done::Deleted("app1".into()));
        assert!(log.lock().unwrap().iter().any(|l| l.starts_with("DELETE /zones/z/spectrum/apps/app1")));
        let (cf, _rx) = stub(json!([]), Arc::new(Mutex::new(Vec::new())));
        assert_eq!(spectrum(&cf, "z", "ssh.example", "graft-edge", true).unwrap(), Done::Absent);
        assert!(
            spectrum(&cf, "z", "ssh.example", "graft-missing", false).unwrap_err().contains("no script graft-missing")
        );
    }
}
