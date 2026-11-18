//! Composed clone-cache misses: the chain's R2 layout and the rules the shard object applies before it forwards an
//! upload-pack (`design/DESIGN.md`, Shape, "Git cache shards (derived)"; `decisions/DECISIONS.md`, "Clone-cache misses
//! are composed from the cached tip"). The container builds and serves the chain (`containers/git/server/compose.go`);
//! this module decides which requests wait for it and which chain files a shard still has to fetch.
//!
//! In each warm bucket a network's chain is `compose/<net>/head`, a small JSON object naming the base's tip, each
//! part's tip in order, and the last tip's tree, and beside it `base-<tip>` and `part-<from>-<to>`, each a `.pack`
//! and its `.idx`. The home site's first shard writes the files first and the head last, so a head never names a file
//! that is not there yet; a shard that finds one missing anyway serves stock.

use serde::{Deserialize, Serialize};

use crate::refs::ObjectId;
use crate::smart::V2Request;

/// The chain as the container's `/compose-head` and R2's `compose/<net>/head` hold it. Hex strings, as the container
/// writes them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainHead {
    pub base: String,
    #[serde(default)]
    pub parts: Vec<String>,
    pub tree: String,
}

impl ChainHead {
    /// `None` unless every id is a full SHA-1 in lowercase hex.
    pub fn checked(self) -> Option<ChainHead> {
        let ok = |s: &str| s.len() == 40 && ObjectId::parse_hex(s.as_bytes()).is_some_and(|o| o.hex() == s);
        (ok(&self.base) && ok(&self.tree) && self.parts.iter().all(|p| ok(p))).then_some(self)
    }

    /// The chain's tip: its last part's, else its base's.
    pub fn tip(&self) -> &str {
        self.parts.last().unwrap_or(&self.base)
    }

    /// The chain's packs in pack order, each by its name without the extension: `base-<tip>`, then
    /// `part-<from>-<to>` per move.
    pub fn packs(&self) -> Vec<String> {
        let mut out = vec![format!("base-{}", self.base)];
        let mut from = &self.base;
        for p in &self.parts {
            out.push(format!("part-{from}-{p}"));
            from = p;
        }
        out
    }

    /// Every file the chain needs, `.pack` and `.idx` of each pack.
    pub fn files(&self) -> Vec<String> {
        self.packs().into_iter().flat_map(|p| [format!("{p}.pack"), format!("{p}.idx")]).collect()
    }

    /// The files `self` names that `had` does not: what a shard holding `had` fetches to install `self`. All of them
    /// when it holds no chain.
    pub fn to_fetch(&self, had: Option<&ChainHead>) -> Vec<String> {
        let have = had.map(ChainHead::files).unwrap_or_default();
        self.files().into_iter().filter(|f| !have.contains(f)).collect()
    }
}

/// Whether `name` is a chain file name: `base-<tip>` or `part-<from>-<to>`, then `.pack` or `.idx`.
pub fn file_name_ok(name: &str) -> bool {
    let hex = |s: &str| s.len() == 40 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    if ext != "pack" && ext != "idx" {
        return false;
    }
    match stem.split('-').collect::<Vec<_>>().as_slice() {
        ["base", t] => hex(t),
        ["part", f, t] => hex(f) && hex(t),
        _ => false,
    }
}

/// The arguments a composable fetch may carry besides one `want`, `deepen 1`, `ofs-delta`, and `done`.
const OTHER_ARGS: [&str; 3] = ["thin-pack", "no-progress", "include-tag"];

