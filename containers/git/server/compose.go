package main

// Composed clone-cache misses and the clone-cache fill (design/DESIGN.md, Shape, "Git cache shards (derived)";
// decisions/DECISIONS.md, "Clone-cache misses are composed from the cached tip" and "The clone cache fills from the
// shard").
//
//	POST /compose?net=N[&start=1][&tip=C]  the chain's next step, to C (default HEAD's commit): with no chain, a base
//	                                 at C, only with start=1; else a part from the chain's tip to C, or, at the
//	                                 foldEvery-th move or past composeDiffMax entries from the chain's tree, a new
//	                                 base at C. {"head","built": [file names],"cpuMs"};
//	                                 "head" null when there is no chain. Holds no network lock
//	GET  /compose-head?net=N         the chain's head, {"base","parts","tree"} or null
//	GET  /compose-file?net=N&name=F  one of the chain's files (base-<tip>.pack|.idx, part-<from>-<to>.pack|.idx)
//	PUT  /compose-file?net=N&name=F  body: a chain file from R2, staged for /compose-head
//	PUT  /compose-head?net=N         body: a head. Installs the staged files it names (each pack checked against its
//	                                 .idx) and drops every chain file it does not name; 409 {"missing"} without one
//	GET  /spooled?id=H               the whole response a /upload-pack &spool=H wrote, once it ended, with its length;
//	                                 404 when it failed or there is none
//	DELETE /spooled?id=H             deletes it
//
// The chain is a base, the depth-1 trees and blobs of one tip, and one part per tip move, the new tip's trees and
// blobs that the base and earlier parts lack. No two of its packs share an object, none holds a commit, and every
// delta in each is to an object earlier in the same pack (ofs-delta), so their bodies concatenated under one header
// and one trailer, with a last pack of the wanted commit's own new objects, are a pack the client's index-pack takes
// as it is. Packs live in their own object directory, <data>/compose/<net>/objects, beside the repository.

import (
	"bufio"
	"bytes"
	"crypto/sha1"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"
)

// foldEvery: a chain folds its base and parts into a new base at this many tip moves (DESIGN, "Git cache shards").
const foldEvery = 20

// composeDiffMax: a wanted commit whose tree differs from the chain tip's in more entries than this is served by stock
// upload-pack [guess]: it is far from the tip, so a composed pack would carry most of two trees. A var for the tests.
var composeDiffMax = 20000

type chainHead struct {
	Base  string   `json:"base"`
	Parts []string `json:"parts"`
	Tree  string   `json:"tree"`
}

func (h chainHead) tip() string {
	if len(h.Parts) > 0 {
		return h.Parts[len(h.Parts)-1]
	}
	return h.Base
}

// packs is the chain's packs in pack order, each by its name without the extension.
func (h chainHead) packs() []string {
	out := []string{"base-" + h.Base}
	from := h.Base
	for _, p := range h.Parts {
		out = append(out, "part-"+from+"-"+p)
		from = p
	}
	return out
}

var fileRe = regexp.MustCompile(`^(base-[0-9a-f]{40}|part-[0-9a-f]{40}-[0-9a-f]{40})\.(pack|idx)$`)

func composeDir(net string) string     { return filepath.Join(data, "compose", net) }
func composeObjects(net string) string { return filepath.Join(composeDir(net), "objects") }
func chainFile(net, name, ext string) string {
	return filepath.Join(composeObjects(net), "pack", "pack-"+name+"."+ext)
}

// One build or install of a network's chain at a time. Serving reads the head and opens its files, so it never waits.
var composeLocks sync.Map

func composeLock(net string) *sync.Mutex {
	m, _ := composeLocks.LoadOrStore(net, &sync.Mutex{})
	return m.(*sync.Mutex)
}

func readHead(net string) (chainHead, bool) {
	var h chainHead
	b, err := os.ReadFile(filepath.Join(composeDir(net), "head.json"))
	if err != nil || json.Unmarshal(b, &h) != nil || !hexRe.MatchString(h.Base) || !hexRe.MatchString(h.Tree) {
		return chainHead{}, false
	}
	return h, true
}

