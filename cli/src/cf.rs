//! The Cloudflare API calls the operator CLI makes, with the deploy token from `CLOUDFLARE_API_TOKEN`. The base URL
//! is `GRAFTYARD_CF_API` when set, so every request can be checked against a local stub.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::config::Res;

pub struct Cf {
    base: String,
    account: String,
    token: String,
    /// How often and how long `prepare_image` polls: Wrangler's 2 s and 15 minutes (`IMAGE_PREPARATION_*`).
    pub(crate) prep_poll: Duration,
    pub(crate) prep_deadline: Duration,
}

/// One module of a script upload.
pub struct Module<'a> {
    pub name: &'a str,
    pub content_type: &'a str,
    pub bytes: &'a [u8],
}

impl Cf {
    pub fn from_env(account: &str) -> Res<Cf> {
        Cf::from_env_var(account, "CLOUDFLARE_API_TOKEN")
    }

    /// The same, with the token from `var`: the runner account's commands read `CLOUDFLARE_RUNNER_API_TOKEN`, so the
    /// install's deploy token is never sent to another account.
    pub fn from_env_var(account: &str, var: &str) -> Res<Cf> {
        let token =
            std::env::var(var).map_err(|_| format!("{var} is not set: the deploy token comes from the environment"))?;
        let base = std::env::var("GRAFTYARD_CF_API").unwrap_or_else(|_| "https://api.cloudflare.com/client/v4".into());
        Ok(Cf::with(&base, account, &token))
    }

