// The git cache shard's container server (design/DESIGN.md, Shape, "Git cache shards (derived)"). Its shard object
// reaches it only through getTcpPort(8080), and it reaches nothing: the container starts with enableInternet false, so
// every byte comes in on a request from its object. It holds nothing canonical. Each network is a bare repository
// with reftable at /data/<net>.git, rebuilt by its object's rehydrate.
//
//	GET  /health                     {"disk","git","region","location"}: disk is a random id written at first start,
//	                                 so the object can tell a new disk (an emptied container) from the one its
//	                                 IngestCursor describes; region is CLOUDFLARE_REGION, for the start-time check
//	POST /init?net=N[&fresh=1]       git init --bare --ref-format=reftable; fresh deletes the repository first
//	POST /validate?net=N[&tips=a,b]  body: a pushed pack, possibly thin. index-pack --strict --fix-thin into a
//	     [&tipsn=K]                  quarantine with the repository as alternate, then every tip must be connected
//	                                 against the repository's refs. With tipsn, the body's first K bytes are the tips,
//	                                 one per line, and the pack follows (a 10,000-ref push has too many for a URL).
//	                                 Answers the PackIndex of the pack's bytes: SHA-256, size, objects, trailer. An
//	                                 empty pack (deletes) is 400. The indexed pack is kept aside under its SHA-256 and
//	                                 joins the repository only when /ingest names it after the push committed, so a
//	                                 push that never commits leaves no object a later push could lean on
//	     validated packs older than an hour (PACK_COMMIT_WINDOW) are deleted
//	POST /ingest?net=N&sha256=H      body: a committed pack. A pack /validate kept aside under H moves in without a
//	                                 second index-pack; otherwise verifies H while writing, then index-pack --strict
//	                                 --fix-thin into the repository. 409 when a thin pack's bases are not there yet,
//	                                 422 on a checksum mismatch. Idempotent per H
//	PUT  /refs?net=N                 body: "<oid> <ref>" lines, the whole ref set; refs not listed are deleted
//	GET  /refs?net=N                 the same lines
//	POST /promote?net=N              body: SHA-256s of ingested packs, one per line. Answers one complete (not thin)
//	                                 pack of every object those packs brought
//	GET  /info-refs?net=N            upload-pack's advertisement, in the protocol the git-protocol header asks for
//	POST /upload-pack?net=N          upload-pack --stateless-rpc, streamed
//	POST /fsck?net=N                 git fsck --strict, refs, and object counts
//	POST /merge?net=N&base=B&head=H  the test merge of a pull request, in a quarantine: {"conflict": [paths]}, or
//	                                 {"merge": oid, "pack": base64} once the tree-diff check passed, the pack holding
//	                                 only the merge's new objects. The repository is not changed
//	GET  /repos                      the LRU's view of the disk: {"disk": bytes the filesystem holds (or
//	                                 GRAFT_DISK_BYTES), "repos": [{"network","bytes","used"}]}, where used is the
//	                                 last request's time in ms, kept as <repo>/graft-used's mtime
//	POST /evict?net=N                deletes the repository; 409 while a request holds it, so a clone streaming
//	                                 from it is never cut
package main

import (
	"bufio"
	"bytes"
	"compress/gzip"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"syscall"
	"time"
)

// The disk the repositories live on. GRAFT_DATA overrides it for the server's tests.
var data = envOr("GRAFT_DATA", "/data")

func envOr(k, d string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return d
}

var (
	netRe = regexp.MustCompile(`^[0-9]{1,19}$`)
	hexRe = regexp.MustCompile(`^[0-9a-f]{40}([0-9a-f]{24})?$`)
	sumRe = regexp.MustCompile(`^[0-9a-f]{64}$`)
	// One lock per network: git's own locking covers refs and packs, but a fresh init must not race an ingest.
	// Readers (upload-pack, fsck) hold it shared, so /evict, which needs it alone, never removes a repository
	// under a running request.
	locks sync.Map
)

