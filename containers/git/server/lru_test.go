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
		Network string `json:"network"`
		Bytes   int64  `json:"bytes"`
		Used    int64  `json:"used"`
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