func writeHead(net string, h chainHead) error {
	b, _ := json.Marshal(h)
	p := filepath.Join(composeDir(net), "head.json")
	if err := os.WriteFile(p+".tmp", b, 0o644); err != nil {
		return err
	}
	return os.Rename(p+".tmp", p)
}

// dropUnnamed deletes every chain pack h does not name. Open packs of a miss being served stay readable.
func dropUnnamed(net string, h chainHead) {
	keep := map[string]bool{}
	for _, p := range h.packs() {
		keep["pack-"+p] = true
	}
	fs, _ := filepath.Glob(filepath.Join(composeObjects(net), "pack", "pack-*"))
	for _, f := range fs {
		b := filepath.Base(f)
		if !keep[strings.TrimSuffix(b, filepath.Ext(b))] {
			os.Remove(f)
		}
	}
}

// inCompose runs git in the repository with the chain's objects as an alternate, so the chain tip's trees resolve
// even before the repository holds them.
func inCompose(net string) []string {
	return []string{"GIT_ALTERNATE_OBJECT_DIRECTORIES=" + composeObjects(net)}
}

// chainFirst looks objects up in the chain's packs before the repository's, so a fold reuses the chain's deltas,
// whose bases are in the new base too: 1.7 CPU-s for linux's 103,000 tip objects, against 10 from the repository's
// packs, whose deltas mostly name bases outside it [measured, tests/m0/results/composed-miss.md].
func chainFirst(net, rp string) []string {
	return []string{"GIT_OBJECT_DIRECTORY=" + composeObjects(net),
		"GIT_ALTERNATE_OBJECT_DIRECTORIES=" + filepath.Join(rp, "objects")}
}

// gitRun runs git on a cpuMeter: m.gitLow, at nice 19, for the chain's builds, which are derived work and must yield
// the vCPUs to validates and catch-ups (as the repack does, LM-1 round 24c); m.git for a miss a client waits on.
type gitRun func(dir string, env []string, stdin io.Reader, args ...string) ([]byte, error)

// newObjects is the root tree of `to` and every tree and blob in its tree that differs from tree `from`, each with
// its path, which pack-objects groups delta candidates by. Gitlinks name no object here.
func newObjects(rp string, env []string, from, to string, g gitRun) ([]string, error) {
	root, err := g(rp, env, nil, "rev-parse", to+"^{tree}")
	if err != nil {
		return nil, err
	}
	out, err := g(rp, env, nil, "diff-tree", "-r", "-t", from, to)
	if err != nil {
		return nil, err
	}
	objs := []string{strings.TrimSpace(string(root)) + " "}
	for _, l := range strings.Split(string(out), "\n") {
		meta, path, ok := strings.Cut(l, "\t")
		f := strings.Fields(meta)
		if !ok || len(f) < 5 || f[1] == "160000" || zero(f[3]) {
			continue
		}
		objs = append(objs, f[3]+" "+path)
	}
	return objs, nil
}

// missingFromChain keeps the objects the chain's packs do not hold, once each.
func missingFromChain(net, rp string, objs []string, g gitRun) ([]string, error) {
	var in bytes.Buffer
	for _, o := range objs {
		in.WriteString(strings.SplitN(o, " ", 2)[0] + "\n")
	}
	env := []string{"GIT_OBJECT_DIRECTORY=" + composeObjects(net), "GIT_ALTERNATE_OBJECT_DIRECTORIES="}
	out, err := g(rp, env, &in, "cat-file", "--batch-check=%(objectname)")
	if err != nil {
		return nil, err
	}
	lines := strings.Split(strings.TrimSpace(string(out)), "\n")
	if len(lines) != len(objs) {
		return nil, fmt.Errorf("cat-file answered %d lines for %d objects", len(lines), len(objs))
	}
	seen := map[string]bool{}
	var keep []string
	for i, l := range lines {
		id := strings.SplitN(objs[i], " ", 2)[0]
		if strings.HasSuffix(l, " missing") && !seen[id] {
			seen[id] = true
			keep = append(keep, objs[i])
		}
	}
	return keep, nil
}

