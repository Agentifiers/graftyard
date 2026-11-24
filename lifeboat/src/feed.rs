//! The lifeboat's read of the install's export bucket (`design/DESIGN.md`, "Install operations", Lifeboat: "It reads
//! the export bucket with an R2 Object Read only token scoped to that bucket"): R2's S3 API, `ListObjectsV2` after the
//! puller's position and `GetObject` of each `export/` key, signed with AWS Signature Version 4
//! (https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-header-based-auth.html; R2 takes region `auto`,
//! https://developers.cloudflare.com/r2/api/s3/tokens/). Pure: the bundle's puller sends what this builds.

use sha2::{Digest as _, Sha256};

/// The R2 S3 credentials of a bucket-scoped Object Read token, and where the bucket is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct S3 {
    /// `https://<account>.r2.cloudflarestorage.com`, or a bed's stand-in.
    pub endpoint: String,
    pub bucket: String,
    pub key_id: String,
    pub secret: String,
    /// `auto` on R2.
    pub region: String,
}

/// One signed request: its URL and the headers to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// HMAC-SHA256 (RFC 2104): a key longer than the 64-byte block is hashed first.
fn hmac(key: &[u8], msg: &str) -> Vec<u8> {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |b: u8| k.iter().map(|x| x ^ b).collect::<Vec<u8>>();
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(msg.as_bytes()).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().to_vec()
}

/// URI-encodes as SigV4 does: unreserved characters stay, everything else is `%XX`, and `/` too unless `path`.
pub fn uri_encode(s: &str, path: bool) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b'/' if path => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `YYYYMMDDTHHMMSSZ` for a time in ms since the epoch.
pub fn amz_date(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

impl S3 {
    /// A signed `GET` of `key` in the bucket (or of the bucket itself) with `query` pairs, at `now_ms`.
    pub fn get(&self, key: Option<&str>, query: &[(&str, &str)], now_ms: u64) -> Signed {
        // The endpoint's authority is the signed host; a path on it (a bed's stand-in) prefixes the canonical path.
        let rest = self.endpoint.split_once("://").map_or(self.endpoint.as_str(), |(_, r)| r).trim_end_matches('/');
        let (host, base) = rest.split_once('/').map_or((rest, String::new()), |(h, b)| (h, format!("/{b}")));
        let path = match key {
            Some(k) => format!("{base}/{}/{}", uri_encode(&self.bucket, true), uri_encode(k, true)),
            None => format!("{base}/{}", uri_encode(&self.bucket, true)),
        };
        let mut q: Vec<(String, String)> =
            query.iter().map(|(k, v)| (uri_encode(k, false), uri_encode(v, false))).collect();
        q.sort();
        let qs = q.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
        let date = amz_date(now_ms);
        let day = &date[..8];
        let canonical = format!(
            "GET\n{path}\n{qs}\nhost:{host}\nx-amz-content-sha256:{EMPTY_SHA256}\nx-amz-date:{date}\n\nhost;x-amz-content-sha256;x-amz-date\n{EMPTY_SHA256}"
        );
        let scope = format!("{day}/{}/s3/aws4_request", self.region);
        let to_sign = format!("AWS4-HMAC-SHA256\n{date}\n{scope}\n{}", hex(&Sha256::digest(canonical.as_bytes())));
        let k = hmac(
            &hmac(&hmac(&hmac(format!("AWS4{}", self.secret).as_bytes(), day), &self.region), "s3"),
            "aws4_request",
        );
        let sig = hex(&hmac(&k, &to_sign));
        let auth = format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature={sig}",
            self.key_id
        );
        let url = format!(
            "{}{path}{}",
            self.endpoint.trim_end_matches('/'),
            if qs.is_empty() { String::new() } else { format!("?{qs}") }
        );
        Signed {
            url,
            headers: vec![
                ("authorization".into(), auth),
                ("x-amz-content-sha256".into(), EMPTY_SHA256.into()),
                ("x-amz-date".into(), date),
            ],
        }
    }

    /// `ListObjectsV2` of `export/` after `after` (a key), at most `max` keys.
    pub fn list_after(&self, after: Option<&str>, max: usize, now_ms: u64) -> Signed {
        let max = max.to_string();
        let mut q = vec![("list-type", "2"), ("prefix", "export/"), ("max-keys", max.as_str())];
        if let Some(a) = after {
            q.push(("start-after", a));
        }
        self.get(None, &q, now_ms)
    }
}

/// The keys of a `ListObjectsV2` answer, in order, and whether it was truncated. Only `<Key>` and `<IsTruncated>` are
/// read; a key is XML-unescaped (`&amp;` and the like), which an `export/` key never needs.
pub fn listed(xml: &str) -> (Vec<String>, bool) {
    let mut keys = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<Key>") {
        let after = &rest[i + 5..];
        let Some(j) = after.find("</Key>") else { break };
        keys.push(
            after[..j]
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&amp;", "&"),
        );
        rest = &after[j + 6..];
    }
    (keys, xml.contains("<IsTruncated>true</IsTruncated>"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The signature of a `ListObjectsV2` with a query, against one computed independently with Python's `hmac` and
    /// `hashlib` from the same canonical request (SigV4's steps, by hand), so a wrong sort, encoding, or key
    /// derivation here fails.
    #[test]
    fn a_list_is_signed_as_sigv4_computes_it_independently() {
        let s3 = S3 {
            endpoint: "https://acct.r2.cloudflarestorage.com".into(),
            bucket: "feed-bucket".into(),
            key_id: "AKID".into(),
            secret: "secret-key".into(),
            region: "auto".into(),
        };
        // 2026-11-24T10:15:00Z.
        let at = 1_795_515_300_000;
        assert_eq!(amz_date(at), "20261124T101500Z");
        let r = s3.list_after(Some("export/000000000100/0000000003"), 100, at);
        assert_eq!(
            r.url,
            "https://acct.r2.cloudflarestorage.com/feed-bucket?list-type=2&max-keys=100&prefix=export%2F&start-after=export%2F000000000100%2F0000000003"
        );
        let auth = &r.headers.iter().find(|(k, _)| k == "authorization").unwrap().1;
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKID/20261124/auto/s3/aws4_request, "));
        assert!(auth.ends_with("Signature=6849f36290a1e03e6f150d17b6e2d94e704414a13e13a2dd1a5492c2006d1ebf"), "{auth}");
    }

    /// RFC 4231's test case 6: a key longer than the block.
    #[test]
    fn hmac_matches_rfc_4231() {
        let key = [0xaau8; 131];
        let got = hex(&hmac(&key, "Test Using Larger Than Block-Size Key - Hash Key First"));
        assert_eq!(got, "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
        assert_eq!(
            hex(&hmac(b"Jefe", "what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn dates_cross_years_and_leap_days() {
        assert_eq!(amz_date(0), "19700101T000000Z");
        assert_eq!(amz_date(951_782_400_000), "20000229T000000Z");
        assert_eq!(amz_date(1_704_067_199_000), "20231231T235959Z");
    }

    #[test]
    fn a_list_answer_gives_its_keys_in_order_and_its_truncation() {
        let xml = "<ListBucketResult><IsTruncated>true</IsTruncated><Contents><Key>export/000000000100/0000000001</Key>\
                   </Contents><Contents><Key>export/000000000100/0000000002</Key></Contents></ListBucketResult>";
        let (keys, more) = listed(xml);
        assert_eq!(keys, ["export/000000000100/0000000001", "export/000000000100/0000000002"]);
        assert!(more);
        assert_eq!(listed("<ListBucketResult><IsTruncated>false</IsTruncated></ListBucketResult>"), (vec![], false));
    }
}
