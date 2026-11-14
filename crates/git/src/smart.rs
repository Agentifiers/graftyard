//! Smart HTTP (`design/SHARED.md`, "Surfaces, tiers, and oracles", `git-http` and `git-protocol-v2`;
//! `research/wire-census.md`, section C): routes, the receive-pack and upload-pack advertisements, the receive-pack
//! request, `report_status`, and the refusals. Pure: the edge Worker reads and writes the bytes.

use graftyard_raft::Acked;
use thiserror::Error;

use crate::pack::INLINE_MAX;
use crate::pkt::{self, Band, Pkt, PktError, Reader};
use crate::refs::{ObjectId, RefName, RefUpdate};
use crate::repo::{ATOMIC_REFS_MAX, PushOut, RefOutcome, RejectReason};

/// What a git URL asks for. github.com serves these with and without `.git`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    InfoRefs {
        owner: String,
        repo: String,
        service: Option<Service>,
    },
    Rpc {
        owner: String,
        repo: String,
        service: Service,
    },
    /// Any other path under a repository: dumb HTTP (`/HEAD`, `/objects/...`), which is refused with 404.
    Dumb {
        owner: String,
        repo: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    UploadPack,
    ReceivePack,
}

impl Service {
    pub fn name(self) -> &'static str {
        match self {
            Service::UploadPack => "git-upload-pack",
            Service::ReceivePack => "git-receive-pack",
        }
    }

    fn parse(s: &str) -> Option<Service> {
        match s {
            "git-upload-pack" => Some(Service::UploadPack),
            "git-receive-pack" => Some(Service::ReceivePack),
            _ => None,
        }
    }

    pub fn advertisement_type(self) -> String {
        format!("application/x-{}-advertisement", self.name())
    }

    pub fn result_type(self) -> String {
        format!("application/x-{}-result", self.name())
    }
}

/// `query` is the URL's query string without `?`. `None` when the path is not a repository path.
pub fn route(path: &str, query: &str) -> Option<Route> {
    let p = path.strip_prefix('/')?;
    let mut parts = p.splitn(3, '/');
    let owner = parts.next().filter(|s| !s.is_empty())?.to_owned();
    let repo = parts.next().filter(|s| !s.is_empty())?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo).to_owned();
    let rest = parts.next().unwrap_or("");
    // No service, or an unknown one, is dumb HTTP's info/refs: refused the same way.
    let service = query.split('&').find_map(|kv| kv.strip_prefix("service=")).and_then(Service::parse);
    Some(match rest {
        "info/refs" => Route::InfoRefs { owner, repo, service },
        "git-upload-pack" => Route::Rpc { owner, repo, service: Service::UploadPack },
        "git-receive-pack" => Route::Rpc { owner, repo, service: Service::ReceivePack },
        _ => Route::Dumb { owner, repo },
    })
}

/// Whether the client asked for protocol v2 in its `Git-Protocol` header (`version=2`, colon-separated).
pub fn wants_v2(git_protocol: Option<&str>) -> bool {
    git_protocol.is_some_and(|h| h.split(':').any(|kv| kv.trim() == "version=2"))
}

/// github.com's protocol v2 capabilities after `version 2` and `agent` (`research/wire-census.md#41`). `bundle-uri`,
/// `packfile-uris`, `object-info`, and `session-id` stay off until T2.
pub const V2_CAPS: [&str; 4] =
    ["ls-refs=unborn", "fetch=shallow wait-for-done filter", "server-option", "object-format=sha1"];

/// The upload-pack advertisement in protocol v2, which needs no refs: the edge answers it without the shard. Over HTTP
/// v2 has no `# service=` preamble.
pub fn upload_pack_v2_advertisement(agent: &str) -> Vec<u8> {
    let mut out = Vec::new();
    pkt::line(&mut out, "version 2");
    pkt::line(&mut out, &format!("agent={agent}"));
    for c in V2_CAPS {
        pkt::line(&mut out, c);
    }
    pkt::flush(&mut out);
    out
}

/// github.com's receive-pack capabilities (`research/wire-census.md#44`), with this host's agent and session id.
pub fn receive_pack_caps(agent: &str, session: &str) -> String {
    format!(
        "report-status report-status-v2 delete-refs side-band-64k ofs-delta atomic object-format=sha1 quiet \
         agent={agent} session-id={session} push-options"
    )
}

/// `# service=` preamble, then refs sorted by name with the capabilities on the first line (or on
/// `capabilities^{}` for an empty repository), then a flush.
pub fn receive_pack_advertisement(refs: &[(RefName, ObjectId)], caps: &str) -> Vec<u8> {
    let mut sorted: Vec<(RefName, ObjectId)> = refs.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = receive_pack_head();
    out.extend(receive_pack_lines(&sorted, caps, true));
    out.extend(receive_pack_tail());
    out
}

/// The advertisement's preamble: `# service=git-receive-pack` and a flush.
pub fn receive_pack_head() -> Vec<u8> {
    let mut out = Vec::new();
    pkt::line(&mut out, "# service=git-receive-pack");
    pkt::flush(&mut out);
    out
}

