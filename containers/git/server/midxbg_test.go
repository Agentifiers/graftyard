package main

import (
	"encoding/binary"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// midx-background (decisions/DECISIONS.md, "`midx-background`"): the multi-pack-index write after every midxEvery-th
// ingest runs in the background under its own TryLock, not in the writer's section, where B0-34 mt7's monorepo
// catch-ups spent 5 to 17.6 s no timed step accounted for (tests/am1/results/b034-mt7.md). A git on PATH holds each
// `multi-pack-index write` while the gate's hold file exists, and logs each one it starts.

type midxGate struct{ log, hold string }

func gateMidx(t *testing.T) midxGate {
	t.Helper()
	real, err := exec.LookPath("git")
	if err != nil {
		t.Fatal(err)
	}
	bin, dir := t.TempDir(), t.TempDir()
	g := midxGate{filepath.Join(dir, "log"), filepath.Join(dir, "hold")}
	wrap := "#!/bin/sh\n" +
		"if [ \"$1\" = multi-pack-index ]; then\n" +
		"  echo start >> " + g.log + "\n" +
		"  while [ -e " + g.hold + " ]; do sleep 0.02; done\n" +
		"fi\n" +
		"exec " + real + " \"$@\"\n"
	if err := os.WriteFile(filepath.Join(bin, "git"), []byte(wrap), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	return g
}

func (g midxGate) holdOn()      { os.WriteFile(g.hold, nil, 0o644) }
func (g midxGate) release()     { os.Remove(g.hold) }
func (g midxGate) started() int { b, _ := os.ReadFile(g.log); return strings.Count(string(b), "start") }

// waitStarted waits until the gate has seen n writes start.
func (g midxGate) waitStarted(t *testing.T, n int) {
	t.Helper()
	for i := 0; g.started() < n; i++ {
		if i > 500 {
			g.release()
			t.Fatalf("%d multi-pack-index writes started, want %d", g.started(), n)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

// midxWait waits for the repository's background multi-pack-index write, if one runs.
func midxWait(rp string) {
	m := midxOf(rp)
	m.mu.Lock()
	m.mu.Unlock()
}

// midxRepo is network 7 with one commit ingested and main on it, and a work tree to commit more from.
type midxRepo struct {
	t    *testing.T
	url  string
	wt   string
	prev string
	n    int
}

func newMidxRepo(t *testing.T, url string) *midxRepo {
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commit(t, wt, map[string]string{"README": "r\n"})
	ingestPack(t, url, "7", selfPack(t, wt, base))
	putRefs(t, url, base)
	return &midxRepo{t: t, url: url, wt: wt, prev: base}
}

// next commits a change and answers it and its pack, thin against the last commit.
func (r *midxRepo) next() (string, []byte) {
	r.n++
	tip := commit(r.t, r.wt, map[string]string{"f": fmt.Sprintf("%d\n", r.n)})
	pack := packOfWt(r.t, r.wt, tip, r.prev)
	r.prev = tip
	return tip, pack
}

// ingestWithin ingests the next commit and fails the test if /ingest has not answered 200 within d.
func (r *midxRepo) ingestWithin(g midxGate, what string, d time.Duration) string {
	r.t.Helper()
	tip, pack := r.next()
	done := make(chan string, 1)
	go func() {
		code, body := postBody(r.t, r.url+"/ingest?net=7&sha256="+sumOf(pack), pack)
		done <- fmt.Sprintf("%d %.200s", code, body)
	}()
	select {
	case got := <-done:
		if !strings.HasPrefix(got, "200 ") {
			g.release()
			r.t.Fatalf("%s: %s", what, got)
		}
	case <-time.After(d):
		g.release()
		r.t.Fatalf("%s waited on the multi-pack-index write", what)
	}
	return tip
}

func midxCovers(t *testing.T, rp string) int {
	t.Helper()
	b, err := os.ReadFile(filepath.Join(rp, "objects", "pack", "multi-pack-index"))
	if err != nil {
		t.Fatalf("no multi-pack-index: %v", err)
	}
	return int(binary.BigEndian.Uint32(b[8:12]))
}

// While a multi-pack-index write runs, the ingest that started it answers, another ingest goes in, and a validate
// answers; evict, which deletes the repository, refuses until it ends. Once it ends the index is whole.
func TestAMidxWriteHoldsNoIngestOrValidate(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	r := newMidxRepo(t, s.URL)
	rp := repo("7")
	g := gateMidx(t)
	g.holdOn()
	midxOf(rp).n = midxEvery - 1
	r.ingestWithin(g, "the ingest that started the write", 10*time.Second)
	g.waitStarted(t, 1)
	b := r.ingestWithin(g, "an ingest beside the write", 10*time.Second)
	run(t, r.wt, "", "checkout", "-q", "-B", "side", b)
	side := commit(t, r.wt, map[string]string{"side": "s\n"})
	vdone := make(chan string, 1)
	go func() {
		code, _, body := validateDetail(t, s.URL, []string{side}, nil, packOfWt(t, r.wt, side, b))
		vdone <- fmt.Sprintf("%d %.200s", code, body)
	}()
	select {
	case got := <-vdone:
		if !strings.HasPrefix(got, "200 ") {
			g.release()
			t.Fatalf("a validate beside the write: %s", got)
		}
	case <-time.After(10 * time.Second):
		g.release()
		t.Fatal("a validate waited on the multi-pack-index write")
	}
	if c, _ := call(t, s, "POST", "/evict?net=7"); c != 409 {
		g.release()
		t.Fatalf("evict during a multi-pack-index write: %d, want 409", c)
	}
	g.release()
	midxWait(rp)
	if out := run(t, rp, "", "multi-pack-index", "verify"); strings.Contains(out, "error") {
		t.Fatalf("multi-pack-index verify: %s", out)
	}
	run(t, rp, "", "cat-file", "-e", b)
	if out := run(t, rp, "", "fsck", "--strict", "--no-dangling", "--no-progress"); strings.Contains(out, "error") {
		t.Fatalf("fsck: %s", out)
	}
}

// A trigger that finds a write running is skipped, not queued: no second write starts when the first ends, and the
// count stays past midxEvery, so the next ingest after it starts one.
func TestASecondMidxTriggerIsSkippedNotQueued(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	r := newMidxRepo(t, s.URL)
	rp := repo("7")
	g := gateMidx(t)
	g.holdOn()
	midxOf(rp).n = midxEvery - 1
	r.ingestWithin(g, "the ingest that started the write", 10*time.Second)
	g.waitStarted(t, 1)
	for i := 0; i < midxEvery+2; i++ {
		r.ingestWithin(g, fmt.Sprintf("ingest %d beside the write", i), 10*time.Second)
	}
	if n := g.started(); n != 1 {
		g.release()
		t.Fatalf("%d writes started while one ran, want 1", n)
	}
	g.release()
	midxWait(rp)
	time.Sleep(300 * time.Millisecond)
	midxWait(rp)
	if n := g.started(); n != 1 {
		t.Fatalf("%d writes after the first ended with no ingest since, want 1: a skipped trigger was queued", n)
	}
	r.ingestWithin(g, "the next ingest", 10*time.Second)
	midxWait(rp)
	if n := g.started(); n != 2 {
		t.Fatalf("%d writes after the next ingest, want 2: the skipped triggers were dropped", n)
	}
}

// Readers see the old multi-pack-index or the new one, never neither: while a write is held, and through repeated
// real writes, the index file is always there and every ingested object is found, packs ingested since the last
// write through their own `.idx`.
func TestReadersSeeTheOldMultiPackIndexOrTheNew(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	r := newMidxRepo(t, s.URL)
	rp := repo("7")
	g := gateMidx(t)
	for i := 0; i < midxEvery; i++ {
		r.ingestWithin(g, fmt.Sprintf("seed ingest %d", i), 10*time.Second)
	}
	midxWait(rp)
	before := midxCovers(t, rp)

	var mu sync.Mutex
	known := []string{r.prev}
	add := func(tip string) {
		mu.Lock()
		known = append(known, tip, tip+":f")
		mu.Unlock()
	}
	var reads atomic.Int64
	stop, failed := make(chan struct{}), make(chan string, 1)
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		midx := filepath.Join(rp, "objects", "pack", "multi-pack-index")
		for {
			select {
			case <-stop:
				return
			default:
			}
			if _, err := os.Stat(midx); err != nil {
				failed <- fmt.Sprintf("no multi-pack-index after %d reads: %v", reads.Load(), err)
				return
			}
			mu.Lock()
			in := strings.Join(known, "\n") + "\n"
			mu.Unlock()
			out, err := gitOut(rp, in, "cat-file", "--batch-check")
			if err != nil || strings.Contains(string(out), "missing") {
				failed <- fmt.Sprintf("read %d: %v %s", reads.Load(), err, out)
				return
			}
			reads.Add(1)
		}
	}()
	check := func() {
		select {
		case f := <-failed:
			g.release()
			close(stop)
			t.Fatal(f)
		default:
		}
	}

	g.holdOn()
	midxOf(rp).n = midxEvery - 1
	add(r.ingestWithin(g, "the ingest that started the held write", 10*time.Second))
	g.waitStarted(t, 1)
	for i := 0; i < 3; i++ {
		add(r.ingestWithin(g, "an ingest beside the held write", 10*time.Second))
	}
	held := reads.Load()
	for i := 0; reads.Load() < held+5 && i < 500; i++ {
		time.Sleep(10 * time.Millisecond)
	}
	check()
	g.release()
	midxWait(rp)
	for c := 0; c < 5; c++ {
		midxOf(rp).n = midxEvery - 1
		add(r.ingestWithin(g, "an ingest that starts a write", 10*time.Second))
		add(r.ingestWithin(g, "an ingest beside it", 10*time.Second))
		midxWait(rp)
		check()
	}
	close(stop)
	wg.Wait()
	select {
	case f := <-failed:
		t.Fatal(f)
	default:
	}
	if reads.Load() < 10 {
		t.Fatalf("only %d reads ran beside the writes", reads.Load())
	}
	if after := midxCovers(t, rp); after <= before {
		t.Fatalf("the multi-pack-index covers %d packs after the writes, %d before", after, before)
	}
	if g.started() < 6 {
		t.Fatalf("%d writes started, want at least 6", g.started())
	}
}

// The repack, which writes its own multi-pack-index and deletes the packs it rolled, and a fresh init, which deletes
// the repository, each wait for a running multi-pack-index write and go on once it ends.
func TestRepackAndFreshInitWaitForAMidxWrite(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	r := newMidxRepo(t, s.URL)
	rp := repo("7")
	g := gateMidx(t)
	for i := 0; i <= repackAt; i++ {
		r.ingestWithin(g, fmt.Sprintf("ingest %d", i), 10*time.Second)
	}
	midxWait(rp)
	hold := func() {
		g.holdOn()
		n := g.started()
		midxOf(rp).n = midxEvery - 1
		r.ingestWithin(g, "the ingest that started the write", 10*time.Second)
		g.waitStarted(t, n+1)
	}
	hold()
	started := make(chan struct{})
	old := runRepack
	runRepack = func(rp string) error {
		close(started)
		return old(rp)
	}
	defer func() { runRepack = old }()
	if !startRepack("7", rp) {
		g.release()
		t.Fatalf("no repack started over %d unkept packs", unkeptPacks(rp))
	}
	select {
	case <-started:
		g.release()
		t.Fatal("the repack ran beside a multi-pack-index write")
	case <-time.After(300 * time.Millisecond):
	}
	g.release()
	select {
	case <-started:
	case <-time.After(10 * time.Second):
		t.Fatal("the repack never ran after the multi-pack-index write ended")
	}
	repacking.Lock()
	repacking.Unlock()
	if out := run(t, rp, "", "fsck", "--strict", "--no-dangling", "--no-progress"); strings.Contains(out, "error") {
		t.Fatalf("fsck after the repack: %s", out)
	}

	hold()
	initDone := make(chan int, 1)
	go func() {
		c, _ := call(t, s, "POST", "/init?net=7&fresh=1")
		initDone <- c
	}()
	select {
	case c := <-initDone:
		g.release()
		t.Fatalf("a fresh init answered %d beside a multi-pack-index write", c)
	case <-time.After(300 * time.Millisecond):
	}
	g.release()
	select {
	case c := <-initDone:
		if c != 200 {
			t.Fatalf("the fresh init after the write: %d", c)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("the fresh init never answered after the multi-pack-index write ended")
	}
	if n := packCount(rp); n != 0 {
		t.Fatalf("%d packs after a fresh init", n)
	}
}
