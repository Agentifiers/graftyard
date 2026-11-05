//! Hostnames and endpoints (`design/SHARED.md`, "Hostnames and endpoints"): the install host's validation, the URLs
//! every surface names, and which surface a request's host and path address.

/// The install's host: a bare DNS name every client classifies as GHES, served on port 443.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    name: String,
    /// Subdomain isolation: `raw.`, `codeload.`, `uploads.`, and `objects.` under the host. On in managed, optional
    /// elsewhere.
    isolation: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostError {
    /// Not a lower-case DNS name of labels of `[a-z0-9-]`: a port, a path, or a scheme included.
    NotAHostname,
    /// Is or ends in `.github.com`, `.ghe.com`, or `.localhost`, which clients classify as dotcom, GHE.com, or
    /// `github.localhost` (`research/clients.md` #1 and #2).
    NotGhes,
}

impl Host {
    /// Validated at install time.
    pub fn new(name: &str, isolation: bool) -> Result<Host, HostError> {
        let labels: Vec<&str> = name.split('.').collect();
        let label_ok = |l: &&str| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        if name.len() > 253 || !labels.iter().all(label_ok) {
            return Err(HostError::NotAHostname);
        }
        // Whole labels only: clients classify `notgithub.com` as GHES (`research/clients.md` #2).
        if ["github.com", "ghe.com", "localhost"].iter().any(|s| name == *s || name.ends_with(&format!(".{s}"))) {
            return Err(HostError::NotGhes);
        }
        if labels.len() < 2 {
            return Err(HostError::NotAHostname);
        }
        Ok(Host { name: name.to_owned(), isolation })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn isolation(&self) -> bool {
        self.isolation
    }

    pub fn web(&self) -> String {
        format!("https://{}", self.name)
    }

    pub fn api(&self) -> String {
        format!("https://{}/api/v3", self.name)
    }

    pub fn graphql(&self) -> String {
        format!("https://{}/api/graphql", self.name)
    }

    /// The origin of `upload_url`, which gh and Octokit take from the release (`ghes-3.22.json`'s upload server).
    pub fn uploads(&self) -> String {
        if self.isolation {
            format!("https://uploads.{}", self.name)
        } else {
            format!("https://{}/api/uploads", self.name)
        }
    }

    /// Where `raw-content` lives: `raw.HOST/o/r/{ref}/{path}` with isolation, `HOST/o/r/raw/{ref}/{path}` without.
    pub fn raw(&self, owner: &str, repo: &str) -> String {
        if self.isolation {
            format!("https://raw.{}/{owner}/{repo}", self.name)
        } else {
            format!("https://{}/{owner}/{repo}/raw", self.name)
        }
    }

    /// Where `codeload-archives` lives: `codeload.HOST/o/r/...` with isolation, `HOST/_codeload/o/r/...` without.
    pub fn codeload(&self, owner: &str, repo: &str) -> String {
        if self.isolation {
            format!("https://codeload.{}/{owner}/{repo}", self.name)
        } else {
            format!("https://{}/_codeload/{owner}/{repo}", self.name)
        }
    }

    /// Where LFS transfer actions point. The bytes come from the edge, never a presigned bucket URL, so a bucket
    /// fault on one site never reaches the client [inference: SHARED's "presigned URLs to `R2Key::Blob` objects"
    /// needs an R2 S3 credential on the request path, and a presigned URL names one bucket].
    pub fn objects(&self) -> String {
        if self.isolation { format!("https://objects.{}", self.name) } else { format!("https://{}/_lfs", self.name) }
    }

    /// The surface `authority` and `path` address, or `None` for another host.
    pub fn endpoint<'p>(&self, authority: &str, path: &'p str) -> Option<Endpoint<'p>> {
        let sub = match authority.strip_suffix(self.name.as_str()) {
            Some("") => None,
            Some(s) if self.isolation => Some(s.strip_suffix('.')?),
            _ => return None,
        };
        match sub {
            None => Some(Endpoint::bare(path, self.isolation)),
            Some("raw") => repo_path(path).map(|(owner, repo, rest)| Endpoint::Raw { owner, repo, rest }),
            Some("codeload") => repo_path(path).map(|(owner, repo, rest)| Endpoint::Codeload { owner, repo, rest }),
            Some("uploads") => Some(Endpoint::Uploads(path)),
            Some("objects") => Some(Endpoint::Objects(path.strip_prefix('/').unwrap_or(path))),
            Some(_) => None,
        }
    }
}

