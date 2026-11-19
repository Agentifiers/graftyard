package main

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

// The LRU's container side with real git: /repos reports each repository's size and last use, a request moves its
// repository to the front, and /evict removes one only while no request holds it.

func serve(t *testing.T) *httptest.Server {
	t.Helper()
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
	data = t.TempDir()
	t.Setenv("GRAFT_DISK_BYTES", "1000000")
	s := httptest.NewServer(mux())
	t.Cleanup(s.Close)
	return s
}

func call(t *testing.T, s *httptest.Server, method, path string) (int, map[string]any) {
	t.Helper()
	req, _ := http.NewRequest(method, s.URL+path, nil)
	r, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	var v map[string]any
	json.NewDecoder(r.Body).Decode(&v)
	return r.StatusCode, v
}

type listed struct {
	Disk  int64 `json:"disk"`
	Repos []struct {
		Network   string `json:"network"`
		Bytes     int64  `json:"bytes"`
		Used      int64  `json:"used"`
		PackBytes int64  `json:"packBytes"`
		Fetched   int64  `json:"fetched"`
		Pushed    int64  `json:"pushed"`
		CaughtUp  int64  `json:"caughtUp"`
	} `json:"repos"`
}

func repos(t *testing.T, s *httptest.Server) listed {
	t.Helper()
	r, err := http.Get(s.URL + "/repos")
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	var l listed
	if err := json.NewDecoder(r.Body).Decode(&l); err != nil {
		t.Fatal(err)
	}
	return l
}

func used(l listed, net string) int64 {
	for _, r := range l.Repos {
		if r.Network == net {
			return r.Used
		}
	}
	return -1
}

func TestReposReportsSizeAndLastUse(t *testing.T) {
	s := serve(t)
	for _, n := range []string{"1", "2"} {
		if c, _ := call(t, s, "POST", "/init?net="+n); c != 200 {
			t.Fatalf("init %s: %d", n, c)
		}
	}
	// A file not named like a repository is not listed.
	os.MkdirAll(filepath.Join(data, "spool"), 0o755)
	l := repos(t, s)
	if l.Disk != 1000000 || len(l.Repos) != 2 {
		t.Fatalf("%+v", l)
	}
	for _, r := range l.Repos {
		if r.Bytes <= 0 {
			t.Fatalf("size of %s: %d", r.Network, r.Bytes)
		}
	}
	before := used(l, "1")
	time.Sleep(20 * time.Millisecond)
	if c, _ := call(t, s, "POST", "/fsck?net=1"); c != 200 {
		t.Fatalf("fsck: %d", c)
	}
	after := repos(t, s)
	if used(after, "1") <= before {
		t.Fatalf("a request did not move network 1 forward: %d then %d", before, used(after, "1"))
	}
	if used(after, "1") <= used(after, "2") {
		t.Fatalf("network 1 is the most recent: %+v", after)
	}
}