func lock(net string) *sync.RWMutex {
	m, _ := locks.LoadOrStore(net, &sync.RWMutex{})
	return m.(*sync.RWMutex)
}

func repo(net string) string { return filepath.Join(data, net+".git") }

func fail(w http.ResponseWriter, code int, format string, a ...any) {
	http.Error(w, fmt.Sprintf(format, a...), code)
}

func jsonOut(w http.ResponseWriter, v any) {
	w.Header().Set("content-type", "application/json")
	json.NewEncoder(w).Encode(v)
}

// ident is the identity every git command runs with, so git never derives one from the host. On Cloudflare Containers
// a `symbolic-ref HEAD` write took 5 s and 0 ms locally; with ident set and HEAD written only when it changes, PUT
// /refs took 11 ms there. Which of the two removed the wait is not isolated.
var ident = []string{"GIT_COMMITTER_NAME=graftyard-shard", "GIT_COMMITTER_EMAIL=shard@graftyard.invalid",
	"GIT_AUTHOR_NAME=graftyard-shard", "GIT_AUTHOR_EMAIL=shard@graftyard.invalid"}

func git(dir string, env []string, stdin io.Reader, args ...string) ([]byte, error) {
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	cmd.Env = append(append(os.Environ(), ident...), env...)
	cmd.Stdin = stdin
	var out, errb bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errb
	if err := cmd.Run(); err != nil {
		return out.Bytes(), fmt.Errorf("git %s: %v: %s", args[0], err, strings.TrimSpace(errb.String()))
	}
	return out.Bytes(), nil
}

func diskID() string {
	p := filepath.Join(data, "disk-id")
	if b, err := os.ReadFile(p); err == nil {
		return strings.TrimSpace(string(b))
	}
	b := make([]byte, 16)
	rand.Read(b)
	id := hex.EncodeToString(b)
	os.MkdirAll(data, 0o755)
	os.WriteFile(p, []byte(id), 0o644)
	return id
}

// netParam returns the network and its repository, or answers 400.
func netParam(w http.ResponseWriter, r *http.Request, mustExist bool) (string, string, bool) {
	net := r.URL.Query().Get("net")
	if !netRe.MatchString(net) {
		fail(w, 400, "bad net")
		return "", "", false
	}
	if _, err := os.Stat(repo(net)); mustExist && err != nil {
		fail(w, 404, "no repository for network %s", net)
		return "", "", false
	}
	touch(repo(net))
	return net, repo(net), true
}

// touch records a request on the repository for the LRU, as the mtime of <repo>/graft-used.
func touch(rp string) {
	if _, err := os.Stat(rp); err != nil {
		return
	}
	p := filepath.Join(rp, "graft-used")
	now := time.Now()
	if err := os.Chtimes(p, now, now); err != nil {
		os.WriteFile(p, nil, 0o644)
	}
}

func dirBytes(d string) int64 {
	var n int64
	filepath.WalkDir(d, func(_ string, e os.DirEntry, err error) error {
		if err == nil && !e.IsDir() {
			if fi, err := e.Info(); err == nil {
				n += fi.Size()
			}
		}
		return nil
	})
	return n
}

func diskBytes() int64 {
	if v := os.Getenv("GRAFT_DISK_BYTES"); v != "" {
		var n int64
		if _, err := fmt.Sscan(v, &n); err == nil {
			return n
		}
	}
	var st syscall.Statfs_t
	if err := syscall.Statfs(data, &st); err != nil {
		return 0
	}
	return int64(st.Blocks) * int64(st.Bsize)
}

func handleRepos(w http.ResponseWriter, r *http.Request) {
	dirs, _ := filepath.Glob(filepath.Join(data, "*.git"))
	repos := []map[string]any{}
	for _, d := range dirs {
		net := strings.TrimSuffix(filepath.Base(d), ".git")
		if !netRe.MatchString(net) {
			continue
		}
		used := int64(0)
		if fi, err := os.Stat(filepath.Join(d, "graft-used")); err == nil {
			used = fi.ModTime().UnixMilli()
		} else if fi, err := os.Stat(d); err == nil {
			used = fi.ModTime().UnixMilli()
		}
		repos = append(repos, map[string]any{"network": net, "bytes": dirBytes(d), "used": used})
	}
	jsonOut(w, map[string]any{"disk": diskBytes(), "repos": repos})
}