/// One run of the advertisement's ref lines, `refs` already in name order (a page of a paged ref read, which is). The
/// capabilities ride on the first line of the run that is `first`, and an empty first run is `capabilities^{}`, so the
/// runs of every page in order, between `receive_pack_head` and `receive_pack_tail`, are the whole advertisement.
pub fn receive_pack_lines(refs: &[(RefName, ObjectId)], caps: &str, first: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let line = |out: &mut Vec<u8>, oid: &ObjectId, name: &[u8], first: bool| {
        let mut p = Vec::with_capacity(48 + name.len() + caps.len());
        p.extend_from_slice(oid.hex().as_bytes());
        p.push(b' ');
        p.extend_from_slice(name);
        if first {
            p.push(0);
            p.extend_from_slice(caps.as_bytes());
        }
        p.push(b'\n');
        pkt::data(out, &p);
    };
    let mut first = first;
    for (name, oid) in refs {
        line(&mut out, oid, name.as_bytes(), first);
        first = false;
    }
    if first {
        line(&mut out, &ObjectId::ZERO, b"capabilities^{}", true);
    }
    out
}

/// The advertisement's closing flush.
pub fn receive_pack_tail() -> Vec<u8> {
    let mut out = Vec::new();
    pkt::flush(&mut out);
    out
}

/// The capabilities a receive-pack request names that shape the reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PushCaps {
    pub report_status: bool,
    pub report_status_v2: bool,
    pub side_band_64k: bool,
    pub atomic: bool,
    pub quiet: bool,
    pub push_options: bool,
    pub agent: Option<String>,
}

impl PushCaps {
    fn parse(s: &[u8]) -> PushCaps {
        let mut c = PushCaps::default();
        for w in String::from_utf8_lossy(s).split(' ') {
            match w {
                "report-status" => c.report_status = true,
                "report-status-v2" => c.report_status_v2 = true,
                "side-band-64k" => c.side_band_64k = true,
                "atomic" => c.atomic = true,
                "quiet" => c.quiet = true,
                "push-options" => c.push_options = true,
                _ => {
                    if let Some(a) = w.strip_prefix("agent=") {
                        c.agent = Some(a.to_owned());
                    }
                }
            }
        }
        c
    }

    fn band(&self) -> Option<Band> {
        self.side_band_64k.then_some(Band::Large)
    }
}

/// A parsed receive-pack request: the commands, the capabilities on the first one, the push options, and the pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceivePack {
    pub updates: Vec<RefUpdate>,
    pub caps: PushCaps,
    pub options: Vec<String>,
    /// Byte offset of the pack in the body. Empty when every command is a delete.
    pub pack_at: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum RequestError {
    #[error(transparent)]
    Pkt(#[from] PktError),
    #[error("no commands")]
    Empty,
    #[error("bad command line {0:?}")]
    Command(String),
    #[error("bad ref name {0:?}")]
    RefName(String),
    #[error("shallow pushes are not supported")]
    Shallow,
    #[error("push certificates are not supported")]
    PushCert,
    #[error("the request ends before its flush")]
    Unterminated,
}

fn command(line: &[u8]) -> Result<RefUpdate, RequestError> {
    let bad = || RequestError::Command(String::from_utf8_lossy(line).into_owned());
    if line.len() < 83 || line[40] != b' ' || line[81] != b' ' {
        return Err(bad());
    }
    let old = ObjectId::parse_hex(&line[..40]).ok_or_else(bad)?;
    let new = ObjectId::parse_hex(&line[41..81]).ok_or_else(bad)?;
    let name = &line[82..];
    let name = RefName::parse(name).map_err(|_| RequestError::RefName(String::from_utf8_lossy(name).into_owned()))?;
    Ok(RefUpdate { name, expect: old.non_zero(), new: new.non_zero() })
}

/// Parses as much of the body as holds the commands and options. A body cut short inside the pack still parses,
/// which the cap refusal needs: it names every ref of a push it did not read whole.
pub fn parse_receive_pack(body: &[u8]) -> Result<ReceivePack, RequestError> {
    let mut r = Reader::new(body);
    let mut updates = Vec::new();
    let mut caps = PushCaps::default();
    loop {
        match r.next_pkt()? {
            None => return Err(RequestError::Unterminated),
            Some(Pkt::Flush) => break,
            Some(p @ Pkt::Data(_)) => {
                let line = p.text().expect("data");
                if line.starts_with(b"shallow ") {
                    return Err(RequestError::Shallow);
                }
                if line.starts_with(b"push-cert") {
                    return Err(RequestError::PushCert);
                }
                let line = if updates.is_empty() {
                    match line.iter().position(|b| *b == 0) {
                        Some(i) => {
                            caps = PushCaps::parse(&line[i + 1..]);
                            &line[..i]
                        }
                        None => line,
                    }
                } else {
                    line
                };
                updates.push(command(line)?);
            }
            Some(_) => return Err(RequestError::Command("special packet".into())),
        }
    }
    if updates.is_empty() {
        return Err(RequestError::Empty);
    }
    let mut options = Vec::new();
    if caps.push_options {
        loop {
            match r.next_pkt()? {
                None => return Err(RequestError::Unterminated),
                Some(Pkt::Flush) => break,
                Some(p @ Pkt::Data(_)) => options.push(String::from_utf8_lossy(p.text().expect("data")).into_owned()),
                Some(_) => return Err(RequestError::Command("special packet in push options".into())),
            }
        }
    }
    Ok(ReceivePack { updates, caps, options, pack_at: r.pos() })
}

/// How many of a body's first bytes the cap refusal parses, as it always has: the commands it names come from them.
pub const CAP_HEAD: usize = 64 << 10;

/// The edge's read of a receive-pack body ahead of its pack (`design/DESIGN.md`, journey 1, step 2). It holds the
/// body while it may still be one the edge buffers whole: until the commands are read and more than `INLINE_MAX` of
/// pack follows them, which no inline pack has. Then the rest streams (`Read::Stream`). A body whose commands do not
/// parse is held to `INLINE_MAX`, then only counted, so it is answered as if it had been held whole. Commands that
/// have not ended are held, up to the cap, as the edge held every body: a refusal names each of their refs. Every byte
/// counts against the cap, and the first `CAP_HEAD` are kept for the cap refusal.
pub struct BodyReader {
    cap: u64,
    n: u64,
    buf: Vec<u8>,
    first: Vec<u8>,
    /// Parse again once `buf` is this long: twice its length at the last try, so the parses cost at most twice the
    /// commands' bytes.
    next_try: usize,
    head: Option<ReceivePack>,
    bad: Option<RequestError>,
}

/// What `BodyReader::push` decides after a chunk.
#[derive(Debug, PartialEq, Eq)]
pub enum Read {
    More,
    /// The body passed the cap: answer `cap_refusal` against `BodyReader::cap_head`.
    OverCap,
    /// The commands are read and more than `INLINE_MAX` of pack follows: the rest of the body streams.
    Stream,
}

/// A body that ended without passing the cap or streaming.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// All of it, as the edge has always read it.
    Whole(Vec<u8>),
    /// Its commands do not parse, and it was too long to hold: the error a parse of the whole body gives.
    Bad(RequestError),
}

