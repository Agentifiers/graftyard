package main

import (
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

// Validates of one network run side by side under the network's shared lock, beside the writers that take it alone:
// ingest adopting validated packs, PUT /refs, and the geometric repack /refs starts past repackAt. Packs overlap (every
// branch carries the same blob, and some branches push byte-identical packs), and every pack is pushed thin. Afterwards
// every validated pack ingests, every tip is reachable, and `git fsck --strict` finds nothing.
func TestConcurrentValidatesOfOneNetwork(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commit(t, wt, map[string]string{"README": "r\n", "shared.txt": strings.Repeat("s", 4000) + "\n"})
	run(t, wt, "", "push", "-q", repo("7"), "main")
	run(t, repo("7"), "", "symbolic-ref", "HEAD", "refs/heads/main")
	// Past repackAt packs, so each PUT /refs below starts a repack while validates run.
	prev := base
	for i := 0; i < repackAt+2; i++ {
		tip := commit(t, wt, map[string]string{"pad": fmt.Sprintf("%d\n", i)})
		idx := exec.Command("git", "index-pack", "--stdin", "--fix-thin")
		idx.Dir = repo("7")
		idx.Stdin = strings.NewReader(string(packOfWt(t, wt, tip, prev)))
		if out, err := idx.CombinedOutput(); err != nil {
			t.Fatalf("index-pack: %v %s", err, out)
		}
		prev = tip
	}
	main := prev
	putRefs(t, s.URL, main)

	// 12 distinct branches, each pushed 3 times with the same pack bytes: 36 validates.
	const distinct, copies = 12, 3
	type push struct {
		ref, tip string
		pack     []byte
	}
	var pushes []push
	for i := 0; i < distinct; i++ {
		run(t, wt, "", "checkout", "-q", "-B", fmt.Sprintf("b%d", i), main)
		tip := commit(t, wt, map[string]string{
			"shared.txt":          strings.Repeat("s", 4000) + "\nshared tail\n",
			fmt.Sprintf("f%d", i): strings.Repeat(fmt.Sprintf("%d", i), 3000) + "\n",
		})
		pack := packOfWt(t, wt, tip, main)
		for c := 0; c < copies; c++ {
			pushes = append(pushes, push{fmt.Sprintf("refs/heads/b%d", i), tip, pack})
		}
	}

	z := strings.Repeat("0", 40)
	var mu sync.Mutex
	sums := map[string]string{} // ref -> validated pack sum
	var wg sync.WaitGroup
	errs := make(chan string, len(pushes)+8)
	stop := make(chan struct{})
	// The writers: PUT /refs of the ingested refs (each may start a repack), and /ingest of every validated pack so
	// far, in a loop while the validates run.
	var writers sync.WaitGroup
	writers.Add(1)
	go func() {
		defer writers.Done()
		for {
			select {
			case <-stop:
				return
			default:
			}
			mu.Lock()
			ingest := map[string]string{}
			for r, s := range sums {
				ingest[r] = s
			}
			mu.Unlock()
			body := main + " refs/heads/main\n"
			for ref, sum := range ingest {
				if c, v := call(t, s, "POST", "/ingest?net=7&sha256="+sum); c != 200 {
					errs <- fmt.Sprintf("ingest %s: %d %v", ref, c, v)
					return
				}
				for _, p := range pushes {
					if p.ref == ref {
						body += p.tip + " " + ref + "\n"
						break
					}
				}
			}
			req, _ := http.NewRequest("PUT", s.URL+"/refs?net=7", strings.NewReader(body))
			r, err := http.DefaultClient.Do(req)
			if err != nil {
				errs <- err.Error()
				return
			}
			r.Body.Close()
			if r.StatusCode != 200 {
				errs <- fmt.Sprintf("PUT /refs: %d", r.StatusCode)
				return
			}
			time.Sleep(5 * time.Millisecond)
		}
	}()
	for _, p := range pushes {
		wg.Add(1)
		go func(p push) {
			defer wg.Done()
			code, o, body := validateDetail(t, s.URL, []string{p.tip}, []update{{p.tip, z, p.ref}}, p.pack)
			if code != 200 || o.Checksum == "" || len(o.Detail) != 1 {
				errs <- fmt.Sprintf("validate %s: %d %s", p.ref, code, body)
				return
			}
			mu.Lock()
			if old, ok := sums[p.ref]; ok && old != o.Checksum {
				errs <- fmt.Sprintf("%s: same bytes, sums %s and %s", p.ref, old, o.Checksum)
			}
			sums[p.ref] = o.Checksum
			mu.Unlock()
		}(p)
	}
	wg.Wait()
	close(stop)
	writers.Wait()
	close(errs)
	for e := range errs {
		t.Fatal(e)
	}
	if len(sums) != distinct {
		t.Fatalf("%d refs validated, want %d", len(sums), distinct)
	}

	// Every validated pack is either ingested already or still set aside, whole, under its sum.
	body := main + " refs/heads/main\n"
	for i := 0; i < len(pushes); i += copies {
		p := pushes[i]
		sum := sums[p.ref]
		if c, v := call(t, s, "POST", "/ingest?net=7&sha256="+sum); c != 200 {
			t.Fatalf("ingest %s: %d %v", p.ref, c, v)
		}
		if _, err := os.Stat(filepath.Join(repo("7"), "graft", sum+".objs")); err != nil {
			t.Fatalf("%s: not recorded: %v", p.ref, err)
		}
		body += p.tip + " " + p.ref + "\n"
	}
	req, _ := http.NewRequest("PUT", s.URL+"/refs?net=7", strings.NewReader(body))
	r, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	r.Body.Close()
	repacking.Lock() // any repack has finished
	repacking.Unlock()
	l := lock("7")
	l.Lock()
	l.Unlock()
	for i := 0; i < len(pushes); i += copies {
		p := pushes[i]
		if got := run(t, repo("7"), "", "rev-parse", p.ref); got != p.tip {
			t.Fatalf("%s = %s, want %s", p.ref, got, p.tip)
		}
		run(t, repo("7"), "", "rev-list", "--objects", "--quiet", p.tip)
	}
	if out := run(t, repo("7"), "", "fsck", "--strict", "--no-dangling"); out != "" {
		t.Fatalf("fsck: %s", out)
	}
	left, _ := filepath.Glob(filepath.Join(repo("7"), "graft", "incoming-*"))
	if len(left) != 0 {
		t.Fatalf("quarantines left behind: %v", left)
	}
}