func handleEvict(w http.ResponseWriter, r *http.Request) {
	net := r.URL.Query().Get("net")
	if !netRe.MatchString(net) {
		fail(w, 400, "bad net")
		return
	}
	l := lock(net)
	if !l.TryLock() {
		fail(w, 409, "network %s is in use", net)
		return
	}
	defer l.Unlock()
	rp := repo(net)
	freed := dirBytes(rp)
	if err := os.RemoveAll(rp); err != nil {
		fail(w, 500, "%v", err)
		return
	}
	jsonOut(w, map[string]any{"evicted": net, "bytes": freed})
}

// spool writes the body to a temporary file, hashing it on the way, and returns the file, its SHA-256, and its size.
func spool(dir string, body io.Reader) (string, string, int64, error) {
	os.MkdirAll(dir, 0o755)
	f, err := os.CreateTemp(dir, "in-*.pack")
	if err != nil {
		return "", "", 0, err
	}
	defer f.Close()
	h := sha256.New()
	n, err := io.CopyBuffer(io.MultiWriter(f, h), body, make([]byte, 1<<20))
	if err != nil {
		os.Remove(f.Name())
		return "", "", 0, err
	}
	return f.Name(), hex.EncodeToString(h.Sum(nil)), n, nil
}

var packLine = regexp.MustCompile(`(?m)^(?:pack|keep)\t([0-9a-f]+)$`)

// indexPack runs index-pack --strict --fix-thin on the spooled file into objDir/pack and returns the pack name.
func indexPack(dir string, env []string, file string) (string, error) {
	f, err := os.Open(file)
	if err != nil {
		return "", err
	}
	defer f.Close()
	out, err := git(dir, env, f, "index-pack", "--strict", "--fix-thin", "--stdin")
	if err != nil {
		// A failed index-pack leaves its tmp_pack_* in the object directory; a thin pack retried after its bases
		// arrive would otherwise leave one per attempt.
		objDir := filepath.Join(dir, "objects")
		for _, e := range env {
			if v, ok := strings.CutPrefix(e, "GIT_OBJECT_DIRECTORY="); ok {
				objDir = v
			}
		}
		tmps, _ := filepath.Glob(filepath.Join(objDir, "pack", "tmp_*"))
		for _, t := range tmps {
			os.Remove(t)
		}
		return "", err
	}
	m := packLine.FindSubmatch(out)
	if m == nil {
		return "", fmt.Errorf("index-pack printed %q", out)
	}
	return string(m[1]), nil
}

// idxObjects lists the object ids of an .idx with git show-index, which reads both hash formats.
func idxObjects(idx string) ([]string, error) {
	f, err := os.Open(idx)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	out, err := git(data, nil, f, "show-index")
	if err != nil {
		return nil, err
	}
	var objs []string
	for _, l := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		if f := strings.Fields(l); len(f) >= 2 {
			objs = append(objs, f[1])
		}
	}
	return objs, nil
}

// record keeps, per ingested pack (by the SHA-256 of the bytes as sent), the objects it brought, for /promote.
func record(r, sum, packName string, objDir string) error {
	objs, err := idxObjects(filepath.Join(objDir, "pack", "pack-"+packName+".idx"))
	if err != nil {
		return err
	}
	os.MkdirAll(filepath.Join(r, "graft"), 0o755)
	tmp := filepath.Join(r, "graft", sum+".tmp")
	if err := os.WriteFile(tmp, []byte(strings.Join(objs, "\n")+"\n"), 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, filepath.Join(r, "graft", sum+".objs"))
}