/// A body that streams: the pack bytes read so far, and the body's first `CAP_HEAD` bytes.
pub struct Streaming {
    pub pack: Vec<u8>,
    pub cap_head: Vec<u8>,
    /// Bytes of the body read so far.
    pub read: u64,
}

impl BodyReader {
    pub fn new(cap: u64) -> BodyReader {
        BodyReader { cap, n: 0, buf: Vec::new(), first: Vec::new(), next_try: 0, head: None, bad: None }
    }

    pub fn push(&mut self, c: &[u8]) -> Read {
        self.n += c.len() as u64;
        let keep = (CAP_HEAD - self.first.len()).min(c.len());
        self.first.extend_from_slice(&c[..keep]);
        if self.n > self.cap {
            return Read::OverCap;
        }
        if self.bad.is_some() {
            return Read::More;
        }
        self.buf.extend_from_slice(c);
        if self.head.is_none() && self.buf.len() >= self.next_try {
            match parse_receive_pack(&self.buf) {
                Ok(rp) => self.head = Some(rp),
                Err(RequestError::Unterminated | RequestError::Pkt(PktError::Truncated(_))) => {
                    self.next_try = self.buf.len() * 2;
                }
                // Any other error is the whole body's too, but a short body is still held whole, so the probe's bare
                // flush and every short body read as before.
                Err(e) if self.buf.len() > INLINE_MAX => {
                    self.bad = Some(e);
                    self.buf = Vec::new();
                }
                Err(_) => self.next_try = INLINE_MAX + 1,
            }
        }
        match &self.head {
            Some(rp) if self.buf.len() - rp.pack_at > INLINE_MAX => Read::Stream,
            _ => Read::More,
        }
    }

    /// A reader that holds the whole body whatever it holds: a gzip body, which the edge inflates only once it ended.
    pub fn whole(cap: u64) -> BodyReader {
        BodyReader { next_try: usize::MAX, ..BodyReader::new(cap) }
    }

    /// The body's first `CAP_HEAD` bytes.
    pub fn cap_head(&self) -> &[u8] {
        &self.first
    }

    /// The body after `Read::Stream`: its commands, and what it read past them.
    pub fn streaming(mut self) -> (ReceivePack, Streaming) {
        let rp = self.head.expect("Read::Stream comes only after the commands parsed");
        let pack = self.buf.split_off(rp.pack_at);
        (rp, Streaming { pack, cap_head: self.first, read: self.n })
    }

    /// The body once it ended.
    pub fn end(self) -> Ended {
        match self.bad {
            Some(e) => Ended::Bad(e),
            None => Ended::Whole(self.buf),
        }
    }
}

fn reason(r: &RejectReason) -> &str {
    match r {
        RejectReason::Stale => "fetch first",
        RejectReason::NonFastForward => "non-fast-forward",
        RejectReason::Hidden => "deny updating a hidden ref",
        RejectReason::TooManyRefs => "push has too many ref updates, split it into smaller pushes",
        RejectReason::Atomic => "atomic push failure",
        // receive-pack's text for REF_TRANSACTION_ERROR_NAME_CONFLICT (refs.c, `ref_transaction_error_msg`).
        RejectReason::NameConflict => "refname conflict",
        RejectReason::AtomicTransaction => "atomic transaction failed",
        RejectReason::Agent(a) => &a.text,
    }
}

