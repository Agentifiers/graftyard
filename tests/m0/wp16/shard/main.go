// shard: the M0-WP16 predictor container's server, cut down from the B0-22 sink (tests/m0/b0-22/sink/main.go).
//
//	GET  /health                 answers once the server is up
//	PUT  /f?path=P               writes the request body to /data/P (the object streams the network's pack from R2)
//	POST /setup                  git init --bare /data/net.git, index the pack, copy /data/shallow (the history
//	                             boundary) to net.git/shallow, write refs from /data/refs
//	POST /predict                body: {"base","head","others":[...],"budget":0.5,"vcpu":4}; one InsightJob. Lands
//	                             head on base as a merge commit (`merge-tree base head`, then `commit-tree`), checks
//	                             each other lands on base, then runs `merge-tree <landed head> <other>` with git's own
//	                             merge base, as the queue would merge them; at most budget*vcpu git processes at once.
//	                             Answers each pair's verdict (Conflict, Clean, HeadStale, OtherStale) and files
//	GET  /cpu                    the container's cgroup CPU usage in microseconds, its CPU count, and merge-trees run
//
// The predictor's share (PREDICT_CPU) is enforced by concurrency: at most budget*vcpu merge-trees run at once. The
// cgroup counter is what the measurement reads.
package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

const repo = "/data/net.git"

// The predictor's CPU share is one semaphore for the whole container, sized on the first /predict call as
// PREDICT_CPU * vCPU, so concurrent jobs queue for it instead of each getting their own share.
var (
	semOnce sync.Once
	sem     chan struct{}
	runs    atomic.Int64 // git merge-tree processes started, read by GET /cpu
	// landed caches merge-tree(base, x) per (base, x): a ref's tree on a base is the same for every job until base moves.
	landedMu sync.Mutex
	landed   = map[string]string{}
	hits     atomic.Int64
	errs     atomic.Int64
)

func cpuUsec() int64 {
	// cgroup v2, then v1.
	if b, err := os.ReadFile("/sys/fs/cgroup/cpu.stat"); err == nil {
		sc := bufio.NewScanner(strings.NewReader(string(b)))
		for sc.Scan() {
			f := strings.Fields(sc.Text())
			if len(f) == 2 && f[0] == "usage_usec" {
				v, _ := strconv.ParseInt(f[1], 10, 64)
				return v
			}
		}
	}
	if b, err := os.ReadFile("/sys/fs/cgroup/cpuacct/cpuacct.usage"); err == nil {
		v, _ := strconv.ParseInt(strings.TrimSpace(string(b)), 10, 64)
		return v / 1000
	}
	return -1
}

type verdict struct {
	Conflict bool     `json:"conflict"`
	Tree     string   `json:"tree"`
	Files    []string `json:"files,omitempty"`
	Ms       int64    `json:"ms"`
	Err      string   `json:"err,omitempty"`
}

func mergeTree(args ...string) verdict {
	runs.Add(1)
	t := time.Now()
	cmd := exec.Command("git", append([]string{"-C", repo, "merge-tree", "--write-tree", "--name-only", "--no-messages"}, args...)...)
	out, err := cmd.Output()
	v := verdict{Ms: time.Since(t).Milliseconds()}
	if err != nil {
		if ee, ok := err.(*exec.ExitError); ok && ee.ExitCode() == 1 {
			v.Conflict = true
		} else {
			v.Err = err.Error()
			return v
		}
	}
	lines := strings.Split(strings.TrimSpace(string(out)), "\n")
	v.Tree = lines[0]
	if v.Conflict && len(lines) > 1 {
		v.Files = lines[1:]
		if len(v.Files) > 8 {
			v.Files = v.Files[:8]
		}
	}
	return v
}

