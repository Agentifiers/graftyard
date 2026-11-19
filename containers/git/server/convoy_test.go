package main

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"net"
	"net/http"
	"os/exec"
	"strings"
	"testing"
	"time"
)

// A request body still arriving holds no network lock. A validate whose pack streams in for seconds, an /ingest
// waiting to take the lock alone, and then an ls-refs: with the lock taken before the body, the waiting writer holds
// every later reader behind the slow body (LM-1 round 6, the home shard's ls-refs walls at 12 s), so the ls-refs here
// answers only after the body ends. A /refs POST whose body is still arriving must not hold the lock either.
func TestValidateBodyHoldsNoLock(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commit(t, wt, map[string]string{"README": "r\n"})
	run(t, wt, "", "push", "-q", repo("7"), "main")
	putRefs(t, s.URL, base)
	mk := func(name string) (string, []byte) {
		run(t, wt, "", "checkout", "-q", "-B", name, base)
		tip := commit(t, wt, map[string]string{name: strings.Repeat(name, 2000) + "\n"})
		return tip, packOfWt(t, wt, tip, base)
	}
	t1, p1 := mk("one")
	code, o, body := validateDetail(t, s.URL, []string{t1}, nil, p1)
	if code != 200 {
		t.Fatalf("validate: %d %s", code, body)
	}
	t2, p2 := mk("two")

	// The slow validate: its tips arrive, then the pack waits on `gate` before its bytes follow.
	gate := make(chan struct{})
	pr, pw := io.Pipe()
	go func() {
		pw.Write([]byte(t2 + "\n"))
		<-gate
		pw.Write(p2)
		pw.Close()
	}()
	slow := make(chan int, 1)
	go func() {
		r, err := http.Post(fmt.Sprintf("%s/validate?net=7&tipsn=%d&class=agent", s.URL, len(t2)+1), "", pr)
		if err != nil {
			slow <- 0
			return
		}
		r.Body.Close()
		slow <- r.StatusCode
	}()
	time.Sleep(200 * time.Millisecond)
	// A writer that needs the lock alone, then readers that arrive after it.
	ingested := make(chan int, 1)
	go func() {
		c, _ := call(t, s, "POST", "/ingest?net=7&sha256="+o.Checksum)
		ingested <- c
	}()
	time.Sleep(100 * time.Millisecond)
	ls := make(chan error, 1)
	go func() {
		pkt := func(l string) string { return fmt.Sprintf("%04x%s", len(l)+4, l) }
		req, _ := http.NewRequest("POST", s.URL+"/upload-pack?net=7",
			strings.NewReader(pkt("command=ls-refs\n")+"0001"+pkt("ref-prefix refs/heads/\n")+"0000"))
		req.Header.Set("git-protocol", "version=2")
		r, err := http.DefaultClient.Do(req)
		if err != nil {
			ls <- err
			return
		}
		b, _ := io.ReadAll(r.Body)
		r.Body.Close()
		if !bytes.Contains(b, []byte(base+" refs/heads/main")) {
			ls <- fmt.Errorf("ls-refs answered %q", b)
			return
		}
		ls <- nil
	}()
	select {
	case c := <-ingested:
		if c != 200 {
			t.Fatalf("ingest: %d", c)
		}
	case <-time.After(5 * time.Second):
		close(gate)
		t.Fatal("an /ingest waited on a validate whose body was still arriving")
	}
	select {
	case err := <-ls:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(5 * time.Second):
		close(gate)
		t.Fatal("an ls-refs waited on a validate whose body was still arriving")
	}

	// A /refs POST whose body is still arriving holds the lock from no one.
	rr, rw := io.Pipe()
	refsDone := make(chan int, 1)
	go func() {
		r, err := http.Post(s.URL+"/refs?net=7", "", rr)
		if err != nil {
			refsDone <- 0
			return
		}
		r.Body.Close()
		refsDone <- r.StatusCode
	}()
	time.Sleep(100 * time.Millisecond)
	done := make(chan struct{})
	go func() {
		call(t, s, "POST", "/fsck?net=7")
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		rw.Close()
		close(gate)
		t.Fatal("a reader waited on a /refs whose body was still arriving")
	}
	rw.Write([]byte(t1 + " refs/heads/one\n"))
	rw.Close()
	if c := <-refsDone; c != 200 {
		t.Fatalf("refs: %d", c)
	}

	close(gate)
	if c := <-slow; c != 200 {
		t.Fatalf("slow validate: %d", c)
	}
}