/// Whether a protocol v2 request is a depth-1 fetch the chain may answer: `fetch`, SHA-1, one wanted id (git clone
/// sends HEAD's and its branch's, the same id twice), `deepen 1`, `ofs-delta`, `done`, and no other argument but
/// `thin-pack`, `no-progress`, and `include-tag`. Only these wait for the shard to ready its chain; anything else goes to stock upload-pack at once. The container applies the
/// same rule (`composable` in `compose.go`) and adds the checks that need the repository: the want is a ref's tip and a
/// commit, the chain is at HEAD, and with `include-tag` no annotated tag points at it.
pub fn composable(req: &V2Request) -> bool {
    if req.command != "fetch" || req.caps.iter().any(|c| c.starts_with("object-format=") && c != "object-format=sha1") {
        return false;
    }
    let (mut want, mut depth1, mut ofs, mut done) = (None, false, false, false);
    for a in &req.args {
        let Ok(a) = std::str::from_utf8(a) else { return false };
        match a {
            "deepen 1" => depth1 = true,
            "ofs-delta" => ofs = true,
            "done" => done = true,
            a if OTHER_ARGS.contains(&a) => {}
            a => match a.strip_prefix("want ") {
                Some(w)
                    if w.len() == 40 && ObjectId::parse_hex(w.as_bytes()).is_some() && want.is_none_or(|o| o == w) =>
                {
                    want = Some(w)
                }
                _ => return false,
            },
        }
    }
    want.is_some() && depth1 && ofs && done
}

/// The container's reason when a composable want is not on the disk yet: the tip moved and this shard has not caught
/// up to it (`compose.go`, `composeMiss`). Retried after the running catch-up.
pub const NOT_YET: &str = "stock: not a ref tip or behind HEAD";

/// The container's reason when the want's tree is past `composeDiffMax` entries from the chain's: the chain is on a
/// tip far from HEAD. Retried after the chain's next step, which builds a new base in that case (`compose.go`,
/// `handleCompose`). On B0-34 m1b34 it was a chain built on the wrong tip, never a far want: over the row's 600 real
/// tip moves a want was at most 2,969 entries from the tip 20 moves back (`tests/am1/results/b034-attr.md`).
pub const OVER_CAP: &str = "stock: over the diff cap";

/// What a composable miss that the container answered 409 got in the end (`compose_then_catch_up`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterCompose<R> {
    /// Composed after the running catch-up, or after the chain's next step.
    Composed(R),
    /// Catch up as before, through a catch-up that starts after now, then serve (composed if the disk then allows it,
    /// else stock): the reason is neither `NOT_YET` nor `OVER_CAP`, `NOT_YET` with no catch-up running, or the retry
    /// still missed.
    CatchUp,
    /// The caller's deadline passed first: the shard answers 503 `warming` and the edge asks the next site.
    Late,
}

/// A composable miss after the container's first `compose=only` answer was 409 with `why`, all of it within
/// `deadline`, the one deadline of the whole upload-pack (the caller's `wait`).
/// - `NOT_YET` with a catch-up running (`running`): wait for it to end (`in_flight`) and ask composed again
///   (`compose`, `Some` on a 200). m41's B0-34 had 143 misses on a just-moved tip, all composed in the end.
/// - `OVER_CAP`: wait for any running catch-up, then for a chain step that starts after now (`step`), then ask
///   composed again.
/// - Anything else, or a retry that missed: `AfterCompose::CatchUp`, as it always did.
///
/// Past `deadline`, `AfterCompose::Late`. Unbounded, a miss waited out a whole stuck catch-up before its 15 s even
/// started: 25 of m1b34's 50 shard 503s, 29 to 99 s each (`tests/am1/results/b034-attr.md`).
pub async fn compose_then_catch_up<R, D, W, Fw, S, Fs, C, Fc>(
    deadline: D,
    why: Option<&str>,
    running: bool,
    in_flight: W,
    step: S,
    compose: C,
) -> AfterCompose<R>
where
    D: std::future::Future<Output = ()>,
    W: FnOnce() -> Fw,
    Fw: std::future::Future<Output = ()>,
    S: FnOnce() -> Fs,
    Fs: std::future::Future<Output = ()>,
    C: FnOnce() -> Fc,
    Fc: std::future::Future<Output = Option<R>>,
{
    let over = why == Some(OVER_CAP);
    if !(over || why == Some(NOT_YET) && running) {
        return AfterCompose::CatchUp;
    }
    let work = async {
        if running {
            in_flight().await;
        }
        if over {
            step().await;
        }
        match compose().await {
            Some(r) => AfterCompose::Composed(r),
            None => AfterCompose::CatchUp,
        }
    };
    match futures_util::future::select(std::pin::pin!(work), std::pin::pin!(deadline)).await {
        futures_util::future::Either::Left((a, _)) => a,
        futures_util::future::Either::Right(_) => AfterCompose::Late,
    }
}