func unresolved(err error) bool {
	s := err.Error()
	return strings.Contains(s, "unresolved delta") || strings.Contains(s, "missing")
}

func handleInit(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, false)
	if !ok {
		return
	}
	l := lock(net)
	l.Lock()
	defer l.Unlock()
	if r.URL.Query().Get("fresh") == "1" {
		os.RemoveAll(rp)
	}
	if _, err := os.Stat(rp); err == nil {
		jsonOut(w, map[string]any{"created": false})
		return
	}
	if _, err := git(data, nil, nil, "-c", "init.defaultBranch=main", "init", "-q", "--bare", "--ref-format=reftable", rp); err != nil {
		fail(w, 500, "%v", err)
		return
	}
	// Shards serve wants by tip; GC never runs here, the repository is rebuilt instead.
	git(rp, nil, nil, "config", "gc.auto", "0")
	git(rp, nil, nil, "config", "uploadpack.allowFilter", "true")
	// As github.com advertises over v0 (research/wire-census.md#43), and partial clones' lazy fetches need them.
	git(rp, nil, nil, "config", "uploadpack.allowTipSHA1InWant", "true")
	git(rp, nil, nil, "config", "uploadpack.allowReachableSHA1InWant", "true")
	jsonOut(w, map[string]any{"created": true})
}

func handleValidate(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	var tips []string
	if t := r.URL.Query().Get("tips"); t != "" {
		tips = strings.Split(t, ",")
	}
	body := io.Reader(r.Body)
	if k := r.URL.Query().Get("tipsn"); k != "" {
		var n int64
		if _, err := fmt.Sscan(k, &n); err != nil || n < 0 || n > 64<<20 {
			fail(w, 400, "bad tipsn")
			return
		}
		head := make([]byte, n)
		if _, err := io.ReadFull(r.Body, head); err != nil {
			fail(w, 400, "body shorter than tipsn")
			return
		}
		tips = append(tips, strings.Fields(string(head))...)
	}
	for _, t := range tips {
		if !hexRe.MatchString(t) {
			fail(w, 400, "bad tip %q", t)
			return
		}
	}
	l := lock(net)
	l.Lock()
	defer l.Unlock()
	sweepValidated(rp)
	q, err := os.MkdirTemp(filepath.Join(rp, "graft"), "incoming-")
	if err != nil {
		os.MkdirAll(filepath.Join(rp, "graft"), 0o755)
		if q, err = os.MkdirTemp(filepath.Join(rp, "graft"), "incoming-"); err != nil {
			fail(w, 500, "%v", err)
			return
		}
	}
	keep := false
	defer func() {
		if !keep {
			os.RemoveAll(q)
		}
	}()
	file, sum, n, err := spool(q, body)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	if n == 0 {
		fail(w, 400, "empty pack")
		return
	}
	// Connectivity is checked against the repository's own objects only, which hold just the committed packs.
	env := []string{"GIT_OBJECT_DIRECTORY=" + q, "GIT_ALTERNATE_OBJECT_DIRECTORIES=" + filepath.Join(rp, "objects")}
	os.MkdirAll(filepath.Join(q, "pack"), 0o755)
	name, err := indexPack(rp, env, file)
	if err != nil {
		fail(w, 422, "index-pack: %v", err)
		return
	}
	if len(tips) > 0 {
		// Tips on stdin: a push may name 10,000 of them.
		in := strings.Join(tips, "\n") + "\n"
		if _, err := git(rp, env, strings.NewReader(in), "rev-list", "--objects", "--quiet", "--stdin", "--not", "--all"); err != nil {
			fail(w, 422, "connectivity: %v", err)
			return
		}
	}
	objs, err := idxObjects(filepath.Join(q, "pack", "pack-"+name+".idx"))
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	trailer := ""
	if f, err := os.Open(file); err == nil {
		f.Seek(-20, io.SeekEnd)
		t := make([]byte, 20)
		io.ReadFull(f, t)
		f.Close()
		trailer = hex.EncodeToString(t)
	}
	// Kept aside under the pack's SHA-256 until /ingest names it.
	os.Remove(file)
	dst := filepath.Join(rp, "graft", "validated-"+sum)
	os.RemoveAll(dst)
	if err := os.Rename(q, dst); err != nil {
		fail(w, 500, "%v", err)
		return
	}
	keep = true
	jsonOut(w, map[string]any{"checksum": sum, "bytes": n, "objects": len(objs), "trailer": trailer})
}