/// The reply body for the client's capabilities: `remote:` lines on band 2 and the status on band 1 with side-band-64k,
/// else the status alone.
pub fn wrap(caps: &PushCaps, status: &[u8], remote: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    match caps.band() {
        Some(band) => {
            for m in remote {
                pkt::sideband(&mut out, band, 2, format!("{m}\n").as_bytes());
            }
            if caps.report_status || caps.report_status_v2 {
                pkt::sideband(&mut out, band, 1, status);
            }
            pkt::flush(&mut out);
        }
        None => {
            if caps.report_status || caps.report_status_v2 {
                out.extend_from_slice(status);
            }
        }
    }
    out
}

fn status(unpack: &str, refs: &[(&RefName, Result<(), &str>)]) -> Vec<u8> {
    let mut s = Vec::new();
    pkt::line(&mut s, &format!("unpack {unpack}"));
    for (name, r) in refs {
        let mut p = Vec::new();
        match r {
            Ok(()) => p.extend_from_slice(b"ok "),
            Err(_) => p.extend_from_slice(b"ng "),
        }
        p.extend_from_slice(name.as_bytes());
        if let Err(why) = r {
            p.push(b' ');
            p.extend_from_slice(why.as_bytes());
        }
        p.push(b'\n');
        pkt::data(&mut s, &p);
    }
    pkt::flush(&mut s);
    s
}

/// The only renderer of `ok`: git's report-status section for a push whose entry committed, one line per ref in
/// the push's order. `Acked` is built only on the leader's host and cannot cross a binding, so the member host calls
/// this and sends the bytes; the edge wraps them with `wrap` and adds nothing.
/// A committed output that may be a push's: `report_status` renders only those.
pub trait PushOutOf {
    fn push_out(&self) -> Option<&PushOut>;
}

impl PushOutOf for PushOut {
    fn push_out(&self) -> Option<&PushOut> {
        Some(self)
    }
}

pub fn report_status<T: PushOutOf>(acked: &Acked<T>) -> Option<Vec<u8>> {
    let refs: Vec<(&RefName, Result<(), &str>)> = acked
        .out
        .push_out()?
        .refs
        .iter()
        .map(|(n, o)| {
            (
                n,
                match o {
                    RefOutcome::Ok => Ok(()),
                    RefOutcome::Rejected(r) => Err(reason(r)),
                },
            )
        })
        .collect();
    Some(status("ok", &refs))
}

/// A refusal of the whole push before or at `commit`: `ng` on every ref, never `ok`, and `remote:` lines that say why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub unpack: String,
    pub reason: String,
    pub remote: Vec<String>,
}

impl Refusal {
    pub fn render(&self, req: &ReceivePack) -> Vec<u8> {
        let refs: Vec<(&RefName, Result<(), &str>)> =
            req.updates.iter().map(|u| (&u.name, Err(self.reason.as_str()))).collect();
        wrap(&req.caps, &status(&self.unpack, &refs), &self.remote)
    }

    /// The pack did not index or did not connect.
    pub fn unpack_failed(why: &str) -> Refusal {
        Refusal { unpack: one_line(why), reason: "unpacker error".into(), remote: vec![] }
    }

    /// An atomic push over `ATOMIC_REFS_MAX`, refused whole with a message that names the limit.
    pub fn too_many_atomic(got: usize) -> Refusal {
        Refusal {
            unpack: "ok".into(),
            reason: format!("atomic push over {ATOMIC_REFS_MAX} ref updates"),
            remote: vec![format!(
                "Push refused: an --atomic push may update at most {ATOMIC_REFS_MAX} refs, and this one updates {got}. \
                 Split it into smaller pushes."
            )],
        }
    }

    /// Ref updates whose encoding alone is over `COMMAND_MAX` (`design/SHARED.md`, "Refs and pushes", Size).
    pub fn updates_too_large(len: usize, max: usize, refs: usize) -> Refusal {
        Refusal {
            unpack: "ok".into(),
            reason: reason(&RejectReason::TooManyRefs).into(),
            remote: vec![format!(
                "Push refused: its {refs} ref updates take {len} bytes, over this host's {max}-byte limit for one push. \
                 Split it into smaller pushes."
            )],
        }
    }

    /// `decide` refused the whole command, for example a revoked token.
    pub fn refused(why: &str) -> Refusal {
        Refusal { unpack: "ok".into(), reason: one_line(why), remote: vec![] }
    }

    /// Agent admission is full (`graftyard_raft::admit`): HTTP 200 with `ng <ref> agent admission full, retry after
    /// <n>s` on every ref and a `remote:` line (`design/DESIGN.md`, Shape, "Agent load and agent safety").
    pub fn agent_full(retry_after_s: u32) -> Refusal {
        Refusal {
            unpack: "ok".into(),
            reason: format!("agent admission full, retry after {retry_after_s}s"),
            remote: vec![format!(
                "This repository is admitting agent writes at its agent share. Retry this push in {retry_after_s} seconds."
            )],
        }
    }
}