// packTo packs `objs` ("<oid> <path>" lines) into the chain's pack `name`, with no delta search: deltas the
// repository already holds are reused where their base is in the same pack, everything else is whole.
func packTo(net, rp string, env []string, objs []string, name string, m *cpuMeter) error {
	dir := filepath.Join(composeObjects(net), "pack")
	os.MkdirAll(dir, 0o755)
	out, err := m.gitLow(rp, env, strings.NewReader(strings.Join(objs, "\n")+"\n"), "pack-objects", "-q",
		"--delta-base-offset", "--window=0", "--threads=1", filepath.Join(dir, "tmp-build"))
	if err != nil {
		return err
	}
	sum := strings.TrimSpace(string(out))
	for _, ext := range []string{"pack", "idx", "rev"} {
		from := filepath.Join(dir, "tmp-build-"+sum+"."+ext)
		if _, err := os.Stat(from); err != nil {
			continue
		}
		if err := os.Rename(from, chainFile(net, name, ext)); err != nil {
			return err
		}
	}
	return nil
}

func handleCompose(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	mu := composeLock(net)
	mu.Lock()
	defer mu.Unlock()
	h, have := readHead(net)
	if !have && r.URL.Query().Get("start") != "1" {
		jsonOut(w, map[string]any{"head": nil, "built": []string{}})
		return
	}
	var m cpuMeter
	tip := r.URL.Query().Get("tip")
	if tip == "" {
		out, err := m.gitLow(rp, nil, nil, "rev-parse", "-q", "--verify", "HEAD^{commit}")
		if err != nil {
			jsonOut(w, map[string]any{"head": nil, "built": []string{}, "why": "no HEAD"})
			return
		}
		tip = strings.TrimSpace(string(out))
	}
	if !hexRe.MatchString(tip) {
		fail(w, 400, "bad tip")
		return
	}
	if have && h.tip() == tip {
		jsonOut(w, map[string]any{"head": h, "built": []string{}, "cpuMs": 0})
		return
	}
	treeOut, err := m.gitLow(rp, nil, nil, "rev-parse", tip+"^{tree}")
	if err != nil {
		fail(w, 422, "%v", err)
		return
	}
	tree := strings.TrimSpace(string(treeOut))
	os.MkdirAll(filepath.Join(composeObjects(net), "pack"), 0o755)
	env := append(chainFirst(net, rp), derivedEnv)
	var next chainHead
	var name string
	// A part is the new tip's diff from the chain's tree. Past composeDiffMax entries the chain is on a tip far from
	// HEAD, so the step builds a new base instead: on B0-34 m1b34 a base built on a one-file row push's tip (HEAD fell
	// to the first ref after a rehydrate skipped main) took 88,000-entry parts at every move, and every miss after it
	// went over the cap to stock (tests/am1/results/b034-attr.md, "Correction").
	var parts []string
	base := !have || len(h.Parts)+1 >= foldEvery
	if !base {
		objs, err := newObjects(rp, env, h.Tree, tip, m.gitLow)
		if err != nil {
			fail(w, 500, "%v", err)
			return
		}
		if len(objs) > composeDiffMax {
			base = true
		} else {
			parts = objs
		}
	}
	if base {
		// A base: every tree and blob of the tip, from the chain's packs and the repository.
		objs, err := m.gitLow(rp, env, nil, "rev-list", "--objects", tree)
		if err != nil {
			fail(w, 500, "%v", err)
			return
		}
		lines := strings.Split(strings.TrimSpace(string(objs)), "\n")
		next, name = chainHead{Base: tip, Parts: []string{}, Tree: tree}, "base-"+tip
		err = packTo(net, rp, env, lines, name, &m)
		if err != nil {
			fail(w, 500, "%v", err)
			return
		}
	} else {
		objs, err := missingFromChain(net, rp, parts, m.gitLow)
		if err == nil {
			name = "part-" + h.tip() + "-" + tip
			err = packTo(net, rp, env, objs, name, &m)
		}
		if err != nil {
			fail(w, 500, "%v", err)
			return
		}
		next = chainHead{Base: h.Base, Parts: append(append([]string{}, h.Parts...), tip), Tree: tree}
	}
	if err := writeHead(net, next); err != nil {
		fail(w, 500, "%v", err)
		return
	}
	dropUnnamed(net, next)
	jsonOut(w, map[string]any{"head": next, "built": []string{name + ".pack", name + ".idx"}, "cpuMs": ms(m.d)})
}