/// Stock upload-packs one shard container runs at once [guess], as the container's `stockMax`: linux's took 2.6 GB of
/// RSS each, and m1b34's monorepo container ran 15 to 21 of them in 12 GiB, holding the network's lock shared 20 to
/// 99 s each, while catch-ups and every validate queued behind them (`tests/am1/results/b034-attr.md`). The next B0-34
/// row's stock-process count per sample (`shardprocs`) measures it. Past it the container answers 429 and the object
/// holds the request in its `StockLine`.
pub const STOCK_MAX: u32 = 2;

/// How often the head of a `StockLine` asks the container again [guess].
pub const STOCK_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// The stock upload-packs one shard object holds while its container's `STOCK_MAX` slots are full, in arrival order:
/// only the head asks the container again, so a later request never takes a slot ahead of an earlier one. A wait, not
/// a 429 to the client: git retries neither 429 nor 503 unless `http.maxRetries` is set, and then only 429 [doc,
/// git-config, `http.maxRetries`]. Each waits within its upload-pack's deadline and leaves the line past it.
#[derive(Debug, Default)]
pub struct StockLine {
    next: u64,
    waiting: std::collections::VecDeque<u64>,
}

impl StockLine {
    /// Whether a new stock request must queue: someone is already waiting.
    pub fn busy(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// A place at the back of the line.
    pub fn join(&mut self) -> u64 {
        self.next += 1;
        self.waiting.push_back(self.next);
        self.next
    }

    /// Whether `t` is at the front, the one place that asks the container.
    pub fn is_head(&self, t: u64) -> bool {
        self.waiting.front() == Some(&t)
    }

    /// Leaves the line: admitted, or past its deadline. The next place becomes the head.
    pub fn leave(&mut self, t: u64) {
        self.waiting.retain(|w| *w != t);
    }

    pub fn len(&self) -> usize {
        self.waiting.len()
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }
}

/// One stock upload-pack through `line` within `deadline`. `call` asks the container, `None` when its slots are full
/// (429); `pause` waits `STOCK_POLL`. A request that finds nobody waiting asks at once; otherwise, or once refused, it
/// joins the line, and only the head asks again. `None` past `deadline`, having left the line.
pub async fn stock_in_line<R, D, C, Fc, P, Fp>(
    line: &std::cell::RefCell<StockLine>,
    deadline: D,
    mut call: C,
    mut pause: P,
) -> Option<R>
where
    D: std::future::Future<Output = ()>,
    C: FnMut() -> Fc,
    Fc: std::future::Future<Output = Option<R>>,
    P: FnMut() -> Fp,
    Fp: std::future::Future<Output = ()>,
{
    struct Place<'a>(&'a std::cell::RefCell<StockLine>, Option<u64>);
    impl Drop for Place<'_> {
        fn drop(&mut self) {
            if let Some(t) = self.1 {
                self.0.borrow_mut().leave(t);
            }
        }
    }
    let work = async {
        if !line.borrow().busy()
            && let Some(r) = call().await
        {
            return r;
        }
        let place = Place(line, Some(line.borrow_mut().join()));
        let t = place.1.expect("joined");
        loop {
            pause().await;
            if line.borrow().is_head(t)
                && let Some(r) = call().await
            {
                return r;
            }
        }
    };
    match futures_util::future::select(std::pin::pin!(work), std::pin::pin!(deadline)).await {
        futures_util::future::Either::Left((r, _)) => Some(r),
        futures_util::future::Either::Right(_) => None,
    }
}

/// One clone-cache miss on a shard object, for its log line `shard miss <net>` (`workers/site/src/shard.rs`,
/// "/upload-pack"). Each time is epoch ms: when the miss arrived, `Compose::ready` returned, the `compose=only` answer
/// came (status and `x-graft-compose`), the catch-up before stock ended, the serving answer's headers came (the
/// container sends them with its first byte), and its first chunk went on to the edge. Beside the client's own time
/// to first byte, the line splits a miss into the wait for the chain, the container's work, and the transit.
#[derive(Clone, Debug, Default)]
pub struct MissLog {
    pub arrived: u64,
    pub ready: Option<u64>,
    pub only: Option<(u64, u16, Option<String>)>,
    pub caught_up: Option<u64>,
    pub served: Option<(u64, u16, Option<String>)>,
    pub first: Option<u64>,
}