fn one_line(s: &str) -> String {
    s.split(['\n', '\r']).next().unwrap_or("").chars().take(200).collect()
}

/// The install's Cloudflare zone plan, which sets the request body limit and whether native SSH exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    Pro,
    Business,
    Enterprise,
}

impl Plan {
    /// The plan's request body limit, in bytes: 100 MB on Free and Pro, 200 MB on Business, and up to 5 GB on
    /// Enterprise (`research/cloudflare.md`, Workers).
    pub fn body_limit(self) -> u64 {
        match self {
            Plan::Pro => 100_000_000,
            Plan::Business => 200_000_000,
            Plan::Enterprise => 5_000_000_000,
        }
    }
}

/// How the edge's cap refusal reads (`design/DESIGN.md`, journey 1, step 2): HTTPS stays the default it names. On
/// Enterprise it points to SSH; on Pro and Business it points to the `cloudflared` SSH option when the install has it
/// on, and otherwise to Enterprise or splitting the push.
pub fn cap_refusal(plan: Plan, ssh_option: bool, cap: u64, host: &str, path: &str) -> Refusal {
    let mb = cap / 1_000_000;
    let head = format!("Push refused: over {mb} MB, this install's limit for one push over HTTPS.");
    let keep = "HTTPS stays the default for every other push.";
    let ssh = format!("git push git@{host}:{path}.git");
    let (how, reason) = match (plan, ssh_option) {
        (Plan::Enterprise, _) => (format!("Push this one over SSH: {ssh}"), "push over HTTPS cap, use SSH"),
        (_, true) => (
            format!(
                "Push this one over SSH through cloudflared (see the install's SSH docs: ProxyCommand cloudflared \
                 access ssh --hostname %h): {ssh}"
            ),
            "push over HTTPS cap, use SSH",
        ),
        (_, false) => (
            "Split it into smaller pushes, or ask your admin about the Enterprise plan, which takes larger pushes \
             and SSH."
                .into(),
            "push over HTTPS cap, split the push",
        ),
    };
    Refusal { unpack: format!("push over {mb} MB"), reason: reason.into(), remote: vec![head, how, keep.into()] }
}

/// A protocol v2 upload-pack request's command and arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct V2Request {
    pub command: String,
    /// Capability lines before the delimiter, such as `agent=` and `object-format=`.
    pub caps: Vec<String>,
    pub args: Vec<Vec<u8>>,
}

