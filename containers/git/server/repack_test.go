package main

import (
	"net/http"
	"os/exec"
	"strings"
	"testing"
	"time"
)

// A catch-up's last step, PUT /refs, starts a geometric repack once the repository holds more than repackAt packs, so
// a shard's pack count stops growing with every ingested push; below it, nothing repacks. Every object stays, so the
// repository still checks out and a merge still finds both sides.
func TestRefsRepacksPastRepackAt(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	// One pack per commit, as ingest leaves them: each pack holds one commit's new objects.
	prev := ""
	tip := ""
	for i := 0; i < repackAt+10; i++ {
		tip = commit(t, wt, map[string]string{"f": strings.Repeat("x", i+1) + "\n"})
		spec := tip + "\n"
		if prev != "" {
			spec += "^" + prev + "\n"
		}
		pack := exec.Command("git", "pack-objects", "--stdout", "--revs", "-q")
		pack.Dir = wt
		pack.Stdin = strings.NewReader(spec)
		b, err := pack.Output()
		if err != nil {
			t.Fatal(err)
		}
		idx := exec.Command("git", "index-pack", "--stdin", "--fix-thin")
		idx.Dir = repo("7")
		idx.Stdin = strings.NewReader(string(b))
		if out, err := idx.CombinedOutput(); err != nil {
			t.Fatalf("index-pack: %v %s", err, out)
		}
		prev = tip
		// Below the threshold a catch-up's /refs starts no repack.
		if i == repackAt/2 {
			putRefs(t, s.URL, tip)
			time.Sleep(200 * time.Millisecond)
			if n := packCount(repo("7")); n != i+1 {
				t.Fatalf("repacked at %d packs, under repackAt", n)
			}
		}
	}
	before := packCount(repo("7"))
	putRefs(t, s.URL, tip)
	deadline := time.Now().Add(10 * time.Second)
	for packCount(repo("7")) >= before && time.Now().Before(deadline) {
		time.Sleep(50 * time.Millisecond)
	}
	repacking.Lock() // the repack has finished
	repacking.Unlock()
	if n := packCount(repo("7")); n >= before || n > 8 {
		t.Fatalf("%d packs after the catch-up's /refs, from %d", n, before)
	}
	if out := run(t, repo("7"), "", "fsck", "--strict", "--no-dangling"); out != "" {
		t.Fatalf("fsck: %s", out)
	}
	if got := run(t, repo("7"), "", "rev-parse", "refs/heads/main"); got != tip {
		t.Fatalf("main %s, want %s", got, tip)
	}
}

func putRefs(t *testing.T, url, tip string) {
	t.Helper()
	req, _ := http.NewRequest("PUT", url+"/refs?net=7", strings.NewReader(tip+" refs/heads/main\n"))
	r, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	r.Body.Close()
	if r.StatusCode != 200 {
		t.Fatalf("PUT /refs: %d", r.StatusCode)
	}
}
