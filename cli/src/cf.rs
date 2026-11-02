//! The Cloudflare API calls the operator CLI makes, with the deploy token from `CLOUDFLARE_API_TOKEN`. The base URL
//! is `GRAFTYARD_CF_API` when set, so every request can be checked against a local stub.

use serde_json::{Value, json};

use crate::config::Res;

pub struct Cf {
    base: String,
    account: String,
    token: String,
}

/// One module of a script upload.
pub struct Module<'a> {
    pub name: &'a str,
    pub content_type: &'a str,
    pub bytes: &'a [u8],
}

impl Cf {
    pub fn from_env(account: &str) -> Res<Cf> {
        let token = std::env::var("CLOUDFLARE_API_TOKEN")
            .map_err(|_| "CLOUDFLARE_API_TOKEN is not set: the deploy token comes from the environment".to_string())?;
        let base = std::env::var("GRAFTYARD_CF_API").unwrap_or_else(|_| "https://api.cloudflare.com/client/v4".into());
        Ok(Cf { base: base.trim_end_matches('/').into(), account: account.into(), token })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/accounts/{}{path}", self.base, self.account)
    }

    fn call(&self, method: &str, path: &str, headers: &[(&str, &str)], body: Option<(&str, &[u8])>) -> Res<Value> {
        let mut req = ureq::request(method, &self.url(path)).set("authorization", &format!("Bearer {}", self.token));
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

    /// Uploads and deploys one version of `script` (`PUT /workers/scripts/{name}`, multipart).
    pub fn put_script(&self, script: &str, metadata: &Value, modules: &[Module]) -> Res<()> {
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
        self.call("PUT", &format!("/workers/scripts/{script}"), &[], Some((&ct, &body)))?;
        Ok(())
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