// sweepValidated deletes validated packs no commit named within the hour a commit may take (PACK_COMMIT_WINDOW).
func sweepValidated(rp string) {
	dirs, _ := filepath.Glob(filepath.Join(rp, "graft", "validated-*"))
	inc, _ := filepath.Glob(filepath.Join(rp, "graft", "incoming-*"))
	for _, d := range append(dirs, inc...) {
		if st, err := os.Stat(d); err == nil && time.Since(st.ModTime()) > time.Hour {
			os.RemoveAll(d)
		}
	}
}

// adopt moves a pack /validate kept aside under sum into the repository: pack, then rev, then index, as receive-pack
// migrates a quarantine. False when there is none.
func adopt(rp, sum string) (string, bool, error) {
	src := filepath.Join(rp, "graft", "validated-"+sum)
	idx, _ := filepath.Glob(filepath.Join(src, "pack", "pack-*.idx"))
	if len(idx) != 1 {
		return "", false, nil
	}
	name := strings.TrimSuffix(strings.TrimPrefix(filepath.Base(idx[0]), "pack-"), ".idx")
	for _, ext := range []string{".pack", ".rev", ".idx"} {
		from := filepath.Join(src, "pack", "pack-"+name+ext)
		if _, err := os.Stat(from); err == nil {
			if err := os.Rename(from, filepath.Join(rp, "objects", "pack", "pack-"+name+ext)); err != nil {
				return "", true, err
			}
		}
	}
	os.RemoveAll(src)
	return name, true, nil
}

func handleIngest(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	want := r.URL.Query().Get("sha256")
	if !sumRe.MatchString(want) {
		fail(w, 400, "bad sha256")
		return
	}
	if _, err := os.Stat(filepath.Join(rp, "graft", want+".objs")); err == nil {
		io.Copy(io.Discard, r.Body)
		jsonOut(w, map[string]any{"ingested": false})
		return
	}
	l := lock(net)
	l.Lock()
	if name, ok, err := adopt(rp, want); ok {
		if err == nil {
			err = record(rp, want, name, filepath.Join(rp, "objects"))
		}
		l.Unlock()
		io.Copy(io.Discard, r.Body)
		if err != nil {
			fail(w, 500, "%v", err)
			return
		}
		jsonOut(w, map[string]any{"ingested": true, "adopted": true})
		return
	}
	l.Unlock()
	file, sum, n, err := spool(filepath.Join(data, "spool"), r.Body)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	defer os.Remove(file)
	if sum != want {
		fail(w, 422, "checksum %s, want %s", sum, want)
		return
	}
	l.Lock()
	defer l.Unlock()
	name, err := indexPack(rp, nil, file)
	if err != nil {
		if unresolved(err) {
			fail(w, 409, "%v", err)
		} else {
			fail(w, 422, "%v", err)
		}
		return
	}
	if err := record(rp, sum, name, filepath.Join(rp, "objects")); err != nil {
		fail(w, 500, "%v", err)
		return
	}
	jsonOut(w, map[string]any{"ingested": true, "bytes": n})
}

func listRefs(rp string) (map[string]string, error) {
	out, err := git(rp, nil, nil, "for-each-ref", "--format=%(objectname) %(refname)")
	if err != nil {
		return nil, err
	}
	refs := map[string]string{}
	for _, l := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		if f := strings.Fields(l); len(f) == 2 {
			refs[f[1]] = f[0]
		}
	}
	return refs, nil
}