pub fn parse_v2(body: &[u8]) -> Result<V2Request, RequestError> {
    let mut r = Reader::new(body);
    let mut command = None;
    let mut caps = Vec::new();
    let mut args = Vec::new();
    let mut in_args = false;
    loop {
        match r.next_pkt()? {
            None => return Err(RequestError::Unterminated),
            Some(Pkt::Flush) => break,
            Some(Pkt::Delim) if !in_args => in_args = true,
            Some(p @ Pkt::Data(_)) => {
                let t = p.text().expect("data");
                if in_args {
                    args.push(t.to_vec());
                } else if let Some(c) = t.strip_prefix(b"command=") {
                    command = Some(String::from_utf8_lossy(c).into_owned());
                } else {
                    caps.push(String::from_utf8_lossy(t).into_owned());
                }
            }
            Some(_) => return Err(RequestError::Command("special packet".into())),
        }
    }
    Ok(V2Request { command: command.ok_or(RequestError::Empty)?, caps, args })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(c: u8) -> ObjectId {
        ObjectId([c; 20])
    }

    fn cmd(old: ObjectId, new: ObjectId, name: &str) -> Vec<u8> {
        format!("{} {} {name}", old.hex(), new.hex()).into_bytes()
    }

    fn request(cmds: &[Vec<u8>], caps: &str, options: &[&str], pack: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        for (i, c) in cmds.iter().enumerate() {
            let mut p = c.clone();
            if i == 0 {
                p.push(0);
                p.extend_from_slice(caps.as_bytes());
            }
            p.push(b'\n');
            pkt::data(&mut b, &p);
        }
        pkt::flush(&mut b);
        if caps.contains("push-options") {
            for o in options {
                pkt::line(&mut b, o);
            }
            pkt::flush(&mut b);
        }
        b.extend_from_slice(pack);
        b
    }

    #[test]
    fn routes_follow_github_with_and_without_dot_git() {
        assert_eq!(
            route("/o/r.git/info/refs", "service=git-upload-pack"),
            Some(Route::InfoRefs { owner: "o".into(), repo: "r".into(), service: Some(Service::UploadPack) })
        );
        assert_eq!(
            route("/o/r/git-receive-pack", ""),
            Some(Route::Rpc { owner: "o".into(), repo: "r".into(), service: Service::ReceivePack })
        );
        // Dumb HTTP: info/refs with no service (403) and object paths (404).
        assert_eq!(
            route("/o/r.git/info/refs", ""),
            Some(Route::InfoRefs { owner: "o".into(), repo: "r".into(), service: None })
        );
        assert_eq!(route("/o/r.git/HEAD", ""), Some(Route::Dumb { owner: "o".into(), repo: "r".into() }));
        assert_eq!(route("/o/r.git/objects/info/packs", "").map(|r| matches!(r, Route::Dumb { .. })), Some(true));
        assert_eq!(
            route("/o/r.git/info/refs", "service=git-annex"),
            Some(Route::InfoRefs { owner: "o".into(), repo: "r".into(), service: None })
        );
        assert_eq!(route("/", ""), None);
        assert_eq!(route("/o", ""), None);
    }

    #[test]
    fn v2_advertisement_is_githubs_set_exactly() {
        let a = upload_pack_v2_advertisement("git/graftyard");
        let mut r = Reader::new(&a);
        let mut lines = Vec::new();
        while let Some(p) = r.next_pkt().unwrap() {
            match p.text() {
                Some(t) => lines.push(String::from_utf8(t.to_vec()).unwrap()),
                None => lines.push("<flush>".into()),
            }
        }
        // research/wire-census.md#41, the agent replaced.
        assert_eq!(
            lines,
            [
                "version 2",
                "agent=git/graftyard",
                "ls-refs=unborn",
                "fetch=shallow wait-for-done filter",
                "server-option",
                "object-format=sha1",
                "<flush>"
            ]
        );
        assert!(wants_v2(Some("version=2")));
        assert!(wants_v2(Some("foo=bar:version=2")));
        assert!(!wants_v2(Some("version=1")));
        assert!(!wants_v2(None));
    }

    #[test]
    fn receive_pack_advertisement_puts_caps_on_the_first_ref_or_on_capabilities() {
        let caps = receive_pack_caps("graftyard/1", "abc");
        let empty = receive_pack_advertisement(&[], &caps);
        assert!(empty.starts_with(b"001f# service=git-receive-pack\n0000"));
        let want = format!("{} capabilities^{{}}\0{caps}\n", ObjectId::ZERO.hex());
        assert!(String::from_utf8_lossy(&empty).contains(&want));
        let refs = vec![
            (RefName::parse(b"refs/heads/z").unwrap(), oid(2)),
            (RefName::parse(b"refs/heads/a").unwrap(), oid(1)),
        ];
        let a = receive_pack_advertisement(&refs, &caps);
        let s = String::from_utf8_lossy(&a);
        let first = s.find("refs/heads/a\0").unwrap();
        assert!(first < s.find("refs/heads/z\n").unwrap(), "sorted, caps on the first");
        assert!(!s.contains("capabilities^{}"));
        assert!(s.ends_with("0000"));
    }

    #[test]
    fn receive_pack_requests_parse_commands_caps_options_and_the_pack_offset() {
        let body = request(
            &[cmd(ObjectId::ZERO, oid(1), "refs/heads/main"), cmd(oid(2), ObjectId::ZERO, "refs/heads/old")],
            "report-status-v2 side-band-64k atomic push-options agent=git/2.39.5",
            &["ci.skip", "x=y"],
            b"PACKxyz",
        );
        let r = parse_receive_pack(&body).unwrap();
        assert_eq!(r.updates.len(), 2);
        assert_eq!(r.updates[0].expect, None, "zero old is a create");
        assert_eq!(r.updates[1].new, None, "zero new is a delete");
        assert!(r.caps.atomic && r.caps.side_band_64k && r.caps.report_status_v2 && !r.caps.report_status);
        assert_eq!(r.caps.agent.as_deref(), Some("git/2.39.5"));
        assert_eq!(r.options, ["ci.skip", "x=y"]);
        assert_eq!(&body[r.pack_at..], b"PACKxyz");
        // Cut inside the pack: still parses, for the cap refusal.
        assert!(parse_receive_pack(&body[..r.pack_at + 2]).is_ok());
    }

    #[test]
    fn receive_pack_requests_refuse_what_git_would_not_send() {
        let ok = cmd(ObjectId::ZERO, oid(1), "refs/heads/main");
        assert_eq!(parse_receive_pack(b"0000"), Err(RequestError::Empty));
        let mut b = Vec::new();
        pkt::data(&mut b, &ok);
        assert_eq!(parse_receive_pack(&b), Err(RequestError::Unterminated), "no flush");
        for bad in [
            cmd(ObjectId::ZERO, oid(1), "HEAD"),
            cmd(ObjectId::ZERO, oid(1), "refs/heads/a..b"),
            b"short".to_vec(),
            format!("{} {}refs/heads/x", ObjectId::ZERO.hex(), oid(1).hex()).into_bytes(),
        ] {
            assert!(
                parse_receive_pack(&request(std::slice::from_ref(&bad), "report-status", &[], b"")).is_err(),
                "{bad:?}"
            );
        }
        let mut b = Vec::new();
        pkt::line(&mut b, &format!("shallow {}", oid(1).hex()));
        assert_eq!(parse_receive_pack(&b), Err(RequestError::Shallow));
        // A request with push-options in its caps must carry the options section.
        let body = request(&[ok], "report-status push-options", &[], b"");
        assert_eq!(parse_receive_pack(&body[..body.len() - 4]), Err(RequestError::Unterminated));
    }

    /// How `BodyReader` read `body` fed in `chunk`-byte chunks with a cap of `cap`: where it stopped, and for a body
    /// that streams, the commands, the pack bytes it held, and how many body bytes it had read.
    #[derive(Debug, PartialEq)]
    enum Fed {
        Ended(Ended),
        OverCap(Vec<u8>, usize),
        Stream(ReceivePack, Vec<u8>, u64),
    }

    fn feed(mut r: BodyReader, body: &[u8], chunk: usize) -> Fed {
        for (i, c) in body.chunks(chunk).enumerate() {
            match r.push(c) {
                Read::More => {}
                Read::OverCap => return Fed::OverCap(r.cap_head().to_vec(), i),
                Read::Stream => {
                    let (rp, s) = r.streaming();
                    return Fed::Stream(rp, s.pack, s.read);
                }
            }
        }
        Fed::Ended(r.end())
    }

    /// The edge holds a push's body only while it may be inline: a pack of INLINE_MAX bytes is read whole, as the edge
    /// always read every body, and one byte more streams once that byte arrives, so the edge holds at most the
    /// commands, INLINE_MAX, and one chunk of any pack, however large. A buffering reader fails here on every large
    /// body.
    #[test]
    fn the_edge_holds_a_push_body_only_while_its_pack_may_be_inline() {
        let cmds = [cmd(ObjectId::ZERO, oid(1), "refs/heads/main"), cmd(oid(2), oid(3), "refs/heads/dev")];
        let caps = "report-status side-band-64k push-options agent=git/2.47.0";
        let at = request(&cmds, caps, &["ci.skip"], b"").len();
        let pack = |n: usize| (0..n).map(|i| (i * 7 % 251) as u8).collect::<Vec<u8>>();
        // Commands split over chunks of every size, down to one byte, parse the same.
        for chunk in [1, 5, 1000, 64 << 10] {
            let body = request(&cmds, caps, &["ci.skip"], &pack(INLINE_MAX));
            assert_eq!(feed(BodyReader::new(1 << 40), &body, chunk), Fed::Ended(Ended::Whole(body.clone())), "{chunk}");
        }
        for (n, chunk) in [(INLINE_MAX + 1, 1 << 10), (INLINE_MAX + 1, 1), (60 << 20, 64 << 10)] {
            let body = request(&cmds, caps, &["ci.skip"], &pack(n));
            let Fed::Stream(rp, held, read) = feed(BodyReader::new(1 << 40), &body, chunk) else {
                panic!("a pack of {n} bytes in {chunk}-byte chunks did not stream")
            };
            assert_eq!(rp, parse_receive_pack(&body).unwrap());
            assert!(held.len() > INLINE_MAX && held.len() <= INLINE_MAX + chunk, "held {} of {n}", held.len());
            assert_eq!(read as usize, at + held.len(), "it held every byte it read past the commands");
            assert_eq!(&body[at..read as usize], &held[..]);
        }
        // Deletes only: no pack, read whole.
        let del = request(&[cmd(oid(2), ObjectId::ZERO, "refs/heads/old")], "report-status", &[], b"");
        assert_eq!(feed(BodyReader::new(1 << 40), &del, 3), Fed::Ended(Ended::Whole(del.clone())));
    }

    /// Every other body is answered as when the edge read it whole: the probe's bare flush and a body that parses to
    /// an error come back whole; a long body whose commands are bad is answered the error a whole-body parse gives,
    /// without holding it; a gzip body is held whole, whatever its length; and the cap counts every byte, keeping the
    /// first `CAP_HEAD`, whether the body streams or not.
    #[test]
    fn a_push_body_the_edge_does_not_stream_reads_as_when_it_was_held_whole() {
        assert_eq!(feed(BodyReader::new(100), b"0000", 1), Fed::Ended(Ended::Whole(b"0000".to_vec())));
        let bad = request(&[cmd(ObjectId::ZERO, oid(1), "HEAD")], "report-status", &[], &[7; 100]);
        assert_eq!(feed(BodyReader::new(1 << 40), &bad, 9), Fed::Ended(Ended::Whole(bad.clone())));
        let big = request(&[cmd(ObjectId::ZERO, oid(1), "HEAD")], "report-status", &[], &[7; 3 << 20]);
        assert_eq!(
            feed(BodyReader::new(1 << 40), &big, 64 << 10),
            Fed::Ended(Ended::Bad(parse_receive_pack(&big).unwrap_err()))
        );
        // No flush in a long body: not a push git sends, and parsed as the whole body parses.
        let mut runon = Vec::new();
        while runon.len() <= INLINE_MAX + 10 {
            pkt::data(&mut runon, &cmd(ObjectId::ZERO, oid(1), "refs/heads/main"));
        }
        assert_eq!(
            feed(BodyReader::new(1 << 40), &runon, 4096),
            Fed::Ended(Ended::Whole(runon.clone())),
            "an unterminated body is held: a parse never fails on it before it ends"
        );
        let long = request(&[cmd(ObjectId::ZERO, oid(1), "refs/heads/main")], "report-status", &[], &[1; 3 << 20]);
        assert_eq!(feed(BodyReader::whole(1 << 40), &long, 64 << 10), Fed::Ended(Ended::Whole(long.clone())));
        // The cap: one byte over is refused, at the chunk that passed it, with the first CAP_HEAD bytes kept. (Past
        // the head the edge counts a streamed body's rest itself.)
        let short = request(&[cmd(ObjectId::ZERO, oid(1), "refs/heads/main")], "report-status", &[], &[1; 500_000]);
        for (body, r) in [(&short, BodyReader::new as fn(u64) -> BodyReader), (&long, BodyReader::whole)] {
            let cap = body.len() as u64 - 1;
            assert_eq!(feed(r(cap + 1), body, 1000), Fed::Ended(Ended::Whole(body.clone())));
            let at = body.len().div_ceil(1000) - 1;
            assert_eq!(feed(r(cap), body, 1000), Fed::OverCap(body[..CAP_HEAD].to_vec(), at));
        }
        assert_eq!(feed(BodyReader::new(10), &long, 1000), Fed::OverCap(long[..1000].to_vec(), 0));
        // A long bad body is no longer held, but every byte still counts.
        let cap = big.len() as u64 - 1;
        let at = big.len().div_ceil(1000) - 1;
        assert_eq!(feed(BodyReader::new(cap), &big, 1000), Fed::OverCap(big[..CAP_HEAD].to_vec(), at));
    }

    #[test]
    fn refusals_say_ng_on_every_ref_and_never_ok() {
        let body = request(
            &[cmd(ObjectId::ZERO, oid(1), "refs/heads/a"), cmd(ObjectId::ZERO, oid(2), "refs/heads/b")],
            "report-status side-band-64k",
            &[],
            b"",
        );
        let req = parse_receive_pack(&body).unwrap();
        for r in [
            Refusal::too_many_atomic(10_001),
            Refusal::updates_too_large(1_300_000, 1_200_000, 2),
            cap_refusal(Plan::Pro, false, 80_000_000, "h", "o/r"),
            Refusal::unpack_failed("index-pack failed\nsecond line"),
        ] {
            let out = r.render(&req);
            let s = String::from_utf8_lossy(&out);
            assert!(!s.contains("\x01ok ") && !s.contains("ok refs/"), "{s}");
            assert_eq!(s.matches("ng refs/heads/").count(), 2, "{s}");
            assert!(s.ends_with("0000"));
        }
        let s = String::from_utf8_lossy(&Refusal::too_many_atomic(10_001).render(&req)).into_owned();
        assert!(s.contains("at most 10000 refs"), "the message names the limit: {s}");
    }

    /// Agent admission's git shape (`design/DESIGN.md`, Shape, "Agent load and agent safety"): report-status `ng <ref>
    /// agent admission full, retry after <n>s` on every ref, and a `remote:` line on band 2.
    #[test]
    fn agent_admission_refuses_in_the_ng_shape_with_a_remote_line() {
        let body = request(
            &[cmd(ObjectId::ZERO, oid(1), "refs/heads/agent/a"), cmd(ObjectId::ZERO, oid(2), "refs/heads/agent/b")],
            "report-status side-band-64k",
            &[],
            b"",
        );
        let req = parse_receive_pack(&body).unwrap();
        let s = String::from_utf8_lossy(&Refusal::agent_full(7).render(&req)).into_owned();
        for r in ["refs/heads/agent/a", "refs/heads/agent/b"] {
            assert!(s.contains(&format!("ng {r} agent admission full, retry after 7s\n")), "{s}");
        }
        assert!(s.contains("\x02This repository is admitting agent writes"), "a remote: line on band 2: {s}");
        assert!(!s.contains("ok refs/"));
    }

    #[test]
    fn the_cap_message_names_what_each_plan_offers() {
        let text = |p, o| cap_refusal(p, o, 80_000_000, "git.example.com", "o/r").remote.join(" ");
        let pro = text(Plan::Pro, false);
        assert!(pro.contains("80 MB") && pro.contains("smaller pushes") && pro.contains("Enterprise"));
        assert!(!pro.contains("git@"), "no SSH without the option: {pro}");
        let opt = text(Plan::Business, true);
        assert!(opt.contains("cloudflared") && opt.contains("git@git.example.com:o/r.git"));
        let ent = text(Plan::Enterprise, false);
        assert!(ent.contains("SSH: git push git@git.example.com:o/r.git") && !ent.contains("cloudflared"));
        for t in [pro, opt, ent] {
            assert!(t.contains("HTTPS stays the default"), "{t}");
        }
    }

    #[test]
    fn v2_requests_split_caps_from_args() {
        let mut b = Vec::new();
        pkt::line(&mut b, "command=fetch");
        pkt::line(&mut b, "agent=git/2.51.0");
        pkt::line(&mut b, "object-format=sha1");
        pkt::delim(&mut b);
        pkt::line(&mut b, "thin-pack");
        pkt::line(&mut b, &format!("want {}", oid(3).hex()));
        pkt::line(&mut b, "done");
        pkt::flush(&mut b);
        let r = parse_v2(&b).unwrap();
        assert_eq!(r.command, "fetch");
        assert_eq!(r.caps, ["agent=git/2.51.0", "object-format=sha1"]);
        assert_eq!(r.args.len(), 3);
        assert_eq!(parse_v2(b"0000"), Err(RequestError::Empty));
    }
}
