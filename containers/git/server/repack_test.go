package main

import (
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
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
	// The repack leaves a commit-graph that holds the tip, so validate's `--not --all` walks read refs from it, not
	// from the packs; it verifies against the repository.
	if out := run(t, repo("7"), "", "commit-graph", "verify", "--no-progress"); strings.Contains(out, "error") {
		t.Fatalf("commit-graph verify: %s", out)
	}
	if _, err := os.Stat(filepath.Join(repo("7"), "objects", "info", "commit-graphs", "commit-graph-chain")); err != nil {
		t.Fatalf("no commit-graph after the repack: %v", err)
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

// A rehydrate's ingests write a multi-pack-index every midxEvery packs, so index-pack's object lookups stop scanning
// every pack (per-pack ingest grew linearly with the pack count). Every ingested pack but the last few is in it, the
// repository stays whole, and the repack at the end of the catch-up leaves no index naming a deleted pack.
func TestIngestWritesAMultiPackIndexEveryMidxEveryPacks(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	n := 2*midxEvery + 7
	prev, tip := "", ""
	for i := 0; i < n; i++ {
		tip = commit(t, wt, map[string]string{"f": strings.Repeat("y", i+1) + "\n"})
		var pack []byte
		if prev == "" {
			b, err := gitOut(wt, tip+"\n", "pack-objects", "--stdout", "--revs", "-q")
			if err != nil {
				t.Fatal(err)
			}
			pack = b
		} else {
			pack = packOfWt(t, wt, tip, prev)
		}
		sum := sha256.Sum256(pack)
		if code, body := postBody(t, s.URL+"/ingest?net=7&sha256="+hex.EncodeToString(sum[:]), pack); code != 200 {
			t.Fatalf("ingest %d: %d %s", i, code, body)
		}
		prev = tip
	}
	midx := filepath.Join(repo("7"), "objects", "pack", "multi-pack-index")
	b, err := os.ReadFile(midx)
	if err != nil {
		t.Fatalf("no multi-pack-index after %d ingests: %v", n, err)
	}
	// The header: "MIDX", version, hash version, chunk count, base count, then the pack count.
	if covered := int(binary.BigEndian.Uint32(b[8:12])); covered != 2*midxEvery {
		t.Fatalf("the multi-pack-index covers %d packs of %d, want %d", covered, n, 2*midxEvery)
	}
	if out := run(t, repo("7"), "", "multi-pack-index", "verify"); strings.Contains(out, "error") {
		t.Fatalf("verify: %s", out)
	}
	putRefs(t, s.URL, tip)
	repacking.Lock()
	repacking.Unlock()
	if out := run(t, repo("7"), "", "fsck", "--strict", "--no-dangling"); out != "" {
		t.Fatalf("fsck: %s", out)
	}
	if _, err := os.Stat(midx); err == nil {
		if out := run(t, repo("7"), "", "multi-pack-index", "verify"); strings.Contains(out, "error") {
			t.Fatalf("verify after the repack: %s", out)
		}
	}
	if got := run(t, repo("7"), "", "rev-parse", "refs/heads/main"); got != tip {
		t.Fatalf("main %s, want %s", got, tip)
	}
}
