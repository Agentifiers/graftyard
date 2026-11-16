package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
	"testing"
	"time"
)

// TestLinuxComposedMissCost measures composed and stock depth-1 misses on a linux-sized network; it runs only with
// GRAFT_LINUX set to a bare clone of torvalds/linux and GRAFT_LINUX_CHAIN to a file of first-parent commits, oldest
// first, ending at HEAD (`git log --first-parent --format=%H -n 61 | tac`). The shard's repository borrows the
// clone's objects through alternates, so the clone is not changed. The tip walks the chain one move at a time, each
// move a /compose; then misses on the tip and on a pull request's head are timed, composed and stock, with CPU as the
// rusage of the git processes the server ran. Results go to the test log (tests/m0/results/composed-miss.md).
func TestLinuxComposedMissCost(t *testing.T) {
	src, chainList := os.Getenv("GRAFT_LINUX"), os.Getenv("GRAFT_LINUX_CHAIN")
	if src == "" || chainList == "" {
		t.Skip("GRAFT_LINUX and GRAFT_LINUX_CHAIN not set")
	}
	cb, err := os.ReadFile(chainList)
	if err != nil {
		t.Fatal(err)
	}
	chain := strings.Fields(string(cb))
	data = t.TempDir()
	s := httptest.NewServer(mux())
	defer s.Close()
	call(t, s, "POST", "/init?net=7")
	rp := repo("7")
	os.WriteFile(filepath.Join(rp, "objects", "info", "alternates"), []byte(filepath.Join(src, "objects")+"\n"), 0o644)
	setTip := func(c string) { run(t, rp, "", "update-ref", "refs/heads/main", c) }
	cpu := func() time.Duration {
		var u, v syscall.Rusage
		syscall.Getrusage(syscall.RUSAGE_CHILDREN, &u)
		syscall.Getrusage(syscall.RUSAGE_SELF, &v)
		return time.Duration(u.Utime.Nano() + u.Stime.Nano() + v.Utime.Nano() + v.Stime.Nano())
	}
	setTip(chain[0])
	c0, t0 := cpu(), time.Now()
	compose(t, s.URL, "&start=1")
	t.Logf("base at %s: cpu %.2f s, wall %.2f s", chain[0][:8], (cpu() - c0).Seconds(), time.Since(t0).Seconds())
	var parts []float64
	for i, c := range chain[1:] {
		setTip(c)
		c0, t0 = cpu(), time.Now()
		o := compose(t, s.URL, "")
		d := (cpu() - c0).Seconds()
		kind := "part"
		if h := o["head"].(map[string]any); len(h["parts"].([]any)) == 0 {
			kind = "fold"
		}
		parts = append(parts, d)
		t.Logf("move %d %s: %s cpu %.2f s, wall %.2f s", i+1, c[:8], kind, d, time.Since(t0).Seconds())
	}
	sort.Float64s(parts)
	t.Logf("moves: p50 %.2f s, max %.2f s CPU", parts[len(parts)/2], parts[len(parts)-1])
	tip := chain[len(chain)-1]

	// A pull request's head on the tip: one file changed, one added, written into the shard's own objects.
	idx := filepath.Join(t.TempDir(), "index")
	env := append(os.Environ(), "GIT_INDEX_FILE="+idx, "GIT_DIR="+rp)
	sh := func(stdin string, args ...string) string {
		c := exec.Command("git", args...)
		c.Env = append(env, "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@t", "GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@t")
		c.Stdin = strings.NewReader(stdin)
		out, err := c.CombinedOutput()
		if err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
		return strings.TrimSpace(string(out))
	}
	sh("", "read-tree", tip)
	mk := sh(sh("", "show", tip+":Makefile")+"\n# pr\n", "hash-object", "-w", "--stdin")
	nw := sh("new file from a pull request\n", "hash-object", "-w", "--stdin")
	sh("", "update-index", "--cacheinfo", "100644,"+mk+",Makefile", "--add", "--cacheinfo", "100644,"+nw+",pr/new.txt")
	pr := sh("pr\n", "commit-tree", sh("", "write-tree"), "-p", tip)
	run(t, rp, "", "update-ref", "refs/pull/1/head", pr)

	miss := func(name, want string) []byte {
		c0, t0 := cpu(), time.Now()
		req, _ := http.NewRequest("POST", s.URL+"/upload-pack?net=7", strings.NewReader(fetchBody(want)))
		req.Header.Set("git-protocol", "version=2")
		r, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		// Time to the first byte, and to the first 64 KiB of pack data: upload-pack's `shallow-info` leaves at once, and
		// its pack only after the delta search, which is what M0-WP9's time to first byte measured.
		var got []byte
		var first, pack64 time.Duration
		buf := make([]byte, 1<<16)
		for {
			n, err := r.Body.Read(buf)
			if n > 0 && first == 0 {
				first = time.Since(t0)
			}
			got = append(got, buf[:n]...)
			if pack64 == 0 {
				if i := bytes.Index(got, []byte("PACK")); i >= 0 && len(got)-i >= 64<<10 {
					pack64 = time.Since(t0)
				}
			}
			if err != nil {
				break
			}
		}
		r.Body.Close()
		t.Logf("%s: %s, first byte %.3f s, first 64 KiB of pack %.3f s, wall %.2f s, cpu %.2f s (server and client), %d bytes",
			name, r.Header.Get("x-graft-compose"), first.Seconds(), pack64.Seconds(), time.Since(t0).Seconds(),
			(cpu() - c0).Seconds(), len(got))
		return got
	}
	for _, w := range []struct{ name, want string }{{"tip", tip}, {"pull head", pr}} {
		for range 2 {
			miss("composed "+w.name, w.want)
		}
	}
	b := miss("composed pull head (checked)", pr)
	c0 = cpu()
	cloneInto(t, b, pr, run(t, rp, "", "rev-parse", pr+"^{tree}"))
	t.Logf("client side: index-pack --strict, fsck --strict, checkout: cpu %.2f s", (cpu() - c0).Seconds())
	// Stock: the same requests with no chain.
	head := filepath.Join(composeDir("7"), "head.json")
	hb, _ := os.ReadFile(head)
	os.Remove(head)
	for _, w := range []struct{ name, want string }{{"tip", tip}, {"pull head", pr}} {
		miss("stock "+w.name, w.want)
	}
	os.WriteFile(head, hb, 0o644)
	var h chainHead
	json.Unmarshal(hb, &h)
	var sizes []string
	for _, p := range h.packs() {
		st, _ := os.Stat(chainFile("7", p, "pack"))
		sizes = append(sizes, fmt.Sprintf("%s %d", strings.SplitN(p, "-", 2)[0], st.Size()))
	}
	t.Logf("chain: %d packs: %s", len(h.packs()), strings.Join(sizes[:min(3, len(sizes))], ", "))
}