func handleRefs(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	if r.Method == http.MethodGet {
		refs, err := listRefs(rp)
		if err != nil {
			fail(w, 500, "%v", err)
			return
		}
		for ref, oid := range refs {
			fmt.Fprintf(w, "%s %s\n", oid, ref)
		}
		return
	}
	l := lock(net)
	l.Lock()
	defer l.Unlock()
	want := map[string]string{}
	head := ""
	sc := bufio.NewScanner(r.Body)
	for sc.Scan() {
		f := strings.Fields(sc.Text())
		if len(f) != 2 || !hexRe.MatchString(f[0]) || !strings.HasPrefix(f[1], "refs/") || strings.Contains(f[1], "..") {
			fail(w, 400, "bad ref line %q", sc.Text())
			return
		}
		want[f[1]] = f[0]
		if f[1] == "refs/heads/main" || head == "" {
			head = f[1]
		}
	}
	have, err := listRefs(rp)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	// Deletes commit before updates, in their own transaction: a ref set may move a name from a file to a directory
	// (refs/heads/a deleted, refs/heads/a/b created), which one update-ref transaction refuses, with reftable even
	// when the delete comes first.
	var dels, ups strings.Builder
	for ref := range have {
		if _, ok := want[ref]; !ok {
			fmt.Fprintf(&dels, "delete %s\n", ref)
		}
	}
	for ref, oid := range want {
		if have[ref] != oid {
			fmt.Fprintf(&ups, "update %s %s\n", ref, oid)
		}
	}
	for _, script := range []string{dels.String(), ups.String()} {
		if script == "" {
			continue
		}
		if _, err := git(rp, nil, strings.NewReader("start\n"+script+"commit\n"), "update-ref", "--stdin"); err != nil {
			fail(w, 422, "%v", err)
			return
		}
	}
	if cur, _ := git(rp, nil, nil, "symbolic-ref", "HEAD"); head != "" && strings.TrimSpace(string(cur)) != head {
		git(rp, nil, nil, "symbolic-ref", "HEAD", head)
	}
	jsonOut(w, map[string]any{"refs": len(want)})
}

func handlePromote(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	l := lock(net)
	l.RLock()
	defer l.RUnlock()
	seen := map[string]bool{}
	var oids bytes.Buffer
	sc := bufio.NewScanner(r.Body)
	for sc.Scan() {
		sum := strings.TrimSpace(sc.Text())
		if !sumRe.MatchString(sum) {
			fail(w, 400, "bad sha256 %q", sum)
			return
		}
		b, err := os.ReadFile(filepath.Join(rp, "graft", sum+".objs"))
		if err != nil {
			fail(w, 409, "pack %s was not ingested here", sum)
			return
		}
		for _, o := range strings.Fields(string(b)) {
			if !seen[o] {
				seen[o] = true
				oids.WriteString(o + "\n")
			}
		}
	}
	// Exactly these objects, deltas only within the pack, so it is complete.
	out, err := git(rp, nil, &oids, "pack-objects", "--stdout", "-q", "--delta-base-offset")
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	w.Header().Set("x-objects", fmt.Sprint(len(seen)))
	w.Write(out)
}

func upload(w http.ResponseWriter, r *http.Request, advertise bool) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	l := lock(net)
	l.RLock()
	defer l.RUnlock()
	args := []string{"upload-pack", "--stateless-rpc"}
	if advertise {
		args = append(args, "--http-backend-info-refs")
	}
	args = append(args, rp)
	// The client's Git-Protocol header, which the object forwards; git 2.26 and later ask for version 2.
	proto := r.Header.Get("git-protocol")
	cmd := exec.Command("git", args...)
	cmd.Env = append(os.Environ(), "GIT_PROTOCOL="+proto)
	body := io.Reader(r.Body)
	if r.Header.Get("content-encoding") == "gzip" {
		z, err := gzip.NewReader(r.Body)
		if err != nil {
			fail(w, 400, "%v", err)
			return
		}
		body = z
	}
	if advertise {
		body = nil
	}
	cmd.Stdin = body
	stdout, _ := cmd.StdoutPipe()
	var errb bytes.Buffer
	cmd.Stderr = &errb
	if err := cmd.Start(); err != nil {
		fail(w, 500, "%v", err)
		return
	}
	w.Header().Set("content-type", "application/x-git-upload-pack-result")
	if advertise {
		w.Header().Set("content-type", "application/x-git-upload-pack-advertisement")
		if !strings.Contains(proto, "version=2") {
			io.WriteString(w, "001e# service=git-upload-pack\n0000")
		}
	}
	io.CopyBuffer(w, stdout, make([]byte, 64<<10))
	if err := cmd.Wait(); err != nil {
		fmt.Fprintf(os.Stderr, "upload-pack: %v: %s\n", err, errb.String())
	}
}

