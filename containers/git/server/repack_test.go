package main

import (
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
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
		// 4 KB of the same text with one line changed per commit, as source files change: git deltas each version
		// against the others only when a repack searches deltas across the packs.
		tip = commit(t, wt, map[string]string{"f": fmt.Sprintf("version %d\n", i) + strings.Repeat("the same line of source text\n", 140)})
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
	// Each push's pack held one version of f whole, so only a delta search across the packs stores the versions as
	// deltas of each other; a repack that skips it (--window=0) keeps every one whole and the disk grows with pushes.
	if n := deltas(t, repo("7")); n < repackAt/2 {
		t.Fatalf("%d objects stored as deltas after the repack; it searched none", n)
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

// deltas counts the objects the repository's packs store as deltas.
func deltas(t *testing.T, rp string) int {
	t.Helper()
	packs, _ := filepath.Glob(filepath.Join(rp, "objects", "pack", "pack-*.idx"))
	n := 0
	for _, p := range packs {
		for _, l := range strings.Split(run(t, rp, "", "verify-pack", "-v", p), "\n") {
			// A delta's line has seven fields: sha type size size-in-pack offset depth base.
			if f := strings.Fields(l); len(f) == 7 {
				n++
			}
		}
	}
	return n
}

// A rehydrate loads promotions through their stored .idx, then ingests the pushed tail above them, and its last /refs
// starts the geometric repack. The promotions are complete and already delta-searched, so the repack must leave each
// one byte for byte where it is and roll only the tail; rolling them held every shard of B0-34 at 4 vCPU and the
// container's memory cap for minutes (tests/am1/results/lm1-memory.md, rounds 24c and 25). The promotions alone,
// however many, start no repack, and the multi-pack-index written after the repack names every pack.
func TestRehydratesRepackLeavesPromotedPacks(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	// More promotions than repackAt: they must not count toward it.
	type promo struct{ name, digest string }
	var promos []promo
	prev, tip := "", ""
	for i := 0; i < repackAt+3; i++ {
		tip = commit(t, wt, map[string]string{fmt.Sprintf("p/%d", i): strings.Repeat(fmt.Sprintf("promoted %d\n", i), 200)})
		spec := tip + "\n"
		if prev != "" {
			spec += "^" + prev + "\n"
		}
		pack, err := gitOut(wt, spec, "pack-objects", "--stdout", "--revs", "-q")
		if err != nil {
			t.Fatal(err)
		}
		sum := stage(t, s.URL, "7", pack, idxFor(t, pack))
		if code, a, body := ingestStaged(t, s.URL, "7", sum); code != 200 || a.Idx != "used" {
			t.Fatalf("promotion %d: %d %s", i, code, body)
		}
		d := sha256.Sum256(pack)
		promos = append(promos, promo{hex.EncodeToString(pack[len(pack)-20:]), hex.EncodeToString(d[:])})
		prev = tip
	}
	// /refs over the promotions alone starts no repack: they do not count toward repackAt.
	req, _ := http.NewRequest("PUT", s.URL+"/refs?net=7&start=1&done=1", strings.NewReader(tip+" refs/heads/main\n"))
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	var ans struct{ Repack bool }
	json.NewDecoder(resp.Body).Decode(&ans)
	resp.Body.Close()
	if resp.StatusCode != 200 || ans.Repack {
		t.Fatalf("PUT /refs over %d promotions alone: %d, repack %v", len(promos), resp.StatusCode, ans.Repack)
	}
	repacking.Lock()
	repacking.Unlock()
	if n := packCount(repo("7")); n != len(promos) {
		t.Fatalf("%d packs after /refs over %d promotions alone", n, len(promos))
	}
	// The pushed tail: one thin pack per push, as /ingest leaves them, past repackAt of its own.
	for i := 0; i <= repackAt+5; i++ {
		tip = commit(t, wt, map[string]string{"f": fmt.Sprintf("version %d\n", i) + strings.Repeat("the same line of source text\n", 140)})
		pack := packOfWt(t, wt, tip, prev)
		if code, body := postBody(t, s.URL+"/ingest?net=7&sha256="+sumOf(pack), pack); code != 200 {
			t.Fatalf("push %d: %d %s", i, code, body)
		}
		prev = tip
	}
	before := packCount(repo("7"))
	putRefs(t, s.URL, tip)
	deadline := time.Now().Add(20 * time.Second)
	for packCount(repo("7")) >= before && time.Now().Before(deadline) {
		time.Sleep(50 * time.Millisecond)
	}
	repacking.Lock() // the repack and its commit-graph have finished
	repacking.Unlock()
	pdir := filepath.Join(repo("7"), "objects", "pack")
	for i, p := range promos {
		b, err := os.ReadFile(filepath.Join(pdir, "pack-"+p.name+".pack"))
		if err != nil {
			t.Fatalf("promotion %d's pack is gone after the repack: %v", i, err)
		}
		if d := sha256.Sum256(b); hex.EncodeToString(d[:]) != p.digest {
			t.Fatalf("promotion %d's pack changed", i)
		}
	}
	if n, kept := packCount(repo("7")), len(promos); n > kept+2 || n >= before {
		t.Fatalf("%d packs from %d, with %d promotions kept: the tail did not roll", n, before, kept)
	}
	if out := run(t, repo("7"), "", "multi-pack-index", "verify", "--no-progress"); strings.Contains(out, "error") {
		t.Fatalf("multi-pack-index verify: %s", out)
	}
	midx, err := os.ReadFile(filepath.Join(pdir, "multi-pack-index"))
	if err != nil {
		t.Fatalf("no multi-pack-index after the repack: %v", err)
	}
	if covered := int(binary.BigEndian.Uint32(midx[8:12])); covered != packCount(repo("7")) {
		t.Fatalf("the multi-pack-index names %d of %d packs", covered, packCount(repo("7")))
	}
	if out := run(t, repo("7"), "", "fsck", "--strict", "--no-dangling"); out != "" {
		t.Fatalf("fsck: %s", out)
	}
	if got := run(t, repo("7"), "", "rev-parse", "refs/heads/main"); got != tip {
		t.Fatalf("main %s, want %s", got, tip)
	}
}

// The repack and its commit-graph run at nice 19 with two pack threads and a bounded delta window, so a catch-up,
// validate, or read that shares the container's vCPUs runs first. /procs lists the running git processes with their
// nice value, so a test (and a saturated shard's /status) can see it.
func TestRepackRunsLowAndProcsListsIt(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	// A git process held open on stdin, run through the repack's own path, stands in for a long repack.
	r, w := io.Pipe()
	cmd := exec.Command("nice", "-n", "19", "git", "hash-object", "--stdin")
	cmd.Stdin = r
	cmd.Dir = repo("7")
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	defer func() { w.Close(); cmd.Wait() }()
	// Other git processes on the machine show too; this one is found by its pid.
	var got *gitProc
	for range 50 {
		resp, err := http.Get(s.URL + "/procs")
		if err != nil {
			t.Fatal(err)
		}
		body, _ := io.ReadAll(resp.Body)
		resp.Body.Close()
		var v struct{ Procs []gitProc }
		if resp.StatusCode != 200 || json.Unmarshal(body, &v) != nil {
			t.Fatalf("/procs: %d %s", resp.StatusCode, body)
		}
		for i, p := range v.Procs {
			if i > 0 && p.CPUs > v.Procs[i-1].CPUs {
				t.Fatalf("/procs not most CPU first: %+v", v.Procs)
			}
			if p.Pid == cmd.Process.Pid {
				got = &p
			}
		}
		if got != nil && got.Cmd == "git hash-object --stdin" && got.RSS > 0 {
			break // seen after nice exec'd git, with its memory mapped
		}
		time.Sleep(20 * time.Millisecond)
	}
	if got == nil || got.Cmd != "git hash-object --stdin" || got.Nice != 19 || got.RSS <= 0 {
		t.Fatalf("/procs: %+v", got)
	}

	// runRepack itself: every git it starts runs at nice 19, seen by a git on PATH ahead of the real one that writes
	// its own nice value, then runs the real git; and with the bounds, seen through GIT_TRACE2's parameters.
	real, err := exec.LookPath("git")
	if err != nil {
		t.Fatal(err)
	}
	bin, log := t.TempDir(), filepath.Join(t.TempDir(), "nice")
	wrap := "#!/bin/sh\nnice >> " + log + "\nexec " + real + " \"$@\"\n"
	if err := os.WriteFile(filepath.Join(bin, "git"), []byte(wrap), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	ev := filepath.Join(t.TempDir(), "trace")
	t.Setenv("GIT_TRACE2_EVENT", ev)
	t.Setenv("GIT_TRACE2_CONFIG_PARAMS", "pack.threads,pack.windowMemory")
	if err := runRepack(repo("7")); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(ev)
	for _, want := range []string{`"param":"pack.threads","value":"2"`, `"param":"pack.windowmemory","value":"256m"`} {
		if !strings.Contains(string(b), want) {
			t.Fatalf("runRepack's git ran without %s", want)
		}
	}
	seen, _ := os.ReadFile(log)
	if lines := strings.Fields(string(seen)); len(lines) == 0 || strings.Trim(strings.Join(lines, ""), "19") != "" {
		t.Fatalf("runRepack's git ran at nice %q, want 19 every time", lines)
	}
}