func handleComposeHead(w http.ResponseWriter, r *http.Request) {
	net, _, ok := netParam(w, r, true)
	if !ok {
		return
	}
	if r.Method == http.MethodGet {
		if h, ok := readHead(net); ok {
			jsonOut(w, h)
		} else {
			jsonOut(w, nil)
		}
		return
	}
	h := chainHead{Parts: []string{}}
	if err := json.NewDecoder(r.Body).Decode(&h); err != nil || !hexRe.MatchString(h.Base) || !hexRe.MatchString(h.Tree) {
		fail(w, 400, "bad head")
		return
	}
	mu := composeLock(net)
	mu.Lock()
	defer mu.Unlock()
	in := filepath.Join(composeDir(net), "in")
	var missing []string
	for _, p := range h.packs() {
		if _, err := os.Stat(chainFile(net, p, "idx")); err == nil {
			continue
		}
		pack, idx := filepath.Join(in, p+".pack"), filepath.Join(in, p+".idx")
		if _, state, why := stagedIdx(pack, idx); state != "used" {
			missing = append(missing, p+" "+state+" "+why)
		}
	}
	if len(missing) > 0 {
		w.Header().Set("content-type", "application/json")
		w.WriteHeader(409)
		json.NewEncoder(w).Encode(map[string]any{"missing": missing})
		return
	}
	os.MkdirAll(filepath.Join(composeObjects(net), "pack"), 0o755)
	for _, p := range h.packs() {
		for _, ext := range []string{"pack", "idx"} {
			if from := filepath.Join(in, p+"."+ext); fileExists(from) {
				if err := os.Rename(from, chainFile(net, p, ext)); err != nil {
					fail(w, 500, "%v", err)
					return
				}
			}
		}
	}
	if err := writeHead(net, h); err != nil {
		fail(w, 500, "%v", err)
		return
	}
	dropUnnamed(net, h)
	os.RemoveAll(in)
	jsonOut(w, h)
}

func fileExists(p string) bool { _, err := os.Stat(p); return err == nil }

func handleComposeFile(w http.ResponseWriter, r *http.Request) {
	net, _, ok := netParam(w, r, true)
	if !ok {
		return
	}
	name := r.URL.Query().Get("name")
	m := fileRe.FindStringSubmatch(name)
	if m == nil {
		fail(w, 400, "bad name")
		return
	}
	if r.Method == http.MethodGet {
		serveFile(w, chainFile(net, m[1], m[2]))
		return
	}
	in := filepath.Join(composeDir(net), "in")
	os.MkdirAll(in, 0o755)
	f, err := os.CreateTemp(in, "tmp-*")
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	n, err := io.CopyBuffer(f, r.Body, make([]byte, 1<<20))
	f.Close()
	if err == nil {
		err = os.Rename(f.Name(), filepath.Join(in, name))
	}
	if err != nil {
		os.Remove(f.Name())
		fail(w, 500, "%v", err)
		return
	}
	jsonOut(w, map[string]any{"bytes": n})
}

// serveFile answers a file with its length, which an R2 put of the body needs.
func serveFile(w http.ResponseWriter, p string) {
	f, err := os.Open(p)
	if err != nil {
		fail(w, 404, "no such file")
		return
	}
	defer f.Close()
	st, _ := f.Stat()
	w.Header().Set("content-type", "application/octet-stream")
	w.Header().Set("content-length", strconv.FormatInt(st.Size(), 10))
	io.CopyBuffer(w, f, make([]byte, 1<<20))
}

// ---- the request ----