// A clone the client gives up on releases the network's lock. B0-34's time-to-first-byte probe cancels each depth-1
// fetch after its first byte; upload-pack's copy to the gone client failed, nothing read its stdout again, so git
// blocked on the full pipe, `cmd.Wait` never returned, and the shared lock stayed held. The next catch-up's /ingest
// waited to take it alone and every later validate queued behind that writer: all three shards of the monorepo
// answered `warming` from 33 s into the window (`tests/am1/results/lm1-memory.md`, round 18c).
func TestCancelledCloneReleasesTheLock(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	// Incompressible content, well past the pipe's and the sockets' buffers, so upload-pack is still writing when
	// the client goes.
	big := make([]byte, 8<<20)
	r := uint32(1)
	for i := range big {
		r ^= r << 13
		r ^= r >> 17
		r ^= r << 5
		big[i] = byte(r)
	}
	base := commit(t, wt, map[string]string{"big.bin": string(big)})
	run(t, wt, "", "push", "-q", repo("7"), "main")
	putRefs(t, s.URL, base)

	pkt := func(l string) string { return fmt.Sprintf("%04x%s", len(l)+4, l) }
	ctx, cancel := context.WithCancel(context.Background())
	req, _ := http.NewRequestWithContext(ctx, "POST", s.URL+"/upload-pack?net=7", strings.NewReader(
		pkt("command=fetch\n")+"0001"+pkt("no-progress\n")+pkt("want "+base+"\n")+pkt("deepen 1\n")+pkt("done\n")+"0000"))
	req.Header.Set("git-protocol", "version=2")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := resp.Body.Read(make([]byte, 1)); err != nil {
		t.Fatal(err)
	}
	cancel()
	resp.Body.Close()
	time.Sleep(200 * time.Millisecond)

	run(t, wt, "", "checkout", "-q", "-B", "one", base)
	t1 := commit(t, wt, map[string]string{"one": "1\n"})
	p1 := packOfWt(t, wt, t1, base)
	sum := sha256.Sum256(p1)
	run(t, wt, "", "checkout", "-q", "-B", "two", base)
	t2 := commit(t, wt, map[string]string{"two": "2\n"})
	p2 := packOfWt(t, wt, t2, base)
	done := make(chan string, 1)
	go func() {
		if code, body := postBody(t, s.URL+"/ingest?net=7&sha256="+hex.EncodeToString(sum[:]), p1); code != 200 {
			done <- fmt.Sprintf("ingest: %d %s", code, body)
			return
		}
		if code, _, body := validateDetail(t, s.URL, []string{t2}, nil, p2); code != 200 {
			done <- fmt.Sprintf("validate: %d %s", code, body)
			return
		}
		done <- ""
	}()
	select {
	case e := <-done:
		if e != "" {
			t.Fatal(e)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("an /ingest and a validate after a cancelled clone did not finish in 10 s: the clone still holds the lock")
	}

	// The same through the handler alone, with a writer that fails after its first write: it returns and frees the lock.
	req2, _ := http.NewRequest("POST", "/upload-pack?net=7", strings.NewReader(
		pkt("command=fetch\n")+"0001"+pkt("no-progress\n")+pkt("want "+base+"\n")+pkt("deepen 1\n")+pkt("done\n")+"0000"))
	req2.Header.Set("git-protocol", "version=2")
	returned := make(chan struct{})
	go func() {
		mux().ServeHTTP(&failAfter{h: http.Header{}, left: 1}, req2)
		close(returned)
	}()
	select {
	case <-returned:
	case <-time.After(10 * time.Second):
		t.Fatal("upload-pack's handler did not return after its response write failed")
	}
	if !lock("7").TryLock() {
		t.Fatal("the network's lock is still held after upload-pack's write failed")
	}
	lock("7").Unlock()
}

// A clone whose caller stops reading but keeps its connection open releases the network's lock. The write to it blocks
// rather than fails, so before the write deadline upload-pack never ended and its shared lock held every later
// /ingest: on B0-34 mt3 a stock pack-objects each on sites b and c sat 15 minutes at frozen CPU, no catch-up stored
// again on those objects, and every upload-pack there answered 503 `warming` (`tests/am1/results/b034-mt.md`).
func TestStalledCloneReleasesTheLock(t *testing.T) {
	old := clientWriteTimeout
	clientWriteTimeout = 500 * time.Millisecond
	t.Cleanup(func() { clientWriteTimeout = old })
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	// Incompressible and well past the sockets' buffers, so the write blocks once the caller stops reading.
	big := make([]byte, 32<<20)
	r := uint32(1)
	for i := range big {
		r ^= r << 13
		r ^= r >> 17
		r ^= r << 5
		big[i] = byte(r)
	}
	base := commit(t, wt, map[string]string{"big.bin": string(big)})
	run(t, wt, "", "push", "-q", repo("7"), "main")
	putRefs(t, s.URL, base)

	pkt := func(l string) string { return fmt.Sprintf("%04x%s", len(l)+4, l) }
	body := pkt("command=fetch\n") + "0001" + pkt("no-progress\n") + pkt("want "+base+"\n") + pkt("deepen 1\n") +
		pkt("done\n") + "0000"
	c, err := net.Dial("tcp", strings.TrimPrefix(s.URL, "http://"))
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	fmt.Fprintf(c, "POST /upload-pack?net=7 HTTP/1.1\r\nHost: x\r\nGit-Protocol: version=2\r\nContent-Length: %d\r\n\r\n%s",
		len(body), body)
	// One read of the answer's start, then no more: the connection stays open, as a caller that stalled.
	if _, err := c.Read(make([]byte, 64)); err != nil {
		t.Fatal(err)
	}
	if tc, ok := c.(*net.TCPConn); ok {
		tc.SetReadBuffer(4096)
	}

	run(t, wt, "", "checkout", "-q", "-B", "one", base)
	t1 := commit(t, wt, map[string]string{"one": "1\n"})
	p1 := packOfWt(t, wt, t1, base)
	sum := sha256.Sum256(p1)
	done := make(chan string, 1)
	go func() {
		if code, body := postBody(t, s.URL+"/ingest?net=7&sha256="+hex.EncodeToString(sum[:]), p1); code != 200 {
			done <- fmt.Sprintf("ingest: %d %s", code, body)
			return
		}
		done <- ""
	}()
	select {
	case e := <-done:
		if e != "" {
			t.Fatal(e)
		}
	case <-time.After(20 * time.Second):
		t.Fatal("an /ingest behind a stalled clone did not finish in 20 s: the clone still holds the network's lock")
	}
}

// failAfter is a ResponseWriter that takes `left` writes, then fails every one, as a gone client's connection does.
type failAfter struct {
	h    http.Header
	left int
}

func (f *failAfter) Header() http.Header { return f.h }
func (f *failAfter) WriteHeader(int)     {}
func (f *failAfter) Write(p []byte) (int, error) {
	if f.left == 0 {
		return 0, io.ErrClosedPipe
	}
	f.left--
	return len(p), nil
}

// A repack holds no network lock: while one runs (held here at its start), an ls-refs, an /ingest, and a validate of
// the same network answer, and evict refuses. With the lock held alone for the repack, every one of them waited for
// it, seconds once a shard's packs reach 100 MB (LM-1 round 7).
func TestRepackHoldsNoNetworkLock(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commit(t, wt, map[string]string{"README": "r\n"})
	run(t, wt, "", "push", "-q", repo("7"), "main")
	prev := base
	for i := 0; i <= repackAt; i++ {
		tip := commit(t, wt, map[string]string{"f": fmt.Sprintf("%d\n", i)})
		idx := exec.Command("git", "index-pack", "--stdin", "--fix-thin")
		idx.Dir = repo("7")
		idx.Stdin = bytes.NewReader(packOfWt(t, wt, tip, prev))
		if out, err := idx.CombinedOutput(); err != nil {
			t.Fatalf("index-pack: %v %s", err, out)
		}
		prev = tip
	}
	started, release := make(chan struct{}), make(chan struct{})
	old := runRepack
	runRepack = func(rp string) error {
		close(started)
		<-release
		return old(rp)
	}
	defer func() { runRepack = old }()
	putRefs(t, s.URL, prev)
	select {
	case <-started:
	case <-time.After(5 * time.Second):
		t.Fatal("no repack started past repackAt")
	}
	run(t, wt, "", "checkout", "-q", "-B", "side", prev)
	side := commit(t, wt, map[string]string{"side": "s\n"})
	pack := packOfWt(t, wt, side, prev)
	done := make(chan string, 1)
	go func() {
		code, o, body := validateDetail(t, s.URL, []string{side}, nil, pack)
		if code != 200 {
			done <- fmt.Sprintf("validate: %d %s", code, body)
			return
		}
		if c, v := call(t, s, "POST", "/ingest?net=7&sha256="+o.Checksum); c != 200 {
			done <- fmt.Sprintf("ingest: %d %v", c, v)
			return
		}
		if c, _ := call(t, s, "POST", "/evict?net=7"); c != 409 {
			done <- fmt.Sprintf("evict during a repack: %d, want 409", c)
			return
		}
		done <- ""
	}()
	select {
	case e := <-done:
		if e != "" {
			close(release)
			t.Fatal(e)
		}
	case <-time.After(5 * time.Second):
		close(release)
		t.Fatal("a validate or ingest waited on a running repack")
	}
	// fsck waits for the repack, which deletes the packs it rolled up as it ends: one that ran alongside answered ok
	// false on site c just after a rehydrate's repack (tests/am1/results/hot-shards-3.md).
	fsck := make(chan map[string]any, 1)
	go func() {
		_, v := call(t, s, "POST", "/fsck?net=7")
		fsck <- v
	}()
	select {
	case v := <-fsck:
		close(release)
		t.Fatalf("fsck answered while a repack ran: %v", v["ok"])
	case <-time.After(300 * time.Millisecond):
	}
	close(release)
	select {
	case v := <-fsck:
		if v["ok"] != true {
			t.Fatalf("fsck after the repack: %v %v", v["out"], v["err"])
		}
	case <-time.After(10 * time.Second):
		t.Fatal("fsck never answered after the repack ended")
	}
	repacking.Lock() // the repack has finished
	repacking.Unlock()
	if n := packCount(repo("7")); n > 8 {
		t.Fatalf("%d packs after the repack", n)
	}
	if out := run(t, repo("7"), "", "fsck", "--strict", "--no-dangling"); out != "" {
		t.Fatalf("fsck: %s", out)
	}
	run(t, repo("7"), "", "cat-file", "-e", side)
}

// A rehydrate that skipped covered packs ends with its last ref page (repack=0) and then the backstop /connected
// (repack=1). The check answers while a repack, held here at its start, would still be running, and only then does the
// repack start, whether the walk passed or failed. Before, the last ref page started the repack and the check waited
// for all of it: 20 minutes and more of 2.6 GB on B0-34 m33, with the shard answering `warming` throughout
// (tests/am1/results/lm1-memory.md, round 21). A /refs without repack=0 still starts the repack, as a plain
// rehydrate's and a catch-up's do.
func TestConnectedRunsBeforeTheRehydratesRepack(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commit(t, wt, map[string]string{"README": "r\n"})
	run(t, wt, "", "push", "-q", repo("7"), "main")
	prev := base
	for i := 0; i <= repackAt; i++ {
		tip := commit(t, wt, map[string]string{"f": fmt.Sprintf("%d\n", i)})
		idx := exec.Command("git", "index-pack", "--stdin", "--fix-thin")
		idx.Dir = repo("7")
		idx.Stdin = bytes.NewReader(packOfWt(t, wt, tip, prev))
		if out, err := idx.CombinedOutput(); err != nil {
			t.Fatalf("index-pack: %v %s", err, out)
		}
		prev = tip
	}
	started := make(chan struct{}, 4)
	release := make(chan struct{})
	old := runRepack
	runRepack = func(rp string) error {
		started <- struct{}{}
		<-release
		return nil
	}
	defer func() { runRepack = old }()
	defer close(release)
	put := func(q string) {
		req, _ := http.NewRequest("PUT", s.URL+"/refs?net=7&start=1&done=1"+q, strings.NewReader(prev+" refs/heads/main\n"))
		r, err := http.DefaultClient.Do(req)
		if err != nil || r.StatusCode != 200 {
			t.Fatalf("PUT /refs%s: %v %v", q, err, r)
		}
		r.Body.Close()
	}
	check := func() map[string]any {
		got := make(chan map[string]any, 1)
		go func() { got <- connected(t, s.URL, "&objects=tips&repack=1") }()
		select {
		case v := <-got:
			return v
		case <-time.After(5 * time.Second):
			t.Fatal("/connected did not answer in 5 s: it waited on the rehydrate's own repack")
			return nil
		}
	}

	put("&repack=0")
	select {
	case <-started:
		t.Fatal("the last ref page started a repack though the rehydrate's check follows it")
	case <-time.After(300 * time.Millisecond):
	}
	if v := check(); v["connected"] != true || v["repack"] != true {
		t.Fatalf("connected: %v", v)
	}
	select {
	case <-started:
	case <-time.After(5 * time.Second):
		t.Fatal("the check did not start the repack after its walk")
	}
	release <- struct{}{}

	// A failed walk starts the repack too: a ref to a commit whose tree is not here.
	bad := run(t, repo("7"), "tree 1111111111111111111111111111111111111111\nparent "+prev+"\n\nbroken\n",
		"hash-object", "-t", "commit", "--literally", "-w", "--stdin")
	prev = bad
	put("&repack=0")
	if v := check(); v["connected"] != false || v["repack"] != true {
		t.Fatalf("connected over a missing tree: %v", v)
	}
	select {
	case <-started:
	case <-time.After(5 * time.Second):
		t.Fatal("a failed check did not start the repack")
	}
	release <- struct{}{}

	// Without repack=0 the last page starts it at once, as before. The released repack's commit-graph still holds
	// the repack lock for a moment, so wait it out first.
	repacking.Lock()
	repacking.Unlock()
	put("")
	select {
	case <-started:
	case <-time.After(5 * time.Second):
		t.Fatal("a last ref page without repack=0 started no repack")
	}
	release <- struct{}{}
}