// changes is `git diff-tree -r` from a to b: each changed path and its entry in b ("mode oid", all zeros if deleted).
func changes(rp string, env []string, a, b string) (map[string]string, error) {
	out, err := git(rp, env, nil, "diff-tree", "-r", "-z", "--no-renames", "--raw", a, b)
	if err != nil {
		return nil, err
	}
	m := map[string]string{}
	f := strings.Split(string(out), "\x00")
	for i := 0; i+1 < len(f); i += 2 {
		// ":<old mode> <new mode> <old oid> <new oid> <status>", then the path.
		h := strings.Fields(strings.TrimPrefix(f[i], ":"))
		if len(h) != 5 {
			return nil, fmt.Errorf("diff-tree printed %q", f[i])
		}
		m[f[i+1]] = h[1] + " " + h[3]
	}
	return m, nil
}

// treeDiffCheck is the merge engine's invariant (research/uptime.md, finding 19): what the merge changes against each
// parent is exactly what the other side changed since the merge base. A path the merge changes against base must be
// one head changed, with head's entry unless base changed it too, and the same the other way round.
func treeDiffCheck(rp string, env []string, base, head, merge string) error {
	mb, err := git(rp, env, nil, "merge-base", base, head)
	if err != nil {
		return err
	}
	o := strings.TrimSpace(string(mb))
	for _, side := range [][2]string{{base, head}, {head, base}} {
		mine, other := side[0], side[1]
		got, err := changes(rp, env, mine, merge)
		if err != nil {
			return err
		}
		theirs, err := changes(rp, env, o, other)
		if err != nil {
			return err
		}
		ours, err := changes(rp, env, o, mine)
		if err != nil {
			return err
		}
		for p, e := range got {
			t, ok := theirs[p]
			if !ok {
				return fmt.Errorf("the merge changes %s against %s, which %s did not change", p, mine, other)
			}
			if _, both := ours[p]; !both && e != t {
				return fmt.Errorf("the merge's %s is not %s's", p, other)
			}
		}
	}
	return nil
}