/// The surface a request addresses. Paths keep their leading `/` where they continue a URL prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint<'p> {
    /// `GET /status`, Dependabot's probe.
    Status,
    /// `/api/v3` and everything under it. `""` is the root document.
    Rest(&'p str),
    Graphql,
    /// `/api/uploads` and everything under it.
    Uploads(&'p str),
    /// `/login/oauth/*` and `/login/device/code`.
    OAuth(&'p str),
    /// LFS transfer bytes: `/_lfs/{repo}/{oid}`.
    Objects(&'p str),
    /// `o/r[.git]/info/lfs/...`.
    Lfs {
        owner: &'p str,
        repo: &'p str,
        rest: &'p str,
    },
    Raw {
        owner: &'p str,
        repo: &'p str,
        rest: &'p str,
    },
    Codeload {
        owner: &'p str,
        repo: &'p str,
        rest: &'p str,
    },
    /// Git smart HTTP and the web paths under a repository (`releases/download/...`, `archive/...`), for
    /// `graftyard_git::smart::route` and the web routes.
    Repo {
        owner: &'p str,
        repo: &'p str,
        rest: &'p str,
    },
    /// Anything else on the bare host: web pages, which are not this package's.
    Web(&'p str),
}

fn repo_path(path: &str) -> Option<(&str, &str, &str)> {
    let p = path.strip_prefix('/')?;
    let (owner, rest) = p.split_once('/')?;
    let (repo, rest) = rest.split_once('/').map_or((rest, ""), |(r, x)| (r, x));
    (!owner.is_empty() && !repo.is_empty()).then_some((owner, repo, rest))
}

impl<'p> Endpoint<'p> {
    fn bare(path: &'p str, isolation: bool) -> Endpoint<'p> {
        if path == "/status" {
            return Endpoint::Status;
        }
        if path == "/api/graphql" {
            return Endpoint::Graphql;
        }
        if let Some(rest) = path.strip_prefix("/api/v3").filter(|r| r.is_empty() || r.starts_with('/')) {
            return Endpoint::Rest(rest);
        }
        if let Some(rest) = path.strip_prefix("/api/uploads").filter(|r| r.is_empty() || r.starts_with('/')) {
            return Endpoint::Uploads(rest);
        }
        if path.starts_with("/login/oauth/") || path == "/login/device/code" {
            return Endpoint::OAuth(path);
        }
        // With isolation on, the old path prefixes are not served (`research/wire-census.md` #33).
        if !isolation {
            if let Some(rest) = path.strip_prefix("/_lfs/") {
                return Endpoint::Objects(rest);
            }
            if let Some((owner, repo, rest)) = path.strip_prefix("/_codeload").and_then(repo_path) {
                return Endpoint::Codeload { owner, repo, rest };
            }
        }
        let Some((owner, repo, rest)) = repo_path(path) else { return Endpoint::Web(path) };
        if let Some(lfs) = rest.strip_prefix("info/lfs/") {
            let repo = repo.strip_suffix(".git").unwrap_or(repo);
            return Endpoint::Lfs { owner, repo, rest: lfs };
        }
        if let Some(raw) = rest.strip_prefix("raw/").filter(|_| !isolation) {
            return Endpoint::Raw { owner, repo, rest: raw };
        }
        Endpoint::Repo { owner, repo, rest }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_clients_classify_as_dotcom_are_refused() {
        for bad in ["github.com", "git.github.com", "acme.ghe.com", "localhost", "github.localhost", "x.localhost"] {
            assert_eq!(Host::new(bad, false), Err(HostError::NotGhes), "{bad}");
        }
        for bad in ["ghes.example.com:8443", "https://ghes.example.com", "ghes.example.com/x", "GHES.example.com", "a"]
        {
            assert_eq!(Host::new(bad, false), Err(HostError::NotAHostname), "{bad}");
        }
        assert!(Host::new("github.example.com", false).is_ok(), "a `github.` label is fine");
        for ok in ["notgithub.com", "acme-ghe.com", "mylocalhost.example", "github.com.example.org"] {
            assert!(Host::new(ok, false).is_ok(), "{ok} is GHES to every client");
        }
        assert!(Host::new("graftyard.dev", true).is_ok());
    }

    #[test]
    fn the_bare_host_serves_every_surface_on_one_origin() {
        let h = Host::new("ghes.example.com", false).unwrap();
        let at = |p| h.endpoint("ghes.example.com", p);
        assert_eq!(at("/api/v3"), Some(Endpoint::Rest("")));
        assert_eq!(at("/api/v3/meta"), Some(Endpoint::Rest("/meta")));
        assert_eq!(at("/api/v3x"), Some(Endpoint::Repo { owner: "api", repo: "v3x", rest: "" }));
        assert_eq!(at("/api/graphql"), Some(Endpoint::Graphql));
        assert_eq!(
            at("/api/uploads/repos/o/r/releases/1/assets"),
            Some(Endpoint::Uploads("/repos/o/r/releases/1/assets"))
        );
        assert_eq!(at("/login/device/code"), Some(Endpoint::OAuth("/login/device/code")));
        assert_eq!(at("/status"), Some(Endpoint::Status));
        assert_eq!(
            at("/o/r.git/info/lfs/objects/batch"),
            Some(Endpoint::Lfs { owner: "o", repo: "r", rest: "objects/batch" })
        );
        assert_eq!(at("/o/r/info/lfs/locks"), Some(Endpoint::Lfs { owner: "o", repo: "r", rest: "locks" }));
        assert_eq!(
            at("/o/r/raw/main/README.md"),
            Some(Endpoint::Raw { owner: "o", repo: "r", rest: "main/README.md" })
        );
        assert_eq!(
            at("/_codeload/o/r/legacy.tar.gz/refs/tags/v1"),
            Some(Endpoint::Codeload { owner: "o", repo: "r", rest: "legacy.tar.gz/refs/tags/v1" })
        );
        assert_eq!(at("/_lfs/7/abc"), Some(Endpoint::Objects("7/abc")));
        assert_eq!(at("/o/r.git/info/refs"), Some(Endpoint::Repo { owner: "o", repo: "r.git", rest: "info/refs" }));
        assert_eq!(at("/"), Some(Endpoint::Web("/")));
        assert_eq!(h.endpoint("raw.ghes.example.com", "/o/r/main/x"), None, "no subdomains without isolation");
        assert_eq!(h.endpoint("evil.com", "/api/v3"), None);
        assert_eq!(h.uploads(), "https://ghes.example.com/api/uploads");
    }

    #[test]
    fn isolation_moves_raw_codeload_uploads_and_objects_to_subdomains() {
        let h = Host::new("graftyard.dev", true).unwrap();
        assert_eq!(
            h.endpoint("raw.graftyard.dev", "/o/r/main/README.md"),
            Some(Endpoint::Raw { owner: "o", repo: "r", rest: "main/README.md" })
        );
        assert_eq!(
            h.endpoint("codeload.graftyard.dev", "/o/r/legacy.zip/refs/tags/v1"),
            Some(Endpoint::Codeload { owner: "o", repo: "r", rest: "legacy.zip/refs/tags/v1" })
        );
        assert_eq!(
            h.endpoint("uploads.graftyard.dev", "/repos/o/r/releases/1/assets"),
            Some(Endpoint::Uploads("/repos/o/r/releases/1/assets"))
        );
        assert_eq!(h.endpoint("objects.graftyard.dev", "/7/abc"), Some(Endpoint::Objects("7/abc")));
        assert_eq!(
            h.endpoint("graftyard.dev", "/_codeload/o/r/x"),
            Some(Endpoint::Repo { owner: "_codeload", repo: "o", rest: "r/x" })
        );
        assert_eq!(h.endpoint("x.graftyard.dev", "/"), None);
        assert_eq!(h.uploads(), "https://uploads.graftyard.dev");
        assert_eq!(h.codeload("o", "r"), "https://codeload.graftyard.dev/o/r");
    }
}