// composable is a protocol v2 fetch the chain answers: one want, `deepen 1`, `done`, `ofs-delta`, and otherwise only
// `thin-pack`, `no-progress`, and `include-tag`, in SHA-1. Anything else (a have, a client shallow, a filter, another
// deepen) goes to stock upload-pack. `crates/git` `compose::composable` is the same rule, which only decides whether
// the shard object readies the chain first.
func composable(body []byte) (want string, includeTag bool, ok bool) {
	var fetch, depth1, done, ofs, args bool
	for len(body) >= 4 {
		n, err := strconv.ParseUint(string(body[:4]), 16, 16)
		if err != nil {
			return "", false, false
		}
		if n < 4 {
			if n == 1 {
				args = true
			}
			body = body[4:]
			continue
		}
		if int(n) > len(body) {
			return "", false, false
		}
		l := strings.TrimSuffix(string(body[4:n]), "\n")
		body = body[n:]
		switch {
		case !args && l == "command=fetch":
			fetch = true
		case !args && strings.HasPrefix(l, "object-format="):
			if l != "object-format=sha1" {
				return "", false, false
			}
		case !args && (strings.HasPrefix(l, "command=")):
			return "", false, false
		case !args:
		case strings.HasPrefix(l, "want "):
			// git clone sends HEAD's want and its branch's, the same id twice.
			if (want != "" && want != l[5:]) || !hexRe.MatchString(l[5:]) || len(l[5:]) != 40 {
				return "", false, false
			}
			want = l[5:]
		case l == "deepen 1":
			depth1 = true
		case l == "done":
			done = true
		case l == "ofs-delta":
			ofs = true
		case l == "include-tag":
			includeTag = true
		case l == "thin-pack" || l == "no-progress":
		default:
			return "", false, false
		}
	}
	return want, includeTag, fetch && depth1 && done && ofs && want != ""
}

// ---- the composed answer ----

type chainPack struct {
	f    *os.File
	body int64
	n    uint32
}

// composeMiss answers a composable fetch from the chain, or says why not; it writes nothing when it answers false.
// The caller holds the network's lock shared.
func composeMiss(w http.ResponseWriter, out io.Writer, net, rp, want string, includeTag bool) (bool, string) {
	h, ok := readHead(net)
	if !ok {
		return false, "no chain"
	}
	var m cpuMeter
	// The chain need not be at HEAD: a want past its tip, HEAD after moves whose parts are not built or not fetched yet,
	// is the chain plus the want's diff from the chain tip, within composeDiffMax and packed with no delta search. Held
	// to HEAD, every miss on a shard whose chain lagged the tip went to stock, at a time to first byte of 2 to 209 s
	// (LM-1 round 26, 9 of 51 misses).
	// A want is a ref's tip or behind HEAD, as stock upload-pack's allowReachableSHA1InWant serves it on main. A clone
	// that read main's tip just before a move wants the old tip: 6 of round 26's misses went to stock for that.
	refs, err := m.git(rp, nil, nil, "for-each-ref", "--points-at="+want, "--format=%(refname) %(objecttype)")
	if err != nil {
		return false, "not a ref tip"
	}
	if len(bytes.TrimSpace(refs)) == 0 {
		if _, err := m.git(rp, nil, nil, "merge-base", "--is-ancestor", want, "HEAD"); err != nil {
			return false, "not a ref tip or behind HEAD"
		}
	}
	if includeTag && annotatedTag.Match(refs) {
		return false, "an annotated tag points at it"
	}
	if t, err := m.git(rp, nil, nil, "cat-file", "-t", want); err != nil || strings.TrimSpace(string(t)) != "commit" {
		return false, "not a commit"
	}
	// Open every pack first: a fold that lands meanwhile deletes them, and open files stay readable.
	var packs []chainPack
	defer func() {
		for _, p := range packs {
			p.f.Close()
		}
	}()
	for _, p := range h.packs() {
		f, err := os.Open(chainFile(net, p, "pack"))
		if err != nil {
			return false, "chain pack missing"
		}
		st, _ := f.Stat()
		head := make([]byte, 12)
		if _, err := f.ReadAt(head, 0); err != nil || st.Size() < 32 || string(head[:4]) != "PACK" {
			f.Close()
			return false, "chain pack unreadable"
		}
		packs = append(packs, chainPack{f, st.Size() - 32, binary.BigEndian.Uint32(head[8:])})
	}
	env := inCompose(net)
	objs, err := newObjects(rp, env, h.Tree, want, m.git)
	if err != nil {
		return false, "diff: " + err.Error()
	}
	if len(objs) > composeDiffMax {
		return false, "over the diff cap"
	}
	objs, err = missingFromChain(net, rp, append([]string{want + " "}, objs...), m.git)
	if err != nil {
		return false, err.Error()
	}
	// No delta search, as for the parts: a delta's base may only be in this pack, and a want behind a lagging chain
	// can bring thousands of objects, which a delta search held to 9 to 19 CPU-s a miss (M0-WP9, B0-34, third pass).
	inc, err := m.git(rp, env, strings.NewReader(strings.Join(objs, "\n")+"\n"), "pack-objects", "--stdout", "-q",
		"--delta-base-offset", "--window=0", "--threads=1")
	if err != nil || len(inc) < 32 {
		return false, "pack of its own objects failed"
	}
	count := binary.BigEndian.Uint32(inc[8:12])
	for _, p := range packs {
		count += p.n
	}
	w.Header().Set("x-graft-compose", "composed")
	bw := bufio.NewWriterSize(out, 1<<16)
	pktLine(bw, "shallow-info\n")
	pktLine(bw, "shallow "+want)
	bw.WriteString("0001")
	pktLine(bw, "packfile\n")
	sb := &sideband{w: bw}
	sum := sha1.New()
	pw := io.MultiWriter(sb, sum)
	hdr := []byte{'P', 'A', 'C', 'K', 0, 0, 0, 2, 0, 0, 0, 0}
	binary.BigEndian.PutUint32(hdr[8:], count)
	pw.Write(hdr)
	sb.flush()
	bw.Flush()
	if fl, ok := w.(http.Flusher); ok {
		fl.Flush()
	}
	for _, p := range packs {
		if _, err := io.Copy(pw, io.NewSectionReader(p.f, 12, p.body)); err != nil {
			return true, "cut: " + err.Error()
		}
	}
	pw.Write(inc[12 : len(inc)-20])
	sb.Write(sum.Sum(nil))
	sb.flush()
	bw.WriteString("0000")
	if err := bw.Flush(); err != nil || sb.err != nil {
		return true, "cut"
	}
	return true, ""
}