// handleMerge is the test merge of GitCacheShard::merge: base and head of a pull request, merged with merge-tree into a
// quarantine. A conflict answers its paths. A clean merge is committed with commit-tree, deterministically (head's
// committer time, the shard's identity), passes the tree-diff check, and answers the merge commit and a pack of only
// its new objects, which the PrSync consumer commits in TestMerged. Nothing joins the repository here: the pack comes
// back to every shard through the chain's log.
func handleMerge(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	base, head := r.URL.Query().Get("base"), r.URL.Query().Get("head")
	if !hexRe.MatchString(base) || !hexRe.MatchString(head) {
		fail(w, 400, "bad base or head")
		return
	}
	l := lock(net)
	l.RLock()
	defer l.RUnlock()
	os.MkdirAll(filepath.Join(rp, "graft"), 0o755)
	q, err := os.MkdirTemp(filepath.Join(rp, "graft"), "merge-")
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	defer os.RemoveAll(q)
	env := []string{"GIT_OBJECT_DIRECTORY=" + q, "GIT_ALTERNATE_OBJECT_DIRECTORIES=" + filepath.Join(rp, "objects")}
	for _, c := range []string{base, head} {
		if _, err := git(rp, env, nil, "cat-file", "-e", c+"^{commit}"); err != nil {
			fail(w, 409, "commit %s is not here", c)
			return
		}
	}
	// merge-tree exits 1 on a conflict, with the tree and then the conflicted paths; `git` keeps no exit code.
	cmd := exec.Command("git", "merge-tree", "--write-tree", "--name-only", "-z", "--no-messages", base, head)
	cmd.Dir = rp
	cmd.Env = append(append(os.Environ(), ident...), env...)
	var errb bytes.Buffer
	cmd.Stderr = &errb
	out, err := cmd.Output()
	f := strings.Split(strings.TrimRight(string(out), "\x00"), "\x00")
	if err != nil {
		if cmd.ProcessState != nil && cmd.ProcessState.ExitCode() == 1 && len(f) > 1 {
			jsonOut(w, map[string]any{"conflict": f[1:]})
			return
		}
		fail(w, 422, "merge-tree: %v: %s", err, strings.TrimSpace(errb.String()))
		return
	}
	tree := f[0]
	ct, err := git(rp, env, nil, "log", "-1", "--format=%ct", head)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	date := "@" + strings.TrimSpace(string(ct)) + " +0000"
	cenv := append(env, "GIT_AUTHOR_DATE="+date, "GIT_COMMITTER_DATE="+date)
	msg := fmt.Sprintf("Merge %s into %s\n", head, base)
	mc, err := git(rp, cenv, strings.NewReader(msg), "commit-tree", tree, "-p", base, "-p", head)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	merge := strings.TrimSpace(string(mc))
	if err := treeDiffCheck(rp, env, base, head, merge); err != nil {
		fail(w, 422, "tree-diff check: %v", err)
		return
	}
	objs, err := git(rp, env, nil, "rev-list", "--objects", merge, "--not", base, head)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	var ids bytes.Buffer
	for _, l := range strings.Split(strings.TrimSpace(string(objs)), "\n") {
		if f := strings.Fields(l); len(f) > 0 {
			ids.WriteString(f[0] + "\n")
		}
	}
	pack, err := git(rp, env, &ids, "pack-objects", "--stdout", "-q", "--delta-base-offset")
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	jsonOut(w, map[string]any{"merge": merge, "pack": pack})
}

func handleFsck(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	l := lock(net)
	l.RLock()
	defer l.RUnlock()
	out, err := git(rp, nil, nil, "fsck", "--strict", "--no-progress", "--no-dangling")
	refs, _ := listRefs(rp)
	count, _ := git(rp, nil, nil, "count-objects", "-v")
	res := map[string]any{"ok": err == nil, "out": string(out), "refs": refs, "count": string(count)}
	if err != nil {
		res["err"] = err.Error()
	}
	jsonOut(w, res)
}

func main() {
	if err := http.ListenAndServe(":8080", mux()); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func mux() *http.ServeMux {
	m := http.NewServeMux()
	disk := diskID()
	version, _ := git(data, nil, nil, "version")
	m.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		jsonOut(w, map[string]any{"disk": disk, "git": strings.TrimSpace(string(version)),
			"region": os.Getenv("CLOUDFLARE_REGION"), "location": os.Getenv("CLOUDFLARE_LOCATION")})
	})
	m.HandleFunc("/init", handleInit)
	m.HandleFunc("/validate", handleValidate)
	m.HandleFunc("/ingest", handleIngest)
	m.HandleFunc("/refs", handleRefs)
	m.HandleFunc("/promote", handlePromote)
	m.HandleFunc("/info-refs", func(w http.ResponseWriter, r *http.Request) { upload(w, r, true) })
	m.HandleFunc("/upload-pack", func(w http.ResponseWriter, r *http.Request) { upload(w, r, false) })
	m.HandleFunc("/fsck", handleFsck)
	m.HandleFunc("/merge", handleMerge)
	m.HandleFunc("/repos", handleRepos)
	m.HandleFunc("/evict", handleEvict)
	return m
}