    pub fn with(base: &str, account: &str, token: &str) -> Cf {
        Cf {
            base: base.trim_end_matches('/').into(),
            account: account.into(),
            token: token.into(),
            prep_poll: Duration::from_secs(2),
            prep_deadline: Duration::from_secs(900),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/accounts/{}{path}", self.base, self.account)
    }

    fn call(&self, method: &str, path: &str, headers: &[(&str, &str)], body: Option<(&str, &[u8])>) -> Res<Value> {
        self.call_url(method, &self.url(path), path, headers, body)
    }

    fn call_url(
        &self,
        method: &str,
        url: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<(&str, &[u8])>,
    ) -> Res<Value> {
        let mut req = ureq::request(method, url).set("authorization", &format!("Bearer {}", self.token));
        for (k, v) in headers {
            req = req.set(k, v);
        }
        let r = match body {
            Some((ct, b)) => req.set("content-type", ct).send_bytes(b),
            None => req.call(),
        };
        let (status, text) = match r {
            Ok(r) => (r.status(), r.into_string().map_err(|e| e.to_string())?),
            Err(ureq::Error::Status(code, r)) => (code, r.into_string().unwrap_or_default()),
            Err(e) => return Err(format!("{method} {path}: {e}")),
        };
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !(200..300).contains(&status) || v["success"] == json!(false) {
            return Err(format!("{method} {path}: HTTP {status}: {}", errors(&v, &text)));
        }
        Ok(v)
    }

    fn json(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &Value) -> Res<Value> {
        self.call(method, path, headers, Some(("application/json", body.to_string().as_bytes())))
    }

    // ---- Spectrum (zone-scoped) ----

    fn zone(&self, method: &str, zone: &str, path: &str, body: Option<&Value>) -> Res<Value> {
        let url = format!("{}/zones/{zone}{path}", self.base);
        let b = body.map(|b| b.to_string());
        self.call_url(method, &url, path, &[], b.as_deref().map(|b| ("application/json", b.as_bytes())))
    }

    /// Every Spectrum application on the zone.
    pub fn spectrum_apps(&self, zone: &str) -> Res<Vec<Value>> {
        let v = self.zone("GET", zone, "/spectrum/apps", None)?;
        Ok(v["result"].as_array().cloned().unwrap_or_default())
    }

    pub fn create_spectrum_app(&self, zone: &str, body: &Value) -> Res<Value> {
        Ok(self.zone("POST", zone, "/spectrum/apps", Some(body))?["result"].clone())
    }

    pub fn update_spectrum_app(&self, zone: &str, id: &str, body: &Value) -> Res<Value> {
        Ok(self.zone("PUT", zone, &format!("/spectrum/apps/{id}"), Some(body))?["result"].clone())
    }

    pub fn delete_spectrum_app(&self, zone: &str, id: &str) -> Res<()> {
        self.zone("DELETE", zone, &format!("/spectrum/apps/{id}"), None).map(|_| ())
    }

    // ---- Rulesets (zone-scoped; `cli/src/oidc.rs`) ----

    /// The zone's entrypoint ruleset for `phase`, or None when the zone has none yet (404).
    pub fn phase_entrypoint(&self, zone: &str, phase: &str) -> Res<Option<Value>> {
        match self.zone("GET", zone, &format!("/rulesets/phases/{phase}/entrypoint"), None) {
            Ok(v) => Ok(Some(v["result"].clone())),
            Err(e) if e.contains("HTTP 404") => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn create_zone_ruleset(&self, zone: &str, body: &Value) -> Res<Value> {
        Ok(self.zone("POST", zone, "/rulesets", Some(body))?["result"].clone())
    }

    pub fn put_zone_ruleset(&self, zone: &str, id: &str, body: &Value) -> Res<Value> {
        Ok(self.zone("PUT", zone, &format!("/rulesets/{id}"), Some(body))?["result"].clone())
    }

    /// A script's tag, the id a Spectrum Worker application names as its origin.
    pub fn script_tag(&self, script: &str) -> Res<String> {
        let v = self.call("GET", "/workers/scripts", &[], None)?;
        v["result"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|s| s["id"] == json!(script))
            .and_then(|s| s["tag"].as_str().map(String::from))
            .ok_or_else(|| format!("no script {script} in the account"))
    }

    // ---- Workers scripts ----

    /// Every script in the account, with its applied migration tag.
    pub fn scripts(&self) -> Res<Vec<(String, Option<String>)>> {
        let v = self.call("GET", "/workers/scripts", &[], None)?;
        Ok(v["result"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|s| {
                        (s["id"].as_str().unwrap_or("").to_string(), s["migration_tag"].as_str().map(String::from))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The bindings of a script's latest version.
    pub fn bindings(&self, script: &str) -> Res<Vec<Value>> {
        let v = self.call("GET", &format!("/workers/scripts/{script}/settings"), &[], None)?;
        Ok(v["result"]["bindings"].as_array().cloned().unwrap_or_default())
    }

    /// Uploads and deploys one version of `script` (`PUT /workers/scripts/{name}`, multipart), after preparing every
    /// image its metadata names.
    pub fn put_script(&self, script: &str, metadata: &Value, modules: &[Module]) -> Res<()> {
        self.prepare_images(metadata)?;
        self.multipart("PUT", &format!("/workers/scripts/{script}"), metadata, modules).map(|_| ())
    }

    /// Uploads one version of `script` without deploying it (`POST /workers/scripts/{name}/versions`), after preparing
    /// every image its metadata names, and answers its id.
    pub fn upload_version(&self, script: &str, metadata: &Value, modules: &[Module]) -> Res<String> {
        self.prepare_images(metadata)?;
        let v = self.multipart("POST", &format!("/workers/scripts/{script}/versions"), metadata, modules)?;
        v["result"]["id"].as_str().map(String::from).ok_or_else(|| "version upload returned no id".into())
    }

    /// The versions and percentages of the script's latest deployment, and when it was made.
    pub fn deployed(&self, script: &str) -> Res<(Vec<(String, f64)>, String)> {
        let v = self.call("GET", &format!("/workers/scripts/{script}/deployments"), &[], None)?;
        let d = &v["result"]["deployments"][0];
        let versions = d["versions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| (x["version_id"].as_str().unwrap_or("").to_string(), x["percentage"].as_f64().unwrap_or(0.0)))
            .collect();
        Ok((versions, d["created_on"].as_str().unwrap_or("").to_string()))
    }

    /// One version as the versions API reads it: its `annotations`, and its `resources` (bindings, without a secret's
    /// value, and `script_runtime`).
    pub fn version(&self, script: &str, version: &str) -> Res<Value> {
        let v = self.call("GET", &format!("/workers/scripts/{script}/versions/{version}"), &[], None)?;
        Ok(v["result"].clone())
    }

    /// A version's `workers/tag` annotation.
    pub fn version_tag(&self, script: &str, version: &str) -> Res<Option<String>> {
        Ok(self.version(script, version)?["annotations"]["workers/tag"].as_str().map(String::from))
    }

    /// A percentage deployment of `versions`, whose percentages sum to 100.
    pub fn deploy_versions(&self, script: &str, versions: &[(&str, u8)], message: &str) -> Res<()> {
        let body = json!({
            "strategy": "percentage",
            "versions": versions.iter().map(|(id, p)| json!({ "version_id": id, "percentage": p })).collect::<Vec<_>>(),
            "annotations": { "workers/message": message },
        });
        self.json("POST", &format!("/workers/scripts/{script}/deployments"), &[], &body).map(|_| ())
    }

    fn multipart(&self, method: &str, path: &str, metadata: &Value, modules: &[Module]) -> Res<Value> {
        let boundary = format!("graftyard{}", crate::config::hex(&crate::config::random::<12>()));
        let mut body = Vec::new();
        let mut part = |name: &str, filename: Option<&str>, ct: &str, bytes: &[u8]| {
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"").as_bytes(),
            );
            if let Some(f) = filename {
                body.extend_from_slice(format!("; filename=\"{f}\"").as_bytes());
            }
            body.extend_from_slice(format!("\r\nContent-Type: {ct}\r\n\r\n").as_bytes());
            body.extend_from_slice(bytes);
            body.extend_from_slice(b"\r\n");
        };
        part("metadata", None, "application/json", metadata.to_string().as_bytes());
        for m in modules {
            part(m.name, Some(m.name), m.content_type, m.bytes);
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let ct = format!("multipart/form-data; boundary={boundary}");
        self.call(method, path, &[], Some((&ct, &body)))
    }

    pub fn workers_dev(&self, script: &str) -> Res<bool> {
        let v = self.call("GET", &format!("/workers/scripts/{script}/subdomain"), &[], None)?;
        Ok(v["result"]["enabled"] == json!(true))
    }

    pub fn set_workers_dev(&self, script: &str, enabled: bool) -> Res<()> {
        let body = json!({ "enabled": enabled, "previews_enabled": false });
        self.json("POST", &format!("/workers/scripts/{script}/subdomain"), &[], &body).map(|_| ())
    }

    pub fn subdomain(&self) -> Res<String> {
        let v = self.call("GET", "/workers/subdomain", &[], None)?;
        v["result"]["subdomain"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| "the account has no workers.dev subdomain".into())
    }

    /// Replaces the script's Cron Triggers.
    pub fn put_schedules(&self, script: &str, crons: &[&str]) -> Res<()> {
        let body = json!(crons.iter().map(|c| json!({ "cron": c })).collect::<Vec<_>>());
        self.json("PUT", &format!("/workers/scripts/{script}/schedules"), &[], &body).map(|_| ())
    }

    // ---- Containers ----

    /// The Durable Object-managed container application of a namespace, whose id is the namespace's id.
    pub fn container_app(&self, namespace: &str) -> Res<Option<Value>> {
        match self.call("GET", &format!("/containers/applications/{namespace}"), &[], None) {
            Ok(v) => Ok(Some(v["result"].clone())),
            Err(e) if e.contains("HTTP 404") => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn create_container_app(&self, body: &Value) -> Res<()> {
        self.json("POST", "/containers/applications", &[], body).map(|_| ())
    }

    /// Prepares a digest-pinned managed image for the Containers runtime and waits until it is ready, as Wrangler does
    /// before it uploads a version with an image map (`waitForImagePreparation`). A version naming an unprepared image
    /// uploads, but every `ctx.container.start` of it fails with "Image not found" (R23's M1 check, run 1712951: 8 of 8
    /// starts). An image already prepared answers `ready` on the first call.
    pub fn prepare_image(&self, image: &str) -> Res<()> {
        let deadline = Instant::now() + self.prep_deadline;
        loop {
            let v = self
                .json("POST", "/containers/image-preparations", &[], &json!({ "image": image }))
                .map_err(|e| format!("preparing image {image}: {e}"))?;
            match v["result"]["status"].as_str() {
                Some("ready") => return Ok(()),
                Some("pending") if Instant::now() + self.prep_poll <= deadline => std::thread::sleep(self.prep_poll),
                Some("pending") => {
                    return Err(format!(
                        "image {image} was not ready after {} s of preparation",
                        self.prep_deadline.as_secs()
                    ));
                }
                Some("error") => {
                    return Err(format!(
                        "preparing image {image}: {}",
                        v["result"]["reason"].as_str().unwrap_or("the preparation failed")
                    ));
                }
                other => return Err(format!("preparing image {image}: unknown status {other:?}")),
            }
        }
    }

    /// Every image a version's `containers` entries name in their `images` maps, each prepared once.
    fn prepare_images(&self, metadata: &Value) -> Res<()> {
        let mut images: Vec<&str> = metadata["containers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["images"].as_object())
            .flat_map(|m| m.values().filter_map(Value::as_str))
            .collect();
        images.sort_unstable();
        images.dedup();
        images.into_iter().try_for_each(|i| self.prepare_image(i))
    }

    /// Every container application in the account.
    pub fn container_apps(&self) -> Res<Vec<Value>> {
        let v = self.call("GET", "/containers/applications", &[], None)?;
        Ok(v["result"].as_array().cloned().unwrap_or_default())
    }

    /// Changes an application's configuration, then rolls it out to every instance as wrangler does (`full_auto`).
    pub fn roll_container_app(&self, id: &str, configuration: &Value) -> Res<()> {
        self.json("PATCH", &format!("/containers/applications/{id}"), &[], &json!({ "configuration": configuration }))?;
        let body = json!({
            "description": "graftyard runners upgrade",
            "strategy": "rolling",
            "kind": "full_auto",
            "step_percentage": 100,
            "target_configuration": configuration,
        });
        self.json("POST", &format!("/containers/applications/{id}/rollouts"), &[], &body).map(|_| ())
    }

    pub fn delete_script(&self, script: &str) -> Res<()> {
        self.call("DELETE", &format!("/workers/scripts/{script}?force=true"), &[], None).map(|_| ())
    }

    // ---- R2 ----

    fn jurisdiction_header(j: Option<&str>) -> Vec<(&str, &str)> {
        j.map(|j| vec![("cf-r2-jurisdiction", j)]).unwrap_or_default()
    }

    /// The bucket's `location` (upper case, as R2 returns it), or `None` when it does not exist.
    pub fn bucket_location(&self, bucket: &str, jurisdiction: Option<&str>) -> Res<Option<String>> {
        match self.call("GET", &format!("/r2/buckets/{bucket}"), &Self::jurisdiction_header(jurisdiction), None) {
            Ok(v) => Ok(Some(v["result"]["location"].as_str().unwrap_or("").to_ascii_uppercase())),
            Err(e) if e.contains("HTTP 404") => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn create_bucket(&self, bucket: &str, hint: &str, jurisdiction: Option<&str>) -> Res<String> {
        let v = self.json(
            "POST",
            "/r2/buckets",
            &Self::jurisdiction_header(jurisdiction),
            &json!({ "name": bucket, "locationHint": hint }),
        )?;
        Ok(v["result"]["location"].as_str().unwrap_or("").to_ascii_uppercase())
    }

    pub fn delete_bucket(&self, bucket: &str, jurisdiction: Option<&str>) -> Res<()> {
        self.call("DELETE", &format!("/r2/buckets/{bucket}"), &Self::jurisdiction_header(jurisdiction), None)
            .map(|_| ())
    }

    /// The bucket's object-lock rules, or an empty list when it has none.
    pub fn bucket_lock(&self, bucket: &str) -> Res<Vec<Value>> {
        let v = self.call("GET", &format!("/r2/buckets/{bucket}/lock"), &[], None)?;
        Ok(v["result"]["rules"].as_array().cloned().unwrap_or_default())
    }

    /// Replaces the bucket's object-lock rules with `rules`.
    pub fn put_bucket_lock(&self, bucket: &str, rules: &[Value]) -> Res<()> {
        self.json("PUT", &format!("/r2/buckets/{bucket}/lock"), &[], &json!({ "rules": rules })).map(|_| ())
    }

    // ---- Secrets Store ----

    /// The id of the store named `name`, created when missing.
    pub fn store(&self, name: &str) -> Res<String> {
        let v = self.call("GET", "/secrets_store/stores", &[], None)?;
        if let Some(s) = v["result"].as_array().into_iter().flatten().find(|s| s["name"] == json!(name)) {
            return Ok(s["id"].as_str().unwrap_or("").into());
        }
        let v = self.json("POST", "/secrets_store/stores", &[], &json!({ "name": name }))?;
        v["result"]["id"].as_str().map(String::from).ok_or_else(|| "store create returned no id".into())
    }

    /// Writes `value` under `name`, replacing an earlier value of the same name. The value is write-only.
    pub fn put_secret(&self, store: &str, name: &str, value: &str, comment: &str) -> Res<()> {
        let path = format!("/secrets_store/stores/{store}/secrets");
        let v = self.call("GET", &format!("{path}?search={name}"), &[], None)?;
        if let Some(s) = v["result"].as_array().into_iter().flatten().find(|s| s["name"] == json!(name)) {
            let id = s["id"].as_str().unwrap_or("");
            return self
                .json("PATCH", &format!("{path}/{id}"), &[], &json!({ "value": value, "comment": comment }))
                .map(|_| ());
        }
        let body = json!([{ "name": name, "value": value, "comment": comment, "scopes": ["workers"] }]);
        self.json("POST", &path, &[], &body).map(|_| ())
    }
}

fn errors(v: &Value, text: &str) -> String {
    match v["errors"].as_array() {
        Some(e) if !e.is_empty() => e
            .iter()
            .map(|e| format!("{} {}", e["code"], e["message"].as_str().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("; "),
        _ => text.chars().take(300).collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::genesis::tests::serve;

    const IMAGE: &str =
        "registry.cloudflare.com/acct/git@sha256:1111111111111111111111111111111111111111111111111111111111111111";

    /// A stub account whose image preparation answers `statuses` in turn (the last one repeats), logging every
    /// request as "METHOD path".
    fn account(
        statuses: &'static [&'static str],
        log: Arc<Mutex<Vec<String>>>,
    ) -> (Cf, std::sync::mpsc::Receiver<crate::genesis::tests::Seen>) {
        let n = Arc::new(Mutex::new(0usize));
        let (url, rx) = serve(64, move |s| {
            let mut it = s.line.split(' ');
            let (m, p) = (it.next().unwrap_or(""), it.next().unwrap_or("").split("/accounts/a").nth(1).unwrap_or(""));
            log.lock().unwrap().push(format!("{m} {p}"));
            let ok = |r: Value| (200, json!({ "success": true, "result": r }).to_string());
            if p == "/containers/image-preparations" {
                let body: Value = serde_json::from_slice(&s.body).unwrap();
                assert_eq!(body, json!({ "image": IMAGE }));
                let mut i = n.lock().unwrap();
                let st = statuses[(*i).min(statuses.len() - 1)];
                *i += 1;
                return ok(json!({ "status": st, "reason": "the image is not in this account" }));
            }
            ok(json!({ "id": "v1" }))
        });
        let mut cf = Cf::with(&url, "a", "t");
        cf.prep_poll = Duration::from_millis(20);
        cf.prep_deadline = Duration::from_millis(200);
        (cf, rx)
    }

    fn git_shard() -> Value {
        json!({ "containers": [{ "name": "s1-gitshard", "class_name": "GitShard", "images": { "git": IMAGE } }] })
    }

    /// The site script's upload (`graftyard sites`, `put_script`) and the runner script's version (`graftyard
    /// runners upgrade`, `upload_version`) each prepare their image first; an image prepared already costs one call.
    #[test]
    fn an_upload_naming_an_image_prepares_it_first_and_a_prepared_image_returns_at_once() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (cf, _rx) = account(&["ready"], log.clone());
        let t = Instant::now();
        cf.put_script("s1", &git_shard(), &[]).unwrap();
        assert!(t.elapsed() < cf.prep_poll, "a prepared image does not wait a poll");
        let runner = json!({ "containers": [
            { "name": "graft-runners-sandboxstd3", "class_name": "SandboxStd3" },
            { "name": "graft-runners-runnerbox", "class_name": "RunnerBox", "images": { "runner": IMAGE, "again": IMAGE } },
        ] });
        assert_eq!(cf.upload_version("graft-runners", &runner, &[]).unwrap(), "v1");
        // A version with no image map prepares nothing.
        cf.put_script("probe", &json!({ "main_module": "probe.js" }), &[]).unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            [
                "POST /containers/image-preparations",
                "PUT /workers/scripts/s1",
                "POST /containers/image-preparations",
                "POST /workers/scripts/graft-runners/versions",
                "PUT /workers/scripts/probe",
            ]
        );
    }

    #[test]
    fn a_pending_image_is_polled_until_ready() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (cf, _rx) = account(&["pending", "pending", "ready"], log.clone());
        cf.upload_version("graft-runners", &git_shard(), &[]).unwrap();
        let preps = log.lock().unwrap().iter().filter(|l| l.ends_with("image-preparations")).count();
        assert_eq!(preps, 3);
        assert_eq!(log.lock().unwrap().last().unwrap(), "POST /workers/scripts/graft-runners/versions");
    }

    /// An image never ready, or refused, stops the upload: no version names an image that cannot start.
    #[test]
    fn an_image_never_ready_or_refused_uploads_nothing_and_names_the_image() {
        for (statuses, want) in
            [(&["pending"][..], "was not ready after"), (&["error"][..], "the image is not in this account")]
        {
            let log = Arc::new(Mutex::new(Vec::new()));
            let (cf, _rx) = account(statuses, log.clone());
            let e = cf.upload_version("graft-runners", &git_shard(), &[]).unwrap_err();
            assert!(e.contains(IMAGE) && e.contains(want), "{e}");
            let e = cf.put_script("s1", &git_shard(), &[]).unwrap_err();
            assert!(e.contains(IMAGE), "{e}");
            assert!(log.lock().unwrap().iter().all(|l| l.ends_with("image-preparations")), "{:?}", log.lock().unwrap());
        }
    }
}

/// R23's hosted check: uploads one `graft-runners` test version through `Cf::upload_version` (preparing its image),
/// from `R23_UPLOAD=<bundle dir>` with `R23_META=<metadata JSON>`, on the runner account. Ignored by default.
#[cfg(test)]
mod hosted {
    #[test]
    #[ignore]
    fn r23_upload_test_version() {
        let dir = std::env::var("R23_UPLOAD").unwrap();
        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(std::env::var("R23_META").unwrap()).unwrap()).unwrap();
        let mut files = Vec::new();
        for e in std::fs::read_dir(&dir).unwrap() {
            let n = e.unwrap().file_name().into_string().unwrap();
            let ct = if n.ends_with(".wasm") {
                "application/wasm"
            } else if n == "entry.js" {
                "application/javascript+module"
            } else {
                continue;
            };
            files.push((n.clone(), ct, std::fs::read(format!("{dir}/{n}")).unwrap()));
        }
        let modules: Vec<super::Module> =
            files.iter().map(|(n, ct, b)| super::Module { name: n, content_type: ct, bytes: b }).collect();
        let account = std::env::var("CLOUDFLARE_RUNNER_ACCOUNT_ID").unwrap();
        let cf = super::Cf::from_env_var(&account, "CLOUDFLARE_RUNNER_API_TOKEN").unwrap();
        let t = std::time::Instant::now();
        let id = cf.upload_version("graft-runners", &meta, &modules).unwrap();
        println!("R23 upload: version {id} after {} ms", t.elapsed().as_millis());
    }
}
