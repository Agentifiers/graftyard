package main

import (
	"bytes"
	"encoding/binary"
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

// An /ingest holds the network's lock alone only to move its pack in, record it and write the midx; its index-pack runs
// under the lock shared. B0-34 m41's catch-ups held the lock 5.5 to 13.5 s for packs of 0.1 to 2.3 MB, with every
// ls-refs and validate queued behind them (`tests/am1/results/b034-trace.md`). Here a git on PATH holds index-pack of
// the first ingest open until the test lets it go: an ls-refs must answer meanwhile. Then a second ingest of another
// pack, as a concurrent catch-up's, starts beside it; its move waits for the held index step (Go's RWMutex then holds
// new readers behind it, as before), and once released both finish, both packs are in and recorded, no quarantine is
// left, the multi-pack-index covers every pack it names, and fsck finds nothing.
func TestIngestIndexesOutsideTheLock(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commit(t, wt, map[string]string{"README": "r\n"})
	ingestPack(t, s.URL, "7", selfPack(t, wt, base))
	putRefs(t, s.URL, base)
	// Past midxEvery ingests by the end, so the second ingest writes a multi-pack-index beside the held one.
	prev := base
	for i := 0; i < midxEvery-2; i++ {
		tip := commit(t, wt, map[string]string{"pad": fmt.Sprintf("%d\n", i)})
		ingestPack(t, s.URL, "7", packOfWt(t, wt, tip, prev))
		prev = tip
	}
	putRefs(t, s.URL, prev)
	slowTip := commit(t, wt, map[string]string{"slow.txt": strings.Repeat("slow\n", 2000)})
	slow := packOfWt(t, wt, slowTip, prev)
	run(t, wt, "", "checkout", "-q", "-b", "side", prev)
	fastTip := commit(t, wt, map[string]string{"fast.txt": strings.Repeat("fast\n", 2000)})
	fast := packOfWt(t, wt, fastTip, prev)

	// index-pack reading the slow pack waits for `release` before it reads; any other git runs at once.
	real, err := exec.LookPath("git")
	if err != nil {
		t.Fatal(err)
	}
	bin, gate := t.TempDir(), t.TempDir()
	marker := filepath.Join(gate, "held")
	release := filepath.Join(gate, "release")
	wrap := "#!/bin/sh\n" +
		"if [ \"$1\" = index-pack ] && [ ! -e " + marker + " ]; then\n" +
		"  touch " + marker + "\n" +
		"  while [ ! -e " + release + " ]; do sleep 0.05; done\n" +
		"fi\n" +
		"exec " + real + " \"$@\"\n"
	if err := os.WriteFile(filepath.Join(bin, "git"), []byte(wrap), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))

	slowDone := make(chan string, 1)
	go func() {
		code, body := postBody(t, s.URL+"/ingest?net=7&sha256="+sumOf(slow), slow)
		slowDone <- fmt.Sprintf("%d %.120s", code, body)
	}()
	for i := 0; ; i++ {
		if _, err := os.Stat(marker); err == nil {
			break
		}
		if i > 400 {
			t.Fatal("the first ingest never reached index-pack")
		}
		time.Sleep(10 * time.Millisecond)
	}

	// With index-pack held: an ls-refs answers, and a second ingest (its index-pack not held) finishes.
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
		if !bytes.Contains(b, []byte(prev+" refs/heads/main")) {
			ls <- fmt.Errorf("ls-refs answered %q", b)
			return
		}
		ls <- nil
	}()
	held := func(what string, c <-chan error) {
		select {
		case err := <-c:
			if err != nil {
				os.WriteFile(release, nil, 0o644)
				t.Fatalf("%s: %v", what, err)
			}
		case <-time.After(10 * time.Second):
			os.WriteFile(release, nil, 0o644)
			t.Fatalf("%s waited on an ingest's index-pack", what)
		}
	}
	held("an ls-refs", ls)
	fastDone := make(chan string, 1)
	go func() {
		code, body := postBody(t, s.URL+"/ingest?net=7&sha256="+sumOf(fast), fast)
		fastDone <- fmt.Sprintf("%d %.120s", code, body)
	}()
	select {
	case got := <-slowDone:
		t.Fatalf("the held ingest answered before its index-pack ran: %s", got)
	default:
	}

	os.WriteFile(release, nil, 0o644)
	for what, c := range map[string]chan string{"the held ingest": slowDone, "the concurrent ingest": fastDone} {
		select {
		case got := <-c:
			if !strings.HasPrefix(got, "200 ") {
				t.Fatalf("%s: %s", what, got)
			}
		case <-time.After(20 * time.Second):
			t.Fatalf("%s never finished", what)
		}
	}

	rp := repo("7")
	for _, tip := range []string{slowTip, fastTip} {
		run(t, rp, "", "cat-file", "-e", tip)
	}
	for _, p := range [][]byte{slow, fast} {
		if _, err := os.Stat(filepath.Join(rp, "graft", sumOf(p)+".objs")); err != nil {
			t.Fatalf("pack %s not recorded: %v", sumOf(p)[:12], err)
		}
	}
	if inc, _ := filepath.Glob(filepath.Join(rp, "graft", "incoming-*")); len(inc) != 0 {
		t.Fatalf("quarantines left behind: %v", inc)
	}
	b, err := os.ReadFile(filepath.Join(rp, "objects", "pack", "multi-pack-index"))
	if err != nil {
		t.Fatalf("no multi-pack-index after %d ingests: %v", midxEvery+1, err)
	}
	if covered := int(binary.BigEndian.Uint32(b[8:12])); covered < midxEvery {
		t.Fatalf("the multi-pack-index covers %d packs, want at least %d", covered, midxEvery)
	}
	if out := run(t, rp, "", "multi-pack-index", "verify"); strings.Contains(out, "error") {
		t.Fatalf("multi-pack-index verify: %s", out)
	}
	if out := run(t, rp, "", "fsck", "--strict", "--no-progress"); out != "" && strings.Contains(out, "error") {
		t.Fatalf("fsck: %s", out)
	}
}