func main() {
	http.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) { fmt.Fprint(w, "ok") })
	http.HandleFunc("/cpu", func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprintf(w, `{"usec":%d,"ncpu":%d,"runs":%d,"hits":%d,"errs":%d,"at":%d}`, cpuUsec(), runtime.NumCPU(), runs.Load(), hits.Load(), errs.Load(), time.Now().UnixMilli())
	})
	http.HandleFunc("/f", func(w http.ResponseWriter, r *http.Request) {
		p := filepath.Clean("/data/" + r.URL.Query().Get("path"))
		if !strings.HasPrefix(p, "/data/") {
			http.Error(w, "bad path", 400)
			return
		}
		os.MkdirAll(filepath.Dir(p), 0o755)
		f, err := os.Create(p + ".part")
		if err != nil {
			http.Error(w, err.Error(), 500)
			return
		}
		n, err := io.CopyBuffer(f, r.Body, make([]byte, 1<<20))
		f.Close()
		if err == nil {
			err = os.Rename(p+".part", p)
		}
		if err != nil {
			http.Error(w, err.Error(), 500)
			return
		}
		fmt.Fprintf(w, `{"bytes":%d}`, n)
	})
	http.HandleFunc("/setup", func(w http.ResponseWriter, r *http.Request) {
		t := time.Now()
		sh := `set -e; rm -rf /data/net.git; git init -q --bare /data/net.git; mkdir -p /data/net.git/objects/pack
mv /data/pack.pack /data/net.git/objects/pack/pack-net.pack
git -C /data/net.git index-pack --threads=0 /data/net.git/objects/pack/pack-net.pack >/dev/null
if [ -f /data/shallow ]; then cp /data/shallow /data/net.git/shallow; fi
git -C /data/net.git update-ref --stdin < /data/refs
git -C /data/net.git count-objects -v | tr '\n' ' '`
		out, err := exec.Command("sh", "-c", sh).CombinedOutput()
		if err != nil {
			http.Error(w, string(out)+err.Error(), 500)
			return
		}
		fmt.Fprintf(w, `{"ms":%d,"out":%q}`, time.Since(t).Milliseconds(), string(out))
	})
	http.HandleFunc("/predict", func(w http.ResponseWriter, r *http.Request) {
		// One InsightJob: land head and every other ref on base, then merge the two landed trees with base as the merge
		// base. A side that conflicts with base itself is Stale for that pair.
		var req struct {
			Base   string   `json:"base"`
			Head   string   `json:"head"`
			Others []string `json:"others"`
			Budget float64  `json:"budget"`
			Vcpu   float64  `json:"vcpu"`
		}
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, err.Error(), 400)
			return
		}
		par := int(req.Budget * req.Vcpu)
		if par < 1 {
			par = 1
		}
		semOnce.Do(func() { sem = make(chan struct{}, par) })
		c0, t0 := cpuUsec(), time.Now()
		run := func(args ...string) verdict { sem <- struct{}{}; defer func() { <-sem }(); return mergeTree(args...) }
		// land(x): merge-tree(base, x), cached per (base, x); "" when x conflicts with base itself (Stale).
		land := func(x string) (string, int64) {
			k := req.Base + " " + x
			landedMu.Lock()
			t, ok := landed[k]
			landedMu.Unlock()
			if ok {
				hits.Add(1)
				return t, 0
			}
			v := run(req.Base, x)
			if v.Err != "" {
				errs.Add(1) // counted, never cached: a git error is not a verdict
				return "", v.Ms
			}
			if v.Conflict {
				v.Tree = ""
			}
			landedMu.Lock()
			landed[k] = v.Tree
			landedMu.Unlock()
			return v.Tree, v.Ms
		}
		// The head as the queue would land it: a merge commit of base and head with land(head)'s tree. Each other ref is
		// then merged into that commit with git's own merge base, so commits both branches carry count once.
		th, mh := land(req.Head)
		hc := ""
		if th != "" {
			cmd := exec.Command("git", "-C", repo, "commit-tree", th, "-p", req.Base, "-p", req.Head, "-m", "predict")
			cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=p", "GIT_AUTHOR_EMAIL=p@example.com", "GIT_COMMITTER_NAME=p", "GIT_COMMITTER_EMAIL=p@example.com",
				"GIT_AUTHOR_DATE=1785542400 +0000", "GIT_COMMITTER_DATE=1785542400 +0000")
			b, err := cmd.Output()
			if err != nil {
				errs.Add(1)
				http.Error(w, "commit-tree: "+err.Error(), 500)
				return
			}
			hc = strings.TrimSpace(string(b))
		}
		out := make([]map[string]any, len(req.Others))
		var wg sync.WaitGroup
		for i, o := range req.Others {
			wg.Add(1)
			go func(i int, o string) {
				defer wg.Done()
				if hc == "" {
					out[i] = map[string]any{"v": "HeadStale"}
					return
				}
				to, ms := land(o)
				if to == "" {
					out[i] = map[string]any{"v": "OtherStale", "ms": ms}
					return
				}
				v := run(hc, o)
				k := "Clean"
				if v.Conflict {
					k = "Conflict"
				}
				if v.Err != "" {
					errs.Add(1)
					k = "Err:" + v.Err
				}
				out[i] = map[string]any{"v": k, "files": v.Files, "ms": ms + v.Ms}
			}(i, o)
		}
		wg.Wait()
		json.NewEncoder(w).Encode(map[string]any{"verdicts": out, "headMs": mh, "cpuUsec": cpuUsec() - c0, "wallMs": time.Since(t0).Milliseconds(), "par": par})
	})
	http.ListenAndServe(":8080", nil)
}
