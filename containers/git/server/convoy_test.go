package main

import (
	"bytes"
	"fmt"
	"io"
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
