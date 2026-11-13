package main

import (
	"bytes"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"io"
	"net/http"
	"reflect"
	"strings"
	"testing"
	"time"
)

func postBody(t *testing.T, url string, body []byte) (int, []byte) {
	t.Helper()
	r, err := http.Post(url, "application/octet-stream", bytes.NewReader(body))
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	b, err := io.ReadAll(r.Body)
	if err != nil {
		t.Fatal(err)
	}
	return r.StatusCode, b
}

func TestHaveListsOnlyIngestedPacks(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commit(t, wt, map[string]string{"f": "base\n"})
	run(t, wt, "", "push", "-q", repo("7"), "main")
	tip := commit(t, wt, map[string]string{"f": "tip\n"})
	pack := packOfWt(t, wt, tip, base)
	sumBytes := sha256.Sum256(pack)
	sum := hex.EncodeToString(sumBytes[:])
	if code, body := postBody(t, s.URL+"/ingest?net=7&sha256="+sum, pack); code != 200 {
		t.Fatalf("ingest: %d %s", code, body)
	}

	random := make([]byte, 32)
	if _, err := rand.Read(random); err != nil {
		t.Fatal(err)
	}
	missing := hex.EncodeToString(random)
	code, body := postBody(t, s.URL+"/have?net=7", []byte(sum+"\n"+missing+"\n"))
	if code != 200 {
		t.Fatalf("have: %d %s", code, body)
	}
	var out struct {
		Have []string `json:"have"`
	}
	if err := json.Unmarshal(body, &out); err != nil {
		t.Fatal(err)
	}
	if want := []string{sum}; !reflect.DeepEqual(out.Have, want) {
		t.Fatalf("have %v, want %v", out.Have, want)
	}
	if code, body := postBody(t, s.URL+"/have?net=7", []byte(sum+"\nnot-a-sha256\n")); code != 400 {
		t.Fatalf("a malformed sum answered %d: %s", code, body)
	}
}

func TestIngestOfAPackWhoseNamedObjectIsMissingAnswers409(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	tip := commit(t, wt, map[string]string{"f": "not in the pack\n"})
	// Naming objects rather than revisions packs only the commit, leaving its tree and blob out.
	pack, err := gitOut(wt, tip+"\n", "pack-objects", "--stdout", "-q")
	if err != nil {
		t.Fatal(err)
	}
	sumBytes := sha256.Sum256(pack)
	sum := hex.EncodeToString(sumBytes[:])
	code, body := postBody(t, s.URL+"/ingest?net=7&sha256="+sum, pack)
	if code != 409 {
		t.Fatalf("a pack missing its commit's tree answered %d, want 409: %s", code, body)
	}
	if !strings.Contains(string(body), "did not receive expected object") {
		t.Fatalf("the pack failed for an unexpected reason: %s", body)
	}
}

func TestDerivedGitCommandsAreCountedApart(t *testing.T) {
	dir := t.TempDir()
	in := strings.NewReader("same input\n")
	plain, err := git(dir, nil, in, "hash-object", "--stdin")
	if err != nil {
		t.Fatal(err)
	}
	marked, err := git(dir, []string{derivedEnv}, strings.NewReader("same input\n"), "hash-object", "--stdin")
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(marked, plain) {
		t.Fatalf("marked output %q, plain output %q", marked, plain)
	}
	derived.Lock()
	left := len(derived.procs)
	derived.Unlock()
	if left != 0 {
		t.Fatalf("%d derived processes still registered after Wait", left)
	}
	// The CPU of derived work is what /load subtracts; a client's same work is not counted. B0-30's k rose from 1 to
	// 6 while the first shard and its cold pool shards spent their CPU on catch-up and rehydrate ingests, which run on
	// every shard whatever k is (tests/am1/results/hot-shards.md, A31).
	big := strings.Repeat("a fairly long line of input to hash\n", 1<<21)
	spent := func(env []string) int64 {
		before := sampleDerived()
		if _, err := git(dir, env, strings.NewReader(big), "hash-object", "--stdin"); err != nil {
			t.Fatal(err)
		}
		return sampleDerived() - before
	}
	if d := spent(nil); d != 0 {
		t.Fatalf("a client command counted %d µs as derived", d)
	}
	if d := spent([]string{derivedEnv}); d < 20_000 {
		t.Fatalf("a derived hash of 72 MiB counted only %d µs", d)
	}
}

// A derived command's CPU counts while it runs, including a child it has not reaped yet: `git repack` spends its
// time in `pack-objects`, and with only the git process counted, /load read about 0.99 busy through every rehydrate's
// final repack (tests/am1/results/hot-shards-2.md, A31). Here git's alias runs a shell whose own child burns the CPU.
func TestARunningDerivedCommandsChildCountsBeforeItExits(t *testing.T) {
	dir := t.TempDir()
	burn := "alias.burn=!sh -c 'sh -c \"i=0; while [ \\$i -lt 1500000 ]; do i=\\$((i+1)); done\"'"
	done := make(chan error, 1)
	before := sampleDerived()
	t0 := time.Now()
	go func() {
		_, err := git(dir, []string{derivedEnv}, nil, "-c", burn, "burn")
		done <- err
	}()
	time.Sleep(1500 * time.Millisecond)
	mid := sampleDerived() - before
	select {
	case err := <-done:
		t.Fatalf("the burn ended within 1.5 s (%v), so the test shows nothing; make it longer", err)
	default:
	}
	if mid < 500_000 {
		t.Fatalf("1.5 s into a derived command whose grandchild burns a CPU, %d µs counted, want at least 500,000", mid)
	}
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	// At its exit the rusage is added once: one single-threaded burn's CPU is at most its wall time, where counting
	// the running child and then the reaped one again would read about twice that.
	wall := time.Since(t0).Microseconds()
	total := sampleDerived() - before
	if total < mid || total > wall+wall/10+50_000 {
		t.Fatalf("after exit %d µs counted over %d µs of wall, %d µs while running", total, wall, mid)
	}
}