impl MissLog {
    /// The line's JSON: `arrived` as epoch ms, every later time in ms after it, and `answer`, the serving answer's
    /// `x-graft-compose` (a 200 without one is stock upload-pack on a request the container found not composable).
    pub fn line(&self) -> serde_json::Value {
        let ms = |t: u64| t.saturating_sub(self.arrived);
        let answer = match &self.served {
            Some((_, 200, c)) => c.clone().unwrap_or_else(|| "stock: not composable".into()),
            Some((_, s, _)) => format!("status {s}"),
            None => "no answer".into(),
        };
        serde_json::json!({
            "arrived": self.arrived,
            "answer": answer,
            "ready_ms": self.ready.map(ms),
            "only": self.only.as_ref().map(|(t, s, c)| serde_json::json!({ "ms": ms(*t), "status": s, "compose": c })),
            "caught_up_ms": self.caught_up.map(ms),
            "headers_ms": self.served.as_ref().map(|(t, ..)| ms(*t)),
            "first_ms": self.first.map(ms),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smart::parse_v2;

    /// Every time is from the miss's arrival, not from the step before it, and the answer names stock unless the
    /// serving answer says composed: a 409 then a stock 200 after the catch-up is stock with the 409's reason kept.
    #[test]
    fn a_miss_line_times_each_step_from_arrival_and_names_its_answer() {
        let stock = MissLog {
            arrived: 1_000_000,
            ready: Some(1_000_003),
            only: Some((1_000_010, 409, Some("stock: no chain".into()))),
            caught_up: Some(1_004_000),
            served: Some((1_009_000, 200, Some("stock: no chain".into()))),
            first: Some(1_009_250),
        };
        let l = stock.line();
        assert_eq!(l["arrived"], 1_000_000);
        assert_eq!(l["ready_ms"], 3);
        assert_eq!(l["only"], serde_json::json!({ "ms": 10, "status": 409, "compose": "stock: no chain" }));
        assert_eq!(l["caught_up_ms"], 4_000);
        assert_eq!(l["headers_ms"], 9_000);
        assert_eq!(l["first_ms"], 9_250);
        assert_eq!(l["answer"], "stock: no chain");
        let bare = MissLog { arrived: 5, served: Some((7, 200, None)), ..MissLog::default() };
        assert_eq!(bare.line()["answer"], "stock: not composable");
        assert_eq!(bare.line()["first_ms"], serde_json::Value::Null);
        let warming = MissLog { arrived: 5, served: Some((9, 503, None)), ..MissLog::default() };
        assert_eq!(warming.line()["answer"], "status 503");
        assert_eq!(MissLog { arrived: 5, ..MissLog::default() }.line()["answer"], "no answer");
    }

    fn pkt(s: &str) -> String {
        format!("{:04x}{s}", s.len() + 4)
    }

    fn fetch(args: &[&str]) -> V2Request {
        let mut b = pkt("command=fetch\n") + &pkt("agent=git/2.56\n") + &pkt("object-format=sha1\n") + "0001";
        for a in args {
            b += &pkt(&format!("{a}\n"));
        }
        parse_v2((b + "0000").as_bytes()).unwrap()
    }

    const W: &str = "want 0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn only_a_one_want_depth_one_ofs_delta_fetch_is_composable() {
        // As `git clone --depth 1` asks it.
        let clone = ["thin-pack", "no-progress", "include-tag", "ofs-delta", "deepen 1", W, "done"];
        assert!(composable(&fetch(&clone)));
        assert!(composable(&fetch(&["ofs-delta", "deepen 1", W, "done"])));
        assert!(composable(&fetch(&[&clone[..], &[W]].concat())), "the same want twice, as git clone sends it");
        let without = |drop: &str| -> Vec<&str> { clone.iter().copied().filter(|a| *a != drop).collect() };
        for (case, args) in [
            ("no ofs-delta", without("ofs-delta")),
            ("no deepen", without("deepen 1")),
            ("no done", without("done")),
            ("no want", without(W)),
            ("two wants", [&clone[..], &["want 1123456789abcdef0123456789abcdef01234567"]].concat()),
            ("deepen 2", [&without("deepen 1")[..], &["deepen 2"]].concat()),
            ("a have", [&clone[..], &["have 1123456789abcdef0123456789abcdef01234567"]].concat()),
            ("a client shallow", [&clone[..], &["shallow 1123456789abcdef0123456789abcdef01234567"]].concat()),
            ("a filter", [&clone[..], &["filter blob:none"]].concat()),
            ("deepen-since", [&clone[..], &["deepen-since 1700000000"]].concat()),
            ("a short want", [&without(W)[..], &["want 0123"]].concat()),
            ("want-ref", [&without(W)[..], &["want-ref refs/heads/main"]].concat()),
        ] {
            assert!(!composable(&fetch(&args)), "{case}");
        }
        let mut sha256 = fetch(&clone);
        sha256.caps = vec!["object-format=sha256".into()];
        assert!(!composable(&sha256), "sha256");
        let mut ls = fetch(&clone);
        ls.command = "ls-refs".into();
        assert!(!composable(&ls), "ls-refs");
    }

    fn id(c: char) -> String {
        c.to_string().repeat(40)
    }

    #[test]
    fn a_shard_fetches_only_the_files_its_chain_lacks() {
        let h = ChainHead { base: id('a'), parts: vec![id('b'), id('c')], tree: id('d') };
        assert_eq!(h.tip(), id('c'));
        assert_eq!(
            h.packs(),
            [
                format!("base-{}", id('a')),
                format!("part-{}-{}", id('a'), id('b')),
                format!("part-{}-{}", id('b'), id('c'))
            ]
        );
        assert!(h.files().iter().all(|f| file_name_ok(f)));
        assert_eq!(h.to_fetch(None).len(), 6);
        // One move behind: only the new part.
        let had = ChainHead { base: id('a'), parts: vec![id('b')], tree: id('e') };
        assert_eq!(
            h.to_fetch(Some(&had)),
            [format!("part-{}-{}.pack", id('b'), id('c')), format!("part-{}-{}.idx", id('b'), id('c'))]
        );
        // After a fold the old files are no help.
        let folded = ChainHead { base: id('c'), parts: vec![], tree: id('d') };
        assert_eq!(folded.to_fetch(Some(&h)), [format!("base-{}.pack", id('c')), format!("base-{}.idx", id('c'))]);
        // A part to the same tip from another base is a different file.
        let other = ChainHead { base: id('f'), parts: vec![id('c')], tree: id('d') };
        assert_eq!(other.to_fetch(Some(&h)).len(), 4);
        assert_eq!(h.to_fetch(Some(&h)), Vec::<String>::new());
        let base_only = ChainHead { base: id('a'), parts: vec![], tree: id('d') };
        assert_eq!(base_only.tip(), id('a'));
    }

    #[test]
    fn heads_and_names_are_checked() {
        assert!(ChainHead { base: id('a'), parts: vec![id('b')], tree: id('c') }.checked().is_some());
        assert!(ChainHead { base: id('A'), parts: vec![], tree: id('c') }.checked().is_none());
        assert!(ChainHead { base: id('a'), parts: vec!["b".into()], tree: id('c') }.checked().is_none());
        assert!(ChainHead { base: id('a'), parts: vec![], tree: "../x".into() }.checked().is_none());
        let h: ChainHead = serde_json::from_str(&format!(r#"{{"base":"{}","tree":"{}"}}"#, id('a'), id('b'))).unwrap();
        assert!(h.parts.is_empty());
        for bad in [
            "base-x.pack",
            "base-.pack",
            "part-aa-bb.idx",
            &format!("base-{}.rev", id('a')),
            &format!("base-{}", id('a')),
            &format!("base-{}-{}.pack", id('a'), id('b')),
            &format!("part-{}.pack", id('a')),
            &format!("../base-{}.pack", id('a')),
            &format!("base-{}.pack", id('A')),
        ] {
            assert!(!file_name_ok(bad), "{bad}");
        }
    }
}