func pktLine(w io.Writer, s string) { fmt.Fprintf(w, "%04x%s", len(s)+4, s) }

// sideband frames the pack as band-1 packets of at most 65515 bytes, as upload-pack's side-band-64k does.
type sideband struct {
	w   io.Writer
	buf []byte
	err error
}

const sidebandMax = 65515

func (s *sideband) Write(p []byte) (int, error) {
	n := len(p)
	for len(p) > 0 {
		k := min(sidebandMax-len(s.buf), len(p))
		s.buf = append(s.buf, p[:k]...)
		p = p[k:]
		if len(s.buf) == sidebandMax {
			s.flush()
		}
	}
	return n, s.err
}

func (s *sideband) flush() {
	if len(s.buf) == 0 || s.err != nil {
		s.buf = s.buf[:0]
		return
	}
	if _, err := fmt.Fprintf(s.w, "%04x\x01", len(s.buf)+5); err != nil {
		s.err = err
	} else if _, err := s.w.Write(s.buf); err != nil {
		s.err = err
	}
	s.buf = s.buf[:0]
}

// ---- the clone-cache fill ----

// A /upload-pack with spool=H writes the whole response to <data>/spool/clone-H.done as it streams it, and goes on
// writing it after the client goes, so the shard object puts it to the clone cache whether or not the client read it
// to its end. B0-34's probe cancels after the first byte, and with the edge's tee no entry was ever written
// (tests/am1/results/lm1-memory.md, round 24c).
var spools sync.Map // id -> chan struct{}, closed when the spool ended

var spoolRe = regexp.MustCompile(`^[0-9a-f]{64}$`)

func spoolPath(id, ext string) string { return filepath.Join(data, "spool", "clone-"+id+ext) }

