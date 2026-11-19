package main

import (
	"bytes"
	"context"
	"crypto/sha1"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// Composed clone-cache misses with real git: the chain's base, parts, and fold, the composed answer a client's
// index-pack --strict takes in an empty repository with a tree equal to the wanted commit's, the stock fallbacks, the
// chain copied to another shard through /compose-file and /compose-head, and the spooled answer a cancelled client
// still leaves whole.

// fetchBody is a protocol v2 depth-1 fetch of want, as git clone --depth 1 asks it, with extra args.
func fetchBody(want string, extra ...string) string {
	b := pkt("command=fetch\n") + pkt("object-format=sha1\n") + "0001" + pkt("thin-pack\n") + pkt("no-progress\n") +
		pkt("ofs-delta\n") + pkt("deepen 1\n")
	for _, e := range extra {
		b += pkt(e + "\n")
	}
	return b + pkt("want "+want+"\n") + pkt("done\n") + "0000"
}

func uploadV2(t *testing.T, url, body string) (*http.Response, []byte) {
	t.Helper()
	req, _ := http.NewRequest("POST", url, strings.NewReader(body))
	req.Header.Set("git-protocol", "version=2")
	r, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	b, _ := io.ReadAll(r.Body)
	if r.StatusCode != 200 {
		t.Fatalf("upload-pack: %d %s", r.StatusCode, b)
	}
	return r, b
}

// packOfResponse parses a v2 fetch response: its shallow lines and the pack from band 1.
func packOfResponse(t *testing.T, b []byte) ([]string, []byte) {
	t.Helper()
	var shallow []string
	var pack []byte
	inPack := false
	for len(b) >= 4 {
		var n int
		fmt.Sscanf(string(b[:4]), "%04x", &n)
		if n < 4 {
			b = b[4:]
			continue
		}
		p := b[4:n]
		b = b[n:]
		switch {
		case inPack && p[0] == 1:
			pack = append(pack, p[1:]...)
		case inPack && p[0] == 3:
			t.Fatalf("error band: %s", p[1:])
		case string(p) == "packfile\n":
			inPack = true
		case strings.HasPrefix(string(p), "shallow "):
			shallow = append(shallow, strings.TrimSpace(string(p[8:])))
		}
	}
	if len(b) != 0 || len(pack) == 0 {
		t.Fatalf("response not whole: %d bytes left, %d pack bytes", len(b), len(pack))
	}
	return shallow, pack
}

// cloneInto indexes the answer's pack in an empty repository as a depth-1 clone does, and checks it: index-pack
// --strict, the shallow line, fsck --strict, and the checked-out tree equal to want's.
func cloneInto(t *testing.T, b []byte, want, wantTree string) {
	t.Helper()
	shallow, pack := packOfResponse(t, b)
	if len(shallow) != 1 || shallow[0] != want {
		t.Fatalf("shallow lines %v, want [%s]", shallow, want)
	}
	d := t.TempDir()
	run(t, d, "", "init", "-q", "--bare")
	os.WriteFile(filepath.Join(d, "shallow"), []byte(want+"\n"), 0o644)
	idx := exec.Command("git", "index-pack", "--strict", "--stdin")
	idx.Dir = d
	idx.Stdin = bytes.NewReader(pack)
	if out, err := idx.CombinedOutput(); err != nil {
		t.Fatalf("index-pack --strict: %v %s", err, out)
	}
	run(t, d, "", "update-ref", "refs/heads/main", want)
	if out := run(t, d, "", "fsck", "--strict", "--no-dangling"); out != "" {
		t.Fatalf("fsck: %s", out)
	}
	if got := run(t, d, "", "rev-parse", want+"^{tree}"); got != wantTree {
		t.Fatalf("tree %s, want %s", got, wantTree)
	}
	wt := t.TempDir()
	run(t, d, "", "--work-tree="+wt, "checkout", "-q", "-f", want, "--", ".")
}

func compose(t *testing.T, url, q string) map[string]any {
	t.Helper()
	r, err := http.Post(url+"/compose?net=7"+q, "", nil)
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	var o map[string]any
	b, _ := io.ReadAll(r.Body)
	if r.StatusCode != 200 || json.Unmarshal(b, &o) != nil {
		t.Fatalf("/compose: %d %s", r.StatusCode, b)
	}
	return o
}

// pushTip pushes main to the shard's repository, as an ingest and a ref write do.
func pushTip(t *testing.T, wt string) string {
	t.Helper()
	run(t, wt, "", "push", "-q", "-f", repo("7"), "HEAD:refs/heads/main")
	return run(t, wt, "", "rev-parse", "HEAD")
}

func TestComposedMiss(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	tip := history(t, wt, 40, 60, 3)
	pushTip(t, wt)
	up := s.URL + "/upload-pack?net=7"

	// No chain yet: stock, and the answer says why.
	r, b := uploadV2(t, up, fetchBody(tip))
	if h := r.Header.Get("x-graft-compose"); h != "stock: no chain" {
		t.Fatalf("x-graft-compose %q before any chain", h)
	}
	cloneInto(t, b, tip, run(t, wt, "", "rev-parse", tip+"^{tree}"))
	// /compose without start builds nothing; with it, a base at the tip.
	if o := compose(t, s.URL, ""); o["head"] != nil {
		t.Fatalf("/compose without start built %v", o)
	}
	compose(t, s.URL, "&start=1")

	// Tip moves, each one a part; an asymmetric history: a rename, a deleted file, a new directory, a file that goes
	// back to an earlier version (its blob already in the base), and a subdirectory moved under another.
	moves := []map[string]string{
		{"d0/f0.txt": "changed once\n", "new/a.txt": "a\n"},
		{"d1/f1.txt": "changed twice\n"},
	}
	for _, m := range moves {
		commit(t, wt, m)
		pushTip(t, wt)
		compose(t, s.URL, "")
	}
	run(t, wt, "", "rm", "-q", "d2/f2.txt")
	run(t, wt, "", "mv", "d3", "new/d3")
	run(t, wt, "", "commit", "-qm", "rm and mv")
	pushTip(t, wt)
	o := compose(t, s.URL, "")
	h := o["head"].(map[string]any)
	if parts := h["parts"].([]any); len(parts) != 3 {
		t.Fatalf("parts %v after 3 moves", parts)
	}
	main := run(t, wt, "", "rev-parse", "HEAD")

	// The tip itself, composed: base and three parts and the commit.
	r, b = uploadV2(t, up, fetchBody(main))
	if c := r.Header.Get("x-graft-compose"); c != "composed" {
		t.Fatalf("x-graft-compose %q on the tip", c)
	}
	cloneInto(t, b, main, run(t, wt, "", "rev-parse", "HEAD^{tree}"))

	// A pull request's head on the tip, with a file put back to the base's version and one new blob: composed.
	run(t, wt, "", "checkout", "-q", "-b", "pr")
	commit(t, wt, map[string]string{"d0/f0.txt": run(t, wt, "", "show", tip+":d0/f0.txt") + "\n", "pr.txt": "pr\n"})
	pr := run(t, wt, "", "rev-parse", "HEAD")
	run(t, wt, "", "push", "-q", repo("7"), "HEAD:refs/pull/1/head")
	r, b = uploadV2(t, up, fetchBody(pr))
	if c := r.Header.Get("x-graft-compose"); c != "composed" {
		t.Fatalf("x-graft-compose %q on a pull head", c)
	}
	cloneInto(t, b, pr, run(t, wt, "", "rev-parse", "HEAD^{tree}"))

	// Fallbacks: a want no ref names, a have, no ofs-delta, a filter, include-tag at an annotated tag.
	loose := commit(t, wt, map[string]string{"loose": "l\n"})
	run(t, wt, "", "push", "-q", repo("7"), loose+":refs/heads/loose")
	run(t, repo("7"), "", "update-ref", "-d", "refs/heads/loose")
	for _, c := range []struct {
		name, body, why string
	}{
		{"want no ref names, not behind HEAD", fetchBody(loose), "stock: not a ref tip or behind HEAD"},
		{"filter", fetchBody(main, "filter blob:none"), ""},
		{"have", fetchBody(main, "have "+tip), ""},
		{"no ofs-delta", strings.Replace(fetchBody(main), pkt("ofs-delta\n"), "", 1), ""},
		{"deepen 2", strings.Replace(fetchBody(main), pkt("deepen 1\n"), pkt("deepen 2\n"), 1), ""},
	} {
		r, _ := uploadV2(t, up, c.body)
		if got := r.Header.Get("x-graft-compose"); got != c.why {
			t.Fatalf("%s: x-graft-compose %q, want %q", c.name, got, c.why)
		}
	}
	run(t, wt, "", "checkout", "-q", "main")
	run(t, wt, "", "tag", "-a", "v1", "-m", "v1")
	run(t, wt, "", "push", "-q", repo("7"), "refs/tags/v1")
	if r, _ := uploadV2(t, up, fetchBody(main, "include-tag")); r.Header.Get("x-graft-compose") != "stock: an annotated tag points at it" {
		t.Fatalf("include-tag at an annotated tag: %q", r.Header.Get("x-graft-compose"))
	}
	if r, b := uploadV2(t, up, fetchBody(main)); r.Header.Get("x-graft-compose") != "composed" {
		t.Fatalf("the tag without include-tag: %q", r.Header.Get("x-graft-compose"))
	} else {
		cloneInto(t, b, main, run(t, wt, "", "rev-parse", "HEAD^{tree}"))
	}
	// A chain behind HEAD still composes, from its own tip: two moves with no part, one a rename, and a want at HEAD
	// and one at the move between. Round 26 sent every such miss to stock while the chain lagged the tip.
	commit(t, wt, map[string]string{"after": "x\n", "d0/f0.txt": "lagging\n"})
	mid := pushTip(t, wt)
	run(t, wt, "", "mv", "new", "renamed")
	run(t, wt, "", "commit", "-qm", "rename")
	next := pushTip(t, wt)
	// `mid` is no ref's tip now, but behind HEAD: a clone that read main just before the move. Round 26 sent 6 of
	// those to stock.
	for _, w := range []string{next, mid} {
		if r, b := uploadV2(t, up, fetchBody(w)); r.Header.Get("x-graft-compose") != "composed" {
			t.Fatalf("a chain behind HEAD: %q", r.Header.Get("x-graft-compose"))
		} else {
			cloneInto(t, b, w, run(t, wt, "", "rev-parse", w+"^{tree}"))
		}
	}
	// compose=only, as the shard object asks before a catch-up: composed when the chain answers, 409 and no answer when
	// it would be stock, and the key's later stock fill still spools.
	only := func(body, spool string) *http.Response {
		req, _ := http.NewRequest("POST", up+"&compose=only&spool="+spool, strings.NewReader(body))
		req.Header.Set("git-protocol", "version=2")
		r, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		return r
	}
	if r := only(fetchBody(next), strings.Repeat("a1", 32)); r.StatusCode != 200 || r.Header.Get("x-graft-compose") != "composed" {
		t.Fatalf("compose=only on a composable miss: %d %q", r.StatusCode, r.Header.Get("x-graft-compose"))
	} else {
		b, _ := io.ReadAll(r.Body)
		r.Body.Close()
		cloneInto(t, b, next, run(t, wt, "", "rev-parse", "HEAD^{tree}"))
	}
	for _, body := range []string{fetchBody(loose), fetchBody(next, "filter blob:none")} {
		r := only(body, strings.Repeat("a2", 32))
		b, _ := io.ReadAll(r.Body)
		r.Body.Close()
		if r.StatusCode != 409 || len(b) != 0 {
			t.Fatalf("compose=only that would be stock: %d %q, %d bytes", r.StatusCode, r.Header.Get("x-graft-compose"), len(b))
		}
	}
	if r, _ := uploadV2(t, up+"&spool="+strings.Repeat("a2", 32), fetchBody(loose)); r.Header.Get("x-graft-spool") == "" {
		t.Fatal("after a compose=only 409 the key's stock fill was not spooled")
	}
	// Past the diff cap it is stock.
	composeDiffMax = 2
	if r, b := uploadV2(t, up, fetchBody(next)); r.Header.Get("x-graft-compose") != "stock: over the diff cap" {
		t.Fatalf("over the cap: %q", r.Header.Get("x-graft-compose"))
	} else {
		cloneInto(t, b, next, run(t, wt, "", "rev-parse", "HEAD^{tree}"))
	}
	composeDiffMax = 20000

	// The fold: at foldEvery moves the chain is one base again, and still composes.
	for i := len(h["parts"].([]any)); i < foldEvery+1; i++ {
		commit(t, wt, map[string]string{fmt.Sprintf("d%d/f%d.txt", i%4, i%40): fmt.Sprintf("move %d\n", i)})
		pushTip(t, wt)
		h = compose(t, s.URL, "")["head"].(map[string]any)
	}
	if parts := h["parts"].([]any); len(parts) >= foldEvery {
		t.Fatalf("no fold: %d parts", len(parts))
	}
	if n, _ := filepath.Glob(filepath.Join(composeObjects("7"), "pack", "pack-*.pack")); len(n) != 1+len(h["parts"].([]any)) {
		t.Fatalf("chain files %v for head %v", n, h)
	}
	main = run(t, wt, "", "rev-parse", "HEAD")
	if r, b := uploadV2(t, up, fetchBody(main)); r.Header.Get("x-graft-compose") != "composed" {
		t.Fatalf("after the fold: %q", r.Header.Get("x-graft-compose"))
	} else {
		cloneInto(t, b, main, run(t, wt, "", "rev-parse", "HEAD^{tree}"))
	}
	// git clone --depth 1 itself, over smart HTTP: it asks for HEAD's id twice, with include-tag, and gets the composed
	// answer, which its index-pack and fsck take.
	cl := t.TempDir()
	cmd := exec.Command("git", "-c", "protocol.version=2", "clone", "-q", "--depth", "1", smartHTTP(t, s, "7"), cl)
	cmd.Env = append(os.Environ(), "GIT_TRACE_CURL=1", "GIT_TRACE_CURL_NO_DATA=1")
	if out, err := cmd.CombinedOutput(); err != nil || !strings.Contains(strings.ToLower(string(out)), "x-graft-compose: composed") {
		t.Fatalf("git clone --depth 1: %v, not composed: %s", err, out)
	}
	run(t, cl, "", "fsck", "--strict", "--no-dangling")
	if got := run(t, cl, "", "rev-parse", "HEAD^{tree}"); got != run(t, wt, "", "rev-parse", "HEAD^{tree}") {
		t.Fatalf("clone tree %s", got)
	}

	// Another shard takes the chain through its files: staged files, then the head, which needs every one.
	head := h
	data0 := data
	other := t.TempDir()
	data = other
	call(t, s, "POST", "/init?net=7")
	run(t, wt, "", "push", "-q", repo("7"), "HEAD:refs/heads/main")
	hb, _ := json.Marshal(head)
	req, _ := http.NewRequest("PUT", s.URL+"/compose-head?net=7", bytes.NewReader(hb))
	if r, err := http.DefaultClient.Do(req); err != nil || r.StatusCode != 409 {
		t.Fatalf("a head with no files staged: %v %v", r.StatusCode, err)
	}
	var names []string
	names = append(names, "base-"+head["base"].(string))
	from := head["base"].(string)
	for _, p := range head["parts"].([]any) {
		names = append(names, "part-"+from+"-"+p.(string))
		from = p.(string)
	}
	for _, n := range names {
		for _, ext := range []string{"pack", "idx"} {
			data = data0
			fr, err := http.Get(s.URL + "/compose-file?net=7&name=" + n + "." + ext)
			if err != nil || fr.StatusCode != 200 {
				t.Fatalf("GET %s.%s: %v", n, ext, err)
			}
			body, _ := io.ReadAll(fr.Body)
			fr.Body.Close()
			data = other
			if code, b := putBody(t, s.URL+"/compose-file?net=7&name="+n+"."+ext, body); code != 200 {
				t.Fatalf("PUT %s.%s: %d %s", n, ext, code, b)
			}
		}
	}
	req, _ = http.NewRequest("PUT", s.URL+"/compose-head?net=7", bytes.NewReader(hb))
	if r, err := http.DefaultClient.Do(req); err != nil || r.StatusCode != 200 {
		t.Fatalf("PUT /compose-head: %v %v", r.StatusCode, err)
	}
	if r, b := uploadV2(t, up, fetchBody(main)); r.Header.Get("x-graft-compose") != "composed" {
		t.Fatalf("on the other shard: %q", r.Header.Get("x-graft-compose"))
	} else {
		cloneInto(t, b, main, run(t, wt, "", "rev-parse", "HEAD^{tree}"))
	}
	data = data0
}

// A clone-cache fill (`spool=`) whose client reads one byte and goes still writes the whole answer, which /spooled
// then serves with its length, and a second fill of the same key while one runs is not spooled twice. Before, the
// edge wrote the clone cache only from its tee of a client read to its end, and B0-34's probe cancels after one byte,
// so no entry was ever written (tests/am1/results/lm1-memory.md, round 24c).
func TestCancelledFillStillSpoolsTheWholeAnswer(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	big := make([]byte, 8<<20)
	x := uint32(1)
	for i := range big {
		x ^= x << 13
		x ^= x >> 17
		x ^= x << 5
		big[i] = byte(x)
	}
	tip := commit(t, wt, map[string]string{"big.bin": string(big)})
	pushTip(t, wt)
	id := strings.Repeat("ab", 32)
	for _, composed := range []bool{false, true} {
		if composed {
			compose(t, s.URL, "&start=1")
			id = strings.Repeat("cd", 32)
		}
		ctx, cancel := context.WithCancel(context.Background())
		req, _ := http.NewRequestWithContext(ctx, "POST", s.URL+"/upload-pack?net=7&spool="+id, strings.NewReader(fetchBody(tip)))
		req.Header.Set("git-protocol", "version=2")
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		if resp.Header.Get("x-graft-spool") != id {
			cancel()
			resp.Body.Close()
			t.Fatalf("not spooled: %v", resp.Header)
		}
		// A second fill of the same key while the first runs is answered but not spooled.
		r2, _ := uploadV2(t, s.URL+"/upload-pack?net=7&spool="+id, fetchBody(tip))
		if r2.Header.Get("x-graft-spool") != "" {
			t.Fatal("a second fill of a key spooled while the first ran")
		}
		resp.Body.Read(make([]byte, 1))
		cancel()
		resp.Body.Close()
		deadline := time.Now().Add(10 * time.Second)
		var r *http.Response
		for {
			r, err = http.Get(s.URL + "/spooled?id=" + id)
			if err == nil && r.StatusCode == 200 {
				break
			}
			if time.Now().After(deadline) {
				t.Fatalf("composed=%v: no spooled answer after the client went: %v", composed, r.StatusCode)
			}
			r.Body.Close()
			time.Sleep(50 * time.Millisecond)
		}
		b, _ := io.ReadAll(r.Body)
		r.Body.Close()
		if r.Header.Get("content-length") != fmt.Sprint(len(b)) {
			t.Fatalf("content-length %s for %d bytes", r.Header.Get("content-length"), len(b))
		}
		cloneInto(t, b, tip, run(t, wt, "", "rev-parse", tip+"^{tree}"))
		call(t, s, "DELETE", "/spooled?id="+id)
		if r, _ := http.Get(s.URL + "/spooled?id=" + id); r.StatusCode != 404 {
			t.Fatalf("after DELETE: %d", r.StatusCode)
		}
	}
	// A fill whose upload-pack fails (a want the repository lacks: ERR, exit 128) leaves nothing to store.
	bad := strings.Repeat("ef", 32)
	req, _ := http.NewRequest("POST", s.URL+"/upload-pack?net=7&spool="+bad, strings.NewReader(fetchBody(strings.Repeat("1", 40))))
	req.Header.Set("git-protocol", "version=2")
	r, err := http.DefaultClient.Do(req)
	if err != nil || r.Header.Get("x-graft-spool") != bad {
		t.Fatalf("failed fill not spooled at all: %v %v", err, r.Header)
	}
	io.Copy(io.Discard, r.Body)
	r.Body.Close()
	if r, _ := http.Get(s.URL + "/spooled?id=" + bad); r.StatusCode != 404 {
		t.Fatalf("a failed fill left a spooled answer: %d", r.StatusCode)
	}
}

// The chain's builds are derived work and run at nice 19, as the repack does, so they yield the vCPUs to validates and
// catch-ups (LM-1 round 24c); a composed miss a client waits on runs at normal priority. Seen by a git on PATH ahead of
// the real one that writes its own nice value.
func TestChainBuildsRunAtNice19(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	history(t, wt, 10, 20, 2)
	pushTip(t, wt)
	real, err := exec.LookPath("git")
	if err != nil {
		t.Fatal(err)
	}
	bin, log := t.TempDir(), filepath.Join(t.TempDir(), "nice")
	wrap := "#!/bin/sh\necho \"$(nice) $1\" >> " + log + "\nexec " + real + " \"$@\"\n"
	if err := os.WriteFile(filepath.Join(bin, "git"), []byte(wrap), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	seen := func() []string {
		b, _ := os.ReadFile(log)
		os.Remove(log)
		return strings.Split(strings.TrimSpace(string(b)), "\n")
	}
	niced := func(what string) {
		lines := seen()
		for _, l := range lines {
			if !strings.HasPrefix(l, "19 ") {
				t.Fatalf("%s ran git at nice %q: %q", what, l, lines)
			}
		}
		if !strings.Contains(strings.Join(lines, "\n"), "19 pack-objects") {
			t.Fatalf("%s ran no pack-objects: %q", what, lines)
		}
	}
	// Niced, the build is still on the CPU meter.
	if o := compose(t, s.URL, "&start=1"); o["cpuMs"].(float64) <= 0 {
		t.Fatalf("the base's CPU went unmetered: %v", o["cpuMs"])
	}
	niced("the base")
	commit(t, wt, map[string]string{"d0/f0.txt": "moved\n"})
	main := pushTip(t, wt)
	seen()
	compose(t, s.URL, "")
	niced("a part")
	if r, _ := uploadV2(t, s.URL+"/upload-pack?net=7", fetchBody(main)); r.Header.Get("x-graft-compose") != "composed" {
		t.Fatalf("not composed: %q", r.Header.Get("x-graft-compose"))
	}
	for _, l := range seen() {
		if strings.HasPrefix(l, "19 ") {
			t.Fatalf("the composed miss ran git at nice 19: %q", l)
		}
	}
}

// A composed miss does not wait for the network's lock: behind a stock answer that streams to a slow reader and a
// writer queued after it, it still answers at once. And a spooled stock answer whose client went lets go of the lock
// while it spools the rest, so the writer does not wait for the spool to end. Round 26's composed misses got their
// first byte in batches, 6 to 68 s after they arrived, whenever a stock answer ended (LM-1 round 26).
func TestComposedMissesDoNotQueueBehindTheLock(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	big := make([]byte, 128<<20)
	x := uint32(1)
	for i := range big {
		x ^= x << 13
		x ^= x >> 17
		x ^= x << 5
		big[i] = byte(x)
	}
	tip := commit(t, wt, map[string]string{"big.bin": string(big)})
	pushTip(t, wt)
	compose(t, s.URL, "&start=1")
	// A stock answer (a filter is never composed) whose reader reads one byte and stalls: it holds the lock shared.
	stall := func(q string) (*http.Response, context.CancelFunc) {
		ctx, cancel := context.WithCancel(context.Background())
		req, _ := http.NewRequestWithContext(ctx, "POST", s.URL+"/upload-pack?net=7"+q, strings.NewReader(fetchBody(tip, "filter blob:limit=1g")))
		req.Header.Set("git-protocol", "version=2")
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		resp.Body.Read(make([]byte, 1))
		t.Cleanup(func() { cancel(); resp.Body.Close() })
		return resp, cancel
	}
	resp, cancel := stall("")
	wrote := make(chan time.Duration, 1)
	t0 := time.Now()
	go func() {
		http.Post(s.URL+"/refs?net=7", "", strings.NewReader(tip+" refs/heads/x\n"))
		wrote <- time.Since(t0)
	}()
	time.Sleep(200 * time.Millisecond)
	t1 := time.Now()
	r, b := uploadV2(t, s.URL+"/upload-pack?net=7", fetchBody(tip))
	if d := time.Since(t1); r.Header.Get("x-graft-compose") != "composed" || d > 2*time.Second {
		t.Fatalf("a composed miss behind a stalled stock answer and a queued writer: %q after %v", r.Header.Get("x-graft-compose"), d)
	}
	cloneInto(t, b, tip, run(t, wt, "", "rev-parse", "HEAD^{tree}"))
	cancel()
	resp.Body.Close()
	<-wrote

	// A spooled stock answer whose client goes: the writer gets the lock while the spool still runs.
	resp, cancel = stall("&spool=" + strings.Repeat("ab", 32))
	cancel()
	resp.Body.Close()
	time.Sleep(100 * time.Millisecond)
	t0 = time.Now()
	go func() {
		http.Post(s.URL+"/refs?net=7", "", strings.NewReader(tip+" refs/heads/y\n"))
		wrote <- time.Since(t0)
	}()
	var lockFree time.Duration
	select {
	case lockFree = <-wrote:
	case <-time.After(20 * time.Second):
		t.Fatal("the writer never got the lock")
	}
	spooledAt := time.Now()
	for {
		if r, _ := http.Get(s.URL + "/spooled?id=" + strings.Repeat("ab", 32)); r.StatusCode == 200 {
			r.Body.Close()
			break
		}
		if time.Since(spooledAt) > 30*time.Second {
			t.Fatal("no spooled answer")
		}
		time.Sleep(20 * time.Millisecond)
	}
	// The writer's wait is bounded by the stock answer's own setup, not the spool: held to the spool's end, it waited
	// to within a millisecond of it.
	if spoolEnd := time.Since(t0); spoolEnd-lockFree < spoolEnd/10 {
		t.Fatalf("the writer waited %v, the spool ran %v: the lock was held to the spool's end", lockFree, spoolEnd)
	}
}

// tailAnswer asks compose=only&tail=1 for body and, from its manifest, builds the answer as the edge does under
// edge-compose: the shallow line and packfile header, a pack header of the total count, each chain pack's body read
// from R2 (here the chain file the manifest names, which ready_chain copies byte for byte), the tail, and a SHA-1
// trailer, in side-band packets (the header alone, then the rest at sidebandMax), then a flush. It also checks the manifest's own numbers against those files.
func tailAnswer(t *testing.T, up, body string) (*http.Response, []byte) {
	t.Helper()
	req, _ := http.NewRequest("POST", up+"&compose=only&tail=1", strings.NewReader(body))
	req.Header.Set("git-protocol", "version=2")
	r, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	b, _ := io.ReadAll(r.Body)
	if r.StatusCode != 200 || r.Header.Get("x-graft-compose") != "tail" {
		return r, b
	}
	if r.Header.Get("content-length") != fmt.Sprint(len(b)) {
		t.Fatalf("content-length %s for %d bytes", r.Header.Get("content-length"), len(b))
	}
	line, tail, ok := bytes.Cut(b, []byte("\n"))
	if !ok {
		t.Fatalf("no manifest line in %d bytes", len(b))
	}
	var m struct {
		V     int    `json:"v"`
		Want  string `json:"want"`
		Packs []struct {
			Key string `json:"key"`
			N   uint32 `json:"n"`
			Len int64  `json:"len"`
		} `json:"packs"`
		TailN uint32 `json:"tailN"`
		Tail  int    `json:"tail"`
	}
	if err := json.Unmarshal(line, &m); err != nil || m.V != 1 || len(m.Packs) == 0 || m.Tail != len(tail) {
		t.Fatalf("manifest %s (%v), tail %d bytes", line, err, len(tail))
	}
	var pack bytes.Buffer
	count := m.TailN
	var bodies [][]byte
	for _, p := range m.Packs {
		name, ok := strings.CutPrefix(p.Key, "compose/7/")
		if !ok || !strings.HasSuffix(name, ".pack") {
			t.Fatalf("pack key %q", p.Key)
		}
		f, err := os.ReadFile(filepath.Join(composeObjects("7"), "pack", "pack-"+name))
		if err != nil {
			t.Fatal(err)
		}
		if binary.BigEndian.Uint32(f[8:12]) != p.N || int64(len(f)-32) != p.Len {
			t.Fatalf("%s: manifest n %d len %d, file n %d len %d", name, p.N, p.Len, binary.BigEndian.Uint32(f[8:12]), len(f)-32)
		}
		count += p.N
		bodies = append(bodies, f[12:len(f)-20])
	}
	hdr := []byte{'P', 'A', 'C', 'K', 0, 0, 0, 2, 0, 0, 0, 0}
	binary.BigEndian.PutUint32(hdr[8:], count)
	pack.Write(hdr)
	for _, b := range bodies {
		pack.Write(b)
	}
	pack.Write(tail)
	sum := sha1.Sum(pack.Bytes())
	pack.Write(sum[:])
	var out bytes.Buffer
	out.WriteString(pkt("shallow-info\n") + pkt("shallow "+m.Want) + "0001" + pkt("packfile\n"))
	// The header is its own packet, as composeMiss flushes it before the bodies so the client's first byte comes early;
	// the rest goes in packets of sidebandMax.
	out.WriteString(hex4(12+5) + "\x01")
	out.Write(pack.Bytes()[:12])
	for p := pack.Bytes()[12:]; len(p) > 0; {
		k := min(len(p), sidebandMax)
		out.WriteString(hex4(k + 5))
		out.WriteByte(1)
		out.Write(p[:k])
		p = p[k:]
	}
	out.WriteString("0000")
	return r, out.Bytes()
}

// compose=only&tail=1 (edge-compose): the manifest and tail a client gets once the edge streams the chain from R2 are
// the same pack, object for object, as the container's composed answer, and a client's index-pack --strict takes it
// with the wanted commit's tree: on a base alone, across parts, on a want behind a lagging chain, and after a fold.
// Whatever compose=only answers 409 to, tail=1 does too, and a tail past tailMax is composed whole as before.
func TestComposedTailMatchesTheComposedAnswer(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	history(t, wt, 40, 60, 3)
	pushTip(t, wt)
	up := s.URL + "/upload-pack?net=7"
	tree := func(c string) string { return run(t, wt, "", "rev-parse", c+"^{tree}") }
	same := func(stage, want string) {
		t.Helper()
		r, b := tailAnswer(t, up, fetchBody(want))
		if r.Header.Get("x-graft-compose") != "tail" {
			t.Fatalf("%s: tail=1 answered %d %q", stage, r.StatusCode, r.Header.Get("x-graft-compose"))
		}
		rc, c := uploadV2(t, up, fetchBody(want))
		if rc.Header.Get("x-graft-compose") != "composed" {
			t.Fatalf("%s: composed answer %q", stage, rc.Header.Get("x-graft-compose"))
		}
		if !bytes.Equal(b, c) {
			t.Fatalf("%s: the edge's answer (%d bytes) differs from the composed one (%d bytes)", stage, len(b), len(c))
		}
		cloneInto(t, b, want, tree(want))
	}
	// No chain: 409 with compose=only's reason, and no body.
	r, b := tailAnswer(t, up, fetchBody(run(t, wt, "", "rev-parse", "HEAD")))
	if r.StatusCode != 409 || r.Header.Get("x-graft-compose") != "stock: no chain" || len(b) != 0 {
		t.Fatalf("no chain: %d %q, %d bytes", r.StatusCode, r.Header.Get("x-graft-compose"), len(b))
	}
	compose(t, s.URL, "&start=1")
	same("base alone", run(t, wt, "", "rev-parse", "HEAD"))
	for i, m := range []map[string]string{{"d0/f0.txt": "one\n", "new/a.txt": "a\n"}, {"d1/f1.txt": "two\n"}} {
		commit(t, wt, m)
		pushTip(t, wt)
		compose(t, s.URL, "")
		if i == 0 {
			same("one part", run(t, wt, "", "rev-parse", "HEAD"))
		}
	}
	same("two parts", run(t, wt, "", "rev-parse", "HEAD"))
	// A want past a lagging chain, with a rename: its own objects are the whole difference.
	run(t, wt, "", "mv", "new", "renamed")
	run(t, wt, "", "commit", "-qm", "rename")
	pushed := pushTip(t, wt)
	same("past the chain", pushed)
	// Not composable, or not on the chain's terms: 409 with the same reason compose=only gives.
	loose := commit(t, wt, map[string]string{"loose": "l\n"})
	for _, c := range []struct{ name, body, why string }{
		{"a filter", fetchBody(loose, "filter blob:none"), "stock: not composable"},
		{"a want no ref names", fetchBody(loose), "stock: not a ref tip or behind HEAD"},
	} {
		r, b := tailAnswer(t, up, c.body)
		if r.StatusCode != 409 || r.Header.Get("x-graft-compose") != c.why || len(b) != 0 {
			t.Fatalf("%s: %d %q, %d bytes", c.name, r.StatusCode, r.Header.Get("x-graft-compose"), len(b))
		}
	}
	// A tail past tailMax: the composed answer, whole, as compose=only gives it.
	head := pushed
	tailMax = 10
	r, b = tailAnswer(t, up, fetchBody(head))
	tailMax = 8 << 20
	if r.StatusCode != 200 || r.Header.Get("x-graft-compose") != "composed" {
		t.Fatalf("a tail past tailMax: %d %q", r.StatusCode, r.Header.Get("x-graft-compose"))
	}
	cloneInto(t, b, head, tree(head))
	// The fold: one base again, still the same answer.
	for i := 0; i < foldEvery+1; i++ {
		commit(t, wt, map[string]string{fmt.Sprintf("d%d/f%d.txt", i%4, i%40): fmt.Sprintf("fold %d\n", i)})
		pushTip(t, wt)
		compose(t, s.URL, "")
	}
	if n, _ := filepath.Glob(filepath.Join(composeObjects("7"), "pack", "pack-base-*.pack")); len(n) != 1 {
		t.Fatalf("bases after the fold: %v", n)
	}
	same("after the fold", run(t, wt, "", "rev-parse", "HEAD"))
}

// With EDGEBED_OUT set, writes a chain and its answers for tests/composed/edgebed/run.sh: chain/<file>.pack for each
// pack, manifest-<stage> (the compose=only&tail=1 answer), composed-<stage> (the container's composed answer), want-<stage>
// and tree-<stage>, at stages base, parts (two parts, a want past the chain), and fold. The chain's files are a large
// random blob, so the composed answer spans many side-band packets and R2 reads several chunks per pack.
func TestWriteEdgeBedChain(t *testing.T) {
	out := os.Getenv("EDGEBED_OUT")
	if out == "" {
		t.Skip("EDGEBED_OUT not set")
	}
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	history(t, wt, 40, 60, 3)
	big := make([]byte, 3<<20)
	x := uint32(7)
	for i := range big {
		x ^= x << 13
		x ^= x >> 17
		x ^= x << 5
		big[i] = byte(x)
	}
	commit(t, wt, map[string]string{"big.bin": string(big)})
	pushTip(t, wt)
	up := s.URL + "/upload-pack?net=7"
	os.MkdirAll(filepath.Join(out, "chain"), 0o755)
	stage := func(name, want string) {
		req, _ := http.NewRequest("POST", up+"&compose=only&tail=1", strings.NewReader(fetchBody(want)))
		req.Header.Set("git-protocol", "version=2")
		r, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		m, _ := io.ReadAll(r.Body)
		r.Body.Close()
		if r.Header.Get("x-graft-compose") != "tail" {
			t.Fatalf("%s: %d %q", name, r.StatusCode, r.Header.Get("x-graft-compose"))
		}
		_, c := uploadV2(t, up, fetchBody(want))
		files, _ := filepath.Glob(filepath.Join(composeObjects("7"), "pack", "pack-*.pack"))
		for _, f := range files {
			b, _ := os.ReadFile(f)
			os.WriteFile(filepath.Join(out, "chain", strings.TrimPrefix(filepath.Base(f), "pack-")), b, 0o644)
		}
		for k, v := range map[string][]byte{"manifest-": m, "composed-": c, "want-": []byte(want),
			"tree-": []byte(run(t, wt, "", "rev-parse", want+"^{tree}"))} {
			os.WriteFile(filepath.Join(out, k+name), v, 0o644)
		}
	}
	compose(t, s.URL, "&start=1")
	stage("base", run(t, wt, "", "rev-parse", "HEAD"))
	for _, m := range []map[string]string{{"d0/f0.txt": "one\n"}, {"d1/f1.txt": "two\n", "new/a": "a\n"}} {
		commit(t, wt, m)
		pushTip(t, wt)
		compose(t, s.URL, "")
	}
	commit(t, wt, map[string]string{"past.txt": "past the chain\n"})
	stage("parts", pushTip(t, wt))
	for i := 0; i < foldEvery+1; i++ {
		commit(t, wt, map[string]string{fmt.Sprintf("d%d/f%d.txt", i%4, i%40): fmt.Sprintf("fold %d\n", i)})
		pushTip(t, wt)
		compose(t, s.URL, "")
	}
	stage("fold", run(t, wt, "", "rev-parse", "HEAD"))
}