// /repos reports each network's last fetch, push, and catch-up apart, for the daily `shard stats` line: a stock fetch
// is not a push, a derived validate (the shard's own promotion) is neither, and a fsck moves only used.
func TestReposReportsFetchPushAndCatchUpApart(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	base := history(t, wt, 3, 50, 1)
	run(t, wt, "", "push", "-q", repo("7"), "main")
	// A small push unpacks to loose objects, so packBytes may be 0, but never more than the repository.
	get := func() (fetched, pushed, caughtUp, packBytes int64) {
		for _, r := range repos(t, s).Repos {
			if r.Network == "7" {
				if r.PackBytes > r.Bytes {
					t.Fatalf("packBytes %d over bytes %d", r.PackBytes, r.Bytes)
				}
				return r.Fetched, r.Pushed, r.CaughtUp, r.PackBytes
			}
		}
		t.Fatal("network 7 not listed")
		return
	}
	if f, p, c, _ := get(); f != 0 || p != 0 || c != 0 {
		t.Fatalf("a new repository: fetched %d pushed %d caughtUp %d", f, p, c)
	}
	tip := commit(t, wt, map[string]string{"new.txt": "new\n"})
	pack, err := gitOut(wt, tip+"\n^"+base+"\n", "pack-objects", "--stdout", "--revs", "-q")
	if err != nil {
		t.Fatal(err)
	}
	if code, body := postBody(t, s.URL+"/validate?net=7&derived=1&tips="+tip, pack); code != 200 {
		t.Fatalf("derived validate: %d %s", code, body)
	}
	call(t, s, "POST", "/fsck?net=7")
	if f, p, c, _ := get(); f != 0 || p != 0 || c != 0 {
		t.Fatalf("a derived validate and a fsck: fetched %d pushed %d caughtUp %d", f, p, c)
	}
	if r, err := http.Get(s.URL + "/info-refs?net=7"); err != nil {
		t.Fatal(err)
	} else {
		r.Body.Close()
	}
	fetched, p, c, _ := get()
	if fetched == 0 || p != 0 || c != 0 {
		t.Fatalf("a fetch: fetched %d pushed %d caughtUp %d", fetched, p, c)
	}
	time.Sleep(20 * time.Millisecond)
	if code, body := postBody(t, s.URL+"/validate?net=7&tips="+tip, pack); code != 200 {
		t.Fatalf("client validate: %d %s", code, body)
	}
	f, pushed, c, _ := get()
	if f != fetched || pushed <= fetched || c != 0 {
		t.Fatalf("a push: fetched %d (was %d) pushed %d caughtUp %d", f, fetched, pushed, c)
	}
	if r, err := http.Post(s.URL+"/refs?net=7", "", strings.NewReader(tip+" refs/heads/x\n")); err != nil {
		t.Fatal(err)
	} else {
		r.Body.Close()
	}
	if f, p, c, _ := get(); f != fetched || p != pushed || c < pushed {
		t.Fatalf("a ref write: fetched %d pushed %d caughtUp %d", f, p, c)
	}
}

func TestEvictRemovesAndRefusesWhileInUse(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=3")
	call(t, s, "POST", "/init?net=4")
	// Hold network 3 shared, as a streaming upload-pack does.
	l := lock("3")
	l.RLock()
	if c, _ := call(t, s, "POST", "/evict?net=3"); c != 409 {
		t.Fatalf("evict under a reader: %d", c)
	}
	l.RUnlock()
	c, v := call(t, s, "POST", "/evict?net=3")
	if c != 200 || v["evicted"] != "3" {
		t.Fatalf("evict: %d %v", c, v)
	}
	if _, err := os.Stat(repo("3")); !os.IsNotExist(err) {
		t.Fatalf("still on disk: %v", err)
	}
	if got := repos(t, s); len(got.Repos) != 1 || got.Repos[0].Network != "4" {
		t.Fatalf("%+v", got)
	}
	// Evicting a network not on disk is fine (idempotent); a bad id is not.
	if c, _ := call(t, s, "POST", "/evict?net=3"); c != 200 {
		t.Fatalf("second evict: %d", c)
	}
	if c, _ := call(t, s, "POST", "/evict?net=../x"); c != 400 {
		t.Fatalf("bad net: %d", c)
	}
	// An evicted network answers 404 until it is rehydrated: the shard object's cursor check sees a missing repo.
	if c, _ := call(t, s, "POST", "/fsck?net=3"); c != 404 {
		t.Fatalf("fsck after evict: %d", c)
	}
}

func TestConcurrentReadersOfOneNetworkDoNotBlockEachOther(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=5")
	var wg sync.WaitGroup
	errs := make(chan string, 8)
	for i := 0; i < 8; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if c, v := call(t, s, "POST", "/fsck?net=5"); c != 200 {
				errs <- strings.TrimSpace(v["err"].(string))
			}
		}()
	}
	wg.Wait()
	close(errs)
	for e := range errs {
		t.Fatal(e)
	}
}