// startSpool answers the file a response is teed into, or nil when another request is already spooling id.
func startSpool(id string) (*os.File, func(ok bool)) {
	ch := make(chan struct{})
	if _, busy := spools.LoadOrStore(id, ch); busy {
		return nil, nil
	}
	sweepSpools()
	os.MkdirAll(filepath.Join(data, "spool"), 0o755)
	f, err := os.Create(spoolPath(id, ".part"))
	if err != nil {
		spools.Delete(id)
		close(ch)
		return nil, nil
	}
	return f, func(ok bool) {
		f.Close()
		if ok && wholeResponse(spoolPath(id, ".part")) {
			os.Rename(spoolPath(id, ".part"), spoolPath(id, ".done"))
		} else {
			os.Remove(spoolPath(id, ".part"))
		}
		close(ch)
		spools.Delete(id)
	}
}

func sweepSpools() {
	old, _ := filepath.Glob(filepath.Join(data, "spool", "clone-*"))
	for _, f := range old {
		if st, err := os.Stat(f); err == nil && time.Since(st.ModTime()) > time.Hour {
			os.Remove(f)
		}
	}
}

// wholeResponse: the spooled answer ends in a flush and starts with no ERR packet, as the edge's cacheable_response
// asked of the tee.
func wholeResponse(p string) bool {
	f, err := os.Open(p)
	if err != nil {
		return false
	}
	defer f.Close()
	st, _ := f.Stat()
	head, tail := make([]byte, 8), make([]byte, 4)
	if st.Size() < 12 {
		return false
	}
	f.ReadAt(head, 0)
	f.ReadAt(tail, st.Size()-4)
	return string(tail) == "0000" && string(head[4:8]) != "ERR "
}

// tee writes to the spool and to the client; once the client fails it writes only to the spool, and calls gone.
type tee struct {
	file      io.Writer
	client    io.Writer
	clientErr error
	gone      func()
}

func (t *tee) Write(p []byte) (int, error) {
	if _, err := t.file.Write(p); err != nil {
		return 0, err
	}
	if t.clientErr == nil {
		if _, t.clientErr = t.client.Write(p); t.clientErr != nil && t.gone != nil {
			t.gone()
		}
	}
	return len(p), nil
}

// flushOnWrite flushes the client after each write, so the first packets reach it before the whole pack is made. Each
// write and flush must reach the client within clientWriteTimeout, or it fails: then upload-pack's copy stops and
// kills the process, or a spool goes on alone (`tee`), and either way the network's shared lock is released. Without
// it, a caller that stopped reading but kept its connection blocked the write for good: on B0-34 mt3 one stock
// pack-objects each on sites b and c sat 15 minutes at frozen CPU holding the lock, no catch-up stored again, and
// every upload-pack there answered 503 `warming` (`tests/am1/results/b034-mt.md`, `TestStalledCloneReleasesTheLock`).
type flushOnWrite struct{ w http.ResponseWriter }

func (f flushOnWrite) Write(p []byte) (int, error) {
	rc := http.NewResponseController(f.w)
	// A writer without deadlines (a test's) answers ErrNotSupported and writes as before.
	_ = rc.SetWriteDeadline(time.Now().Add(clientWriteTimeout))
	n, err := f.w.Write(p)
	if err != nil {
		return n, err
	}
	if err := rc.Flush(); err != nil && !errors.Is(err, http.ErrNotSupported) {
		return n, err
	}
	return n, nil
}

// clientWriteTimeout: how long one write of an upload-pack answer may wait for its caller [guess: a 64 KiB write to a
// reading caller takes milliseconds; the shard object and the edge read on as the client does]. A var for the tests.
var clientWriteTimeout = 30 * time.Second

func handleSpooled(w http.ResponseWriter, r *http.Request) {
	id := r.URL.Query().Get("id")
	if !spoolRe.MatchString(id) {
		fail(w, 400, "bad id")
		return
	}
	if r.Method == http.MethodDelete {
		os.Remove(spoolPath(id, ".done"))
		jsonOut(w, map[string]any{"deleted": id})
		return
	}
	if ch, ok := spools.Load(id); ok {
		select {
		case <-ch.(chan struct{}):
		case <-r.Context().Done():
			return
		case <-time.After(15 * time.Minute):
			fail(w, 503, "spool %s still running", id)
			return
		}
	}
	serveFile(w, spoolPath(id, ".done"))
}

var annotatedTag = regexp.MustCompile(`(?m)^refs/tags/\S+ tag$`)
