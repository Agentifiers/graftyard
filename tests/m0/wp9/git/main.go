// wp9git: the M0-WP9 B0-34 git cache shard's server (tests/m0/src/wp9/git.ts starts it in a container).
//
//	GET  /health                  answers once the server is up
//	PUT  /f?path=P                writes the request body to /data/P (the shard object streams R2 objects in)
//	POST /setup?net=N             builds /data/N.git from /data/in/N: packs and indexes, commit-graph, refs, the base
//	                              tip pack and its object list (the clone-cache tip pack, for compose misses)
//	GET  /load                    cpu use over the last 10 s (from /proc/stat), queue depth per class, slots
//	POST /op?net=N&kind=K&arg=A&cls=C   one unit of git work through the shard's work queue (below)
//
// The work queue has one slot per vCPU and serves Human, then MergeQueue, then Bot, then Agent (`AdmitClass`). An Agent
// job still waiting after SHED_S is shed (503), as the design's shard queue sheds agent work first. Nothing preempts a
// running job. Kinds:
//
//	push    an agent push of real commit C: head A = C's tree on C's parent (deterministic, so every shard derives the
//	        same A), a thin pack of A ^C^ (the client's side, not timed), then the receive side, timed:
//	        `index-pack --stdin --fix-thin` into a quarantine and `rev-list --objects A --not --all`, then the head-push
//	        test merge `merge-tree --write-tree <tip> A`.
//	human   a human push of real commit C: the same receive side, no test merge.
//	merge   a merge-queue merge: `merge-tree --write-tree P1 P2` for a real pull (arg "P1 P2").
//	miss    a clone-cache miss for the merge ref of agent head A of C (its test merge onto the tip, as
//	        `refs/pull/N/merge`), depth 1: mode=stock runs `pack-objects --revs --shallow` as
//	        upload-pack does; mode=compose sends the base tip pack plus a pack of A's tree objects missing from it,
//	        with one header and trailer (`wp9git compose`). The response streams the first 64 KiB as soon as they
//	        exist (for time to first byte), discards the rest, and ends with a JSON line of stats.
//
// CPU per job is its git children's user + sys time (rusage).
package main

import (
	"bufio"
	"bytes"
	"container/heap"
	"crypto/sha1"
	"encoding"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"
)

const shedS = 20

var classRank = map[string]int{"Human": 0, "MergeQueue": 1, "Bot": 2, "Agent": 3}

// ---- work queue ----

type job struct {
	rank, seq int
	at        time.Time
	ready     chan bool // true: run; false: shed
}
type jobHeap []*job

func (h jobHeap) Len() int { return len(h) }
func (h jobHeap) Less(i, j int) bool {
	if h[i].rank != h[j].rank {
		return h[i].rank < h[j].rank
	}
	return h[i].seq < h[j].seq
}
func (h jobHeap) Swap(i, j int) { h[i], h[j] = h[j], h[i] }
func (h *jobHeap) Push(x any)   { *h = append(*h, x.(*job)) }
func (h *jobHeap) Pop() any     { o := *h; n := len(o); x := o[n-1]; *h = o[:n-1]; return x }

// SLOTS (the instance's vCPUs) comes from the start options, because NumCPU can report the host's count.
func envInt(k string, d int) int {
	if v, err := strconv.Atoi(os.Getenv(k)); err == nil && v > 0 {
		return v
	}
	return d
}

var (
	qMu    sync.Mutex
	q      jobHeap
	slots  = envInt("SLOTS", runtime.NumCPU())
	free   = slots
	seq    int
	shed   = map[string]int{}
	queued = map[int]int{}
)

func acquire(cls string) (time.Duration, bool) {
	t := time.Now()
	j := &job{rank: classRank[cls], at: t, ready: make(chan bool, 1)}
	qMu.Lock()
	seq++
	j.seq = seq
	if free > 0 && q.Len() == 0 {
		free--
		qMu.Unlock()
		return 0, true
	}
	heap.Push(&q, j)
	queued[j.rank]++
	qMu.Unlock()
	ok := <-j.ready
	return time.Since(t), ok
}

func release() {
	qMu.Lock()
	defer qMu.Unlock()
	for q.Len() > 0 {
		j := heap.Pop(&q).(*job)
		queued[j.rank]--
		if j.rank == 3 && time.Since(j.at) > shedS*time.Second {
			shed["Agent"]++
			j.ready <- false
			continue
		}
		j.ready <- true
		return
	}
	free++
}

// ---- cpu ----

var (
	cpuMu   sync.Mutex
	cpuHist []float64 // one sample a second: share of all vCPUs busy
)

// readStat returns busy and total CPU time. With a cgroup v2 cpu.stat it is the container's own usage against
// wall time x SLOTS; otherwise the machine's /proc/stat.
var cgStart = time.Now()

func readStat() (busy, total uint64) {
	if b, err := os.ReadFile("/sys/fs/cgroup/cpu.stat"); err == nil {
		for _, l := range strings.Split(string(b), "\n") {
			if f := strings.Fields(l); len(f) == 2 && f[0] == "usage_usec" {
				u, _ := strconv.ParseUint(f[1], 10, 64)
				return u, uint64(time.Since(cgStart).Microseconds()) * uint64(slots)
			}
		}
	}
	b, _ := os.ReadFile("/proc/stat")
	line := strings.SplitN(string(b), "\n", 2)[0]
	f := strings.Fields(line)[1:]
	for i, x := range f {
		v, _ := strconv.ParseUint(x, 10, 64)
		total += v
		if i != 3 && i != 4 { // idle, iowait
			busy += v
		}
	}
	return
}

func sampleCPU() {
	pb, pt := readStat()
	for range time.Tick(time.Second) {
		b, t := readStat()
		u := 0.0
		if t > pt {
			u = float64(b-pb) / float64(t-pt)
		}
		pb, pt = b, t
		cpuMu.Lock()
		cpuHist = append(cpuHist, u)
		if len(cpuHist) > 600 {
			cpuHist = cpuHist[len(cpuHist)-600:]
		}
		cpuMu.Unlock()
	}
}

// ---- git helpers ----

type usage struct{ cpu float64 }

func (u *usage) add(c *exec.Cmd) {
	if c.ProcessState != nil {
		if r, ok := c.ProcessState.SysUsage().(*syscall.Rusage); ok {
			u.cpu += float64(r.Utime.Sec+r.Stime.Sec) + float64(r.Utime.Usec+r.Stime.Usec)/1e6
		}
	}
}

var detEnv = []string{"GIT_AUTHOR_NAME=wp9", "GIT_AUTHOR_EMAIL=wp9@example.invalid", "GIT_COMMITTER_NAME=wp9",
	"GIT_COMMITTER_EMAIL=wp9@example.invalid", "GIT_AUTHOR_DATE=1790000000 +0000", "GIT_COMMITTER_DATE=1790000000 +0000"}

func gitCmd(env []string, stdin io.Reader, args ...string) *exec.Cmd {
	c := exec.Command("git", args...)
	c.Env = append(append(os.Environ(), env...), detEnv...)
	c.Stdin = stdin
	return c
}

func run(u *usage, env []string, stdin io.Reader, args ...string) (string, error) {
	c := gitCmd(env, stdin, args...)
	var out, errb bytes.Buffer
	c.Stdout, c.Stderr = &out, &errb
	err := c.Run()
	if u != nil {
		u.add(c)
	}
	if err != nil {
		return out.String(), fmt.Errorf("git %s: %v %s", args[0], err, strings.TrimSpace(errb.String()))
	}
	return strings.TrimSpace(out.String()), nil
}

// netInfo is one network on this shard. Third pass: the tip moves along `chain` (real first-parent history, oldest
// first) as the merge queue lands. The composed-miss base is the depth-1 pack of chain[baseIdx]; for each tip move a
// background worker (nice 19, outside the slot queue) packs the tip's objects the base and earlier parts lack into one
// part, and precomputes the trailer hash state for base + parts, so a miss sends base + parts + a pack of the merge
// commit's few new objects. When the tip is RENEW_AT moves past the base, the worker folds base + parts into a new base.
type netInfo struct {
	mu       sync.Mutex
	name     string
	dir, tip string
	base     map[string]bool // objects in base + parts
	baseN    uint32
	basePath string
	parts    []string // part files after the base, in order
	partsN   uint32   // objects in parts
	chain    []string
	tipIdx   int
	baseIdx  int
	builtIdx int // chain index the base + parts cover
	renewals []map[string]any
	ready    bool // false while setup catches up: the worker builds without nice and with no delta search
}

const renewAt = 20

type covered struct {
	tip, builtTip string
	files         []string
	set           map[string]bool
	n             uint32
	behind        int
}

func (ni *netInfo) snapshot() covered {
	ni.mu.Lock()
	defer ni.mu.Unlock()
	return covered{tip: ni.tip, builtTip: ni.chain[ni.builtIdx], files: append([]string{ni.basePath}, ni.parts...), set: ni.base,
		n: ni.baseN + ni.partsN, behind: ni.tipIdx - ni.baseIdx}
}

// moveTip advances the tip to chain[n]; the worker builds the parts.
func (ni *netInfo) moveTip(n int) {
	ni.mu.Lock()
	defer ni.mu.Unlock()
	if n >= len(ni.chain) {
		n = len(ni.chain) - 1
	}
	if n > ni.tipIdx {
		ni.tipIdx, ni.tip = n, ni.chain[n]
	}
}

func (ni *netInfo) isReady() bool { ni.mu.Lock(); defer ni.mu.Unlock(); return ni.ready }

func packCount(path string) uint32 {
	h, err := os.Open(path)
	if err != nil {
		return 0
	}
	defer h.Close()
	b := make([]byte, 12)
	io.ReadFull(h, b)
	return binary.BigEndian.Uint32(b[8:12])
}

// worker builds one part per tip move (or one part for a catch-up of several moves), precomputes the hash state, and
// folds every RENEW_AT moves. Each step is recorded in `renewals` with its CPU.
func (ni *netInfo) worker() {
	for {
		time.Sleep(200 * time.Millisecond)
		ni.mu.Lock()
		built, target, set := ni.builtIdx, ni.tipIdx, ni.base
		ni.mu.Unlock()
		if target <= built || ni.basePath == "" {
			continue
		}
		t := time.Now()
		var u usage
		rec := map[string]any{"at": t.UnixMilli(), "from": built, "idx": target, "kind": "part"}
		diff, err := run(&u, []string{"GIT_DIR=" + ni.dir}, nil, "diff-tree", "-r", "-t", ni.chain[built], ni.chain[target])
		root, err2 := run(&u, []string{"GIT_DIR=" + ni.dir}, nil, "rev-parse", ni.chain[target]+"^{tree}")
		if err == nil {
			err = err2
		}
		var list bytes.Buffer
		add := []string{}
		if err == nil {
			want := []string{root}
			for _, l := range strings.Split(diff, "\n") {
				if f := strings.Fields(l); len(f) >= 5 && len(f[3]) == 40 && strings.Trim(f[3], "0") != "" {
					want = append(want, f[3])
				}
			}
			seen := map[string]bool{}
			for _, o := range want {
				if !set[o] && !seen[o] {
					seen[o] = true
					add = append(add, o)
					list.WriteString(o + "\n")
				}
			}
		}
		part := filepath.Join("/data", fmt.Sprintf("%s.part.%d.pack", ni.name, target))
		if err == nil {
			f, _ := os.Create(part)
			// r6: normal priority. At nice 19 a saturated shard starved its own worker (r5: a 10 CPU-s part took 43 s),
			// and every miss meanwhile packed the missing objects itself.
			args := []string{"-n", "0", "git", "pack-objects", "--stdout", "--delta-base-offset", "--threads=1"}
			if !ni.isReady() {
				// Catch-up at setup: a shard that joins late builds one part for many moves before it serves, so it
				// skips the delta search (B0-34: `--window=0` cut a depth-1 pack from 25 to 9 CPU-s for 0.5% more bytes).
				args = []string{"-n", "0", "git", "pack-objects", "--stdout", "--delta-base-offset", "--threads=4", "--window=0"}
			}
			pk := exec.Command("nice", args...)
			pk.Env = append(append(os.Environ(), "GIT_DIR="+ni.dir), detEnv...)
			pk.Stdin, pk.Stdout = &list, f
			err = pk.Run()
			f.Close()
			u.add(pk)
		}
		if err != nil {
			rec["error"] = err.Error()
			ni.mu.Lock()
			ni.renewals = append(ni.renewals, rec)
			ni.mu.Unlock()
			time.Sleep(5 * time.Second)
			continue
		}
		pn := packCount(part)
		next := make(map[string]bool, len(set)+len(add))
		for k := range set {
			next[k] = true
		}
		for _, o := range add {
			next[o] = true
		}
		ni.mu.Lock()
		ni.parts = append(ni.parts, part)
		ni.partsN += pn
		ni.base, ni.builtIdx = next, target
		files := append([]string{ni.basePath}, ni.parts...)
		n := ni.baseN + ni.partsN
		fold := target-ni.baseIdx >= renewAt
		ni.mu.Unlock()
		rec["added"], rec["partBytes"] = len(add), fileSize(part)
		if fold {
			// Fold: base + parts become one pack file with a new header and trailer; no delta search.
			nb := filepath.Join("/data", fmt.Sprintf("%s.base.%d.pack", ni.name, target))
			f, _ := os.Create(nb)
			c := exec.Command("nice", append([]string{"-n", "19", "/wp9git", "compose"}, files...)...)
			c.Stdout = f
			err = c.Run()
			f.Close()
			u.add(c)
			rec["kind"], rec["baseBytes"] = "part+fold", fileSize(nb)
			if err == nil {
				ni.mu.Lock()
				old := append([]string{ni.basePath}, ni.parts...)
				ni.basePath, ni.baseN, ni.parts, ni.partsN, ni.baseIdx = nb, n, nil, 0, target
				files = []string{nb}
				ni.mu.Unlock()
				time.AfterFunc(time.Minute, func() {
					for _, o := range old {
						os.Remove(o)
						if m, _ := filepath.Glob(o + ".sha.*"); m != nil {
							for _, x := range m {
								os.Remove(x)
							}
						}
					}
				})
			} else {
				rec["foldError"] = err.Error()
			}
		}
		// Precompute the hash state for the common miss: one more object (the merge commit; its tree equals the tip's).
		c := exec.Command("nice", append([]string{"-n", "19", "/wp9git", "hashstate", strconv.Itoa(int(n + 1))}, files...)...)
		c.Run()
		u.add(c)
		rec["cpu"], rec["ms"] = u.cpu, time.Since(t).Milliseconds()
		ni.mu.Lock()
		ni.renewals = append(ni.renewals, rec)
		ni.mu.Unlock()
	}
}

func fileSize(p string) int64 {
	if st, err := os.Stat(p); err == nil {
		return st.Size()
	}
	return -1
}

var (
	netsMu sync.Mutex
	nets   = map[string]*netInfo{}
)

func getNet(n string) *netInfo { netsMu.Lock(); defer netsMu.Unlock(); return nets[n] }

func setup(n string, tipIdx int) (map[string]any, error) {
	t := time.Now()
	in := filepath.Join("/data/in", n)
	dir := filepath.Join("/data", n+".git")
	os.RemoveAll(dir)
	if _, err := run(nil, nil, nil, "init", "-q", "--bare", dir); err != nil {
		return nil, err
	}
	// Third pass: files over the upload limit arrive as byte parts `<name>.partNNN`; join them in order first.
	if parts, _ := filepath.Glob(filepath.Join(in, "*.part[0-9][0-9][0-9]")); len(parts) > 0 {
		sort.Strings(parts)
		for _, p := range parts {
			whole := p[:len(p)-len(".part000")]
			f, err := os.OpenFile(whole, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
			if err != nil {
				return nil, err
			}
			src, err := os.Open(p)
			if err == nil {
				_, err = io.Copy(f, src)
				src.Close()
			}
			f.Close()
			if err != nil {
				return nil, err
			}
			os.Remove(p)
		}
	}
	ents, _ := os.ReadDir(in)
	packs := 0
	for _, e := range ents {
		name := e.Name()
		switch {
		case strings.HasPrefix(name, "pack-"):
			os.Rename(filepath.Join(in, name), filepath.Join(dir, "objects/pack", name))
			if strings.HasSuffix(name, ".pack") {
				packs++
			}
		case name == "commit-graph":
			os.MkdirAll(filepath.Join(dir, "objects/info"), 0o755)
			os.Rename(filepath.Join(in, name), filepath.Join(dir, "objects/info/commit-graph"))
		}
	}
	refs, _ := os.ReadFile(filepath.Join(in, "refs"))
	tip := strings.Fields(string(refs))[0]
	if _, err := run(nil, nil, nil, "-C", dir, "update-ref", "refs/heads/master", tip); err != nil {
		return nil, err
	}
	ni := &netInfo{name: n, dir: dir, tip: tip, base: map[string]bool{}, chain: []string{tip}}
	if b, err := os.ReadFile(filepath.Join(in, "chain")); err == nil {
		if c := strings.Fields(string(b)); len(c) > 0 && c[0] == tip {
			ni.chain = c
		}
	}
	if b, err := os.ReadFile(filepath.Join(in, "base.objs")); err == nil {
		for _, l := range strings.Split(string(b), "\n") {
			if len(l) >= 40 {
				ni.base[l[:40]] = true
			}
		}
		ni.basePath = filepath.Join("/data", n+".base.pack")
		os.Rename(filepath.Join(in, "base.pack"), ni.basePath)
		if f, err := os.Open(ni.basePath); err == nil {
			h := make([]byte, 12)
			io.ReadFull(f, h)
			ni.baseN = binary.BigEndian.Uint32(h[8:12])
			f.Close()
		}
	}
	for _, f := range []string{"commits", "merges"} {
		os.Rename(filepath.Join(in, f), filepath.Join("/data", n+"."+f))
	}
	netsMu.Lock()
	nets[n] = ni
	netsMu.Unlock()
	go ni.worker()
	// Third pass: catch up to the network's tip before answering, so a shard that joins late serves composed misses
	// from base + one part that covers every move so far, not from a stale base.
	tc := time.Now()
	if tipIdx > 0 {
		ni.moveTip(tipIdx)
		for deadline := time.Now().Add(5 * time.Minute); time.Now().Before(deadline); time.Sleep(200 * time.Millisecond) {
			ni.mu.Lock()
			done := ni.builtIdx >= tipIdx && len(ni.renewals) > 0
			ni.mu.Unlock()
			if done {
				break
			}
		}
	}
	ni.mu.Lock()
	ni.ready = true
	catch := append([]map[string]any(nil), ni.renewals...)
	ni.mu.Unlock()
	return map[string]any{"catchUpMs": time.Since(tc).Milliseconds(), "catchUp": catch, "net": n, "packs": packs, "tip": tip, "chain": len(ni.chain), "baseObjs": len(ni.base), "baseN": ni.baseN, "ms": time.Since(t).Milliseconds()}, nil
}

// agentHead derives agent head A for real commit c, with its objects in objDir (alternates: the network).
func agentHead(u *usage, ni *netInfo, objDir, c string) (string, error) {
	env := []string{"GIT_DIR=" + ni.dir}
	if objDir != "" {
		env = append(env, "GIT_OBJECT_DIRECTORY="+objDir, "GIT_ALTERNATE_OBJECT_DIRECTORIES="+ni.dir+"/objects")
	}
	return run(u, env, nil, "commit-tree", c+"^{tree}", "-p", c+"^", "-m", "wp9 agent head for "+c)
}

var tmpSeq struct {
	sync.Mutex
	n int
}

func tmpDir() string {
	tmpSeq.Lock()
	tmpSeq.n++
	n := tmpSeq.n
	tmpSeq.Unlock()
	d := fmt.Sprintf("/data/tmp/%d", n)
	os.MkdirAll(d+"/q/pack", 0o755)
	os.MkdirAll(d+"/r/pack", 0o755)
	return d
}

// receive runs a push's receive side for real commit c and returns the head; `test` adds the head-push test merge.
func receive(ni *netInfo, c string, test bool) (map[string]any, error) {
	d := tmpDir()
	defer os.RemoveAll(d)
	var client, recv, tm usage
	a, err := agentHead(&client, ni, d+"/q", c)
	if err != nil {
		return nil, err
	}
	qenv := []string{"GIT_DIR=" + ni.dir, "GIT_OBJECT_DIRECTORY=" + d + "/q", "GIT_ALTERNATE_OBJECT_DIRECTORIES=" + ni.dir + "/objects"}
	pk := gitCmd(qenv, strings.NewReader(a+"\n^"+c+"^\n"), "pack-objects", "--revs", "--thin", "--stdout", "--delta-base-offset", "--threads=1")
	var pack bytes.Buffer
	pk.Stdout = &pack
	if err := pk.Run(); err != nil {
		return nil, fmt.Errorf("client pack: %v", err)
	}
	client.add(pk)
	t := time.Now()
	renv := []string{"GIT_DIR=" + ni.dir, "GIT_OBJECT_DIRECTORY=" + d + "/r", "GIT_ALTERNATE_OBJECT_DIRECTORIES=" + ni.dir + "/objects"}
	if _, err := run(&recv, renv, bytes.NewReader(pack.Bytes()), "index-pack", "--stdin", "--fix-thin", "--threads=1"); err != nil {
		return nil, err
	}
	if _, err := run(&recv, renv, nil, "rev-list", "--objects", a, "--not", "--all"); err != nil {
		return nil, err
	}
	recvMs := time.Since(t).Milliseconds()
	out := map[string]any{"head": a, "packBytes": pack.Len(), "clientCpu": client.cpu, "recvCpu": recv.cpu, "recvMs": recvMs}
	if test {
		t2 := time.Now()
		tip := ni.snapshot().tip
		_, err := run(&tm, renv, nil, "merge-tree", "--write-tree", tip, a)
		out["mergeCpu"], out["mergeMs"], out["conflict"] = tm.cpu, time.Since(t2).Milliseconds(), err != nil
	}
	return out, nil
}

// miss streams a depth-1 pack for agent head A of c: the first 64 KiB to w as soon as they exist, the rest discarded.
func miss(w http.ResponseWriter, ni *netInfo, c, mode string, t0 time.Time) (map[string]any, error) {
	var u usage
	h, err := agentHead(&u, ni, "", c) // into the network itself: a shard that took the push holds A
	if err != nil {
		return nil, err
	}
	// CI checks out the PR's merge ref: the test merge of A onto the tip, a commit with parents (tip, A).
	cv := ni.snapshot()
	tip := cv.tip
	// r7 (WP9_MISS_AT_BUILT=1): a composed miss builds the merge ref on the last tip the shard has packed, never on a
	// newer one, so a miss never packs a tip move itself. The ref is at most one part behind the tip, as a lazily
	// computed test merge can be (`TestMerge::Stale`).
	if mode == "compose" && os.Getenv("WP9_MISS_AT_BUILT") == "1" {
		tip = cv.builtTip
	}
	tree, err := run(&u, []string{"GIT_DIR=" + ni.dir}, nil, "merge-tree", "--write-tree", tip, h)
	if err != nil {
		tree = strings.Fields(tree + " x")[0] // a conflicted merge still writes a tree, with markers
	}
	a, err := run(&u, []string{"GIT_DIR=" + ni.dir}, nil, "commit-tree", strings.Fields(tree)[0], "-p", tip, "-p", h, "-m", "wp9 merge ref")
	if err != nil {
		return nil, err
	}
	var src *exec.Cmd
	stats := map[string]any{"head": a, "mode": mode, "behind": cv.behind, "parts": len(cv.files) - 1, "atBuilt": tip != cv.tip}
	switch mode {
	case "stock":
		src = gitCmd([]string{"GIT_DIR=" + ni.dir}, strings.NewReader("--shallow "+a+"\n"+a+"\n"), "pack-objects", "--revs", "--shallow", "--stdout", "--delta-base-offset", "--threads=1")
	case "compose":
		// Third pass: the objects of A's tree that differ from the tree the base and its parts cover (diff-tree of the
		// two trees, new side), plus A's root tree and commit, minus what base and parts hold. For an agent head on the
		// current tip this is the merge commit and the PR's changed paths; the parts already carry the tip's moves.
		diff, err := run(&u, []string{"GIT_DIR=" + ni.dir}, nil, "diff-tree", "-r", "-t", cv.builtTip, a)
		if err != nil {
			return nil, err
		}
		root, err := run(&u, []string{"GIT_DIR=" + ni.dir}, nil, "rev-parse", a+"^{tree}")
		if err != nil {
			return nil, err
		}
		var list bytes.Buffer
		n := 0
		want := []string{a, root}
		for _, l := range strings.Split(diff, "\n") {
			if f := strings.Fields(l); len(f) >= 5 && len(f[3]) == 40 && strings.Trim(f[3], "0") != "" {
				want = append(want, f[3])
			}
		}
		seen := map[string]bool{}
		for _, o := range want {
			if !cv.set[o] && !seen[o] {
				seen[o] = true
				list.WriteString(o + "\n")
				n++
			}
		}
		inc := filepath.Join("/data/tmp", fmt.Sprintf("inc-%d.pack", time.Now().UnixNano()))
		f, _ := os.Create(inc)
		pk := gitCmd([]string{"GIT_DIR=" + ni.dir}, &list, "pack-objects", "--stdout", "--delta-base-offset", "--threads=1")
		pk.Stdout = f
		err = pk.Run()
		f.Close()
		u.add(pk)
		defer os.Remove(inc)
		if err != nil {
			return nil, fmt.Errorf("inc pack: %v", err)
		}
		stats["incObjects"] = n
		src = exec.Command("/wp9git", append([]string{"compose"}, append(cv.files, inc)...)...)
	default:
		return nil, fmt.Errorf("mode %q", mode)
	}
	out, _ := src.StdoutPipe()
	if err := src.Start(); err != nil {
		return nil, err
	}
	fl, _ := w.(http.Flusher)
	buf := make([]byte, 1<<16)
	sent, total := 0, int64(0)
	var ttfb int64 = -1
	for {
		k, rerr := out.Read(buf)
		if k > 0 {
			if ttfb < 0 {
				ttfb = time.Since(t0).Milliseconds()
			}
			total += int64(k)
			if sent < 1<<16 {
				w.Write(buf[:k])
				sent += k
				if fl != nil {
					fl.Flush()
				}
			}
		}
		if rerr != nil {
			break
		}
	}
	err = src.Wait()
	u.add(src)
	stats["cpu"], stats["bytes"], stats["ttfbMs"] = u.cpu, total, ttfb
	if err != nil {
		stats["error"] = err.Error()
	}
	return stats, nil
}

// compose writes packs as one pack (one header, one trailer) to stdout: every object of each is whole or a delta whose
// base is in the same part (ofs-delta) or earlier (ref-delta to an object the base holds), so the client's index-pack
// accepts it. SHA-1's state after the header and every file but the last depends only on those files and the total
// object count, so it is cached beside the last-but-one file as `<file>.sha.<count>` (written by `hashstate` ahead of
// time), and a miss then hashes only the last file.
func compose(paths []string) {
	var total uint32
	for _, p := range paths {
		total += packCount(p)
	}
	hdr := []byte{'P', 'A', 'C', 'K', 0, 0, 0, 2, 0, 0, 0, 0}
	binary.BigEndian.PutUint32(hdr[8:12], total)
	w := bufio.NewWriterSize(os.Stdout, 1<<20)
	w.Write(hdr)
	w.Flush()
	h := sha1.New()
	cache := fmt.Sprintf("%s.sha.%d", paths[len(paths)-2], total)
	cached := false
	if st, err := os.ReadFile(cache); err == nil && h.(encoding.BinaryUnmarshaler).UnmarshalBinary(st) == nil {
		cached = true
	} else {
		h.Reset()
		h.Write(hdr)
	}
	for i, p := range paths {
		f, _ := os.Open(p)
		st, _ := f.Stat()
		f.Seek(12, io.SeekStart)
		if cached && i < len(paths)-1 {
			io.CopyN(w, f, st.Size()-32)
		} else {
			io.CopyN(io.MultiWriter(w, h), f, st.Size()-32)
		}
		f.Close()
	}
	w.Write(h.Sum(nil))
	w.Flush()
}

// hashstate writes the SHA-1 state after a header for `count` objects and every file's body, as compose caches it.
func hashstate(count uint32, paths []string) {
	hdr := []byte{'P', 'A', 'C', 'K', 0, 0, 0, 2, 0, 0, 0, 0}
	binary.BigEndian.PutUint32(hdr[8:12], count)
	h := sha1.New()
	h.Write(hdr)
	for _, p := range paths {
		f, _ := os.Open(p)
		st, _ := f.Stat()
		f.Seek(12, io.SeekStart)
		io.CopyN(h, f, st.Size()-32)
		f.Close()
	}
	if st, err := h.(encoding.BinaryMarshaler).MarshalBinary(); err == nil {
		cache := fmt.Sprintf("%s.sha.%d", paths[len(paths)-1], count)
		os.WriteFile(cache+".tmp", st, 0o644)
		os.Rename(cache+".tmp", cache)
	}
}

func writeJSON(w http.ResponseWriter, v any) { w.Header().Set("content-type", "application/json"); json.NewEncoder(w).Encode(v) }

func main() {
	if len(os.Args) > 3 && os.Args[1] == "compose" {
		compose(os.Args[2:])
		return
	}
	if len(os.Args) > 3 && os.Args[1] == "hashstate" {
		n, _ := strconv.Atoi(os.Args[2])
		hashstate(uint32(n), os.Args[3:])
		return
	}
	os.MkdirAll("/data/tmp", 0o755)
	go sampleCPU()
	http.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) { fmt.Fprint(w, "ok") })
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
		tip, _ := strconv.Atoi(r.URL.Query().Get("tip"))
		out, err := setup(r.URL.Query().Get("net"), tip)
		if err != nil {
			http.Error(w, err.Error(), 500)
			return
		}
		writeJSON(w, out)
	})
	http.HandleFunc("/load", func(w http.ResponseWriter, r *http.Request) {
		cpuMu.Lock()
		h := append([]float64(nil), cpuHist...)
		cpuMu.Unlock()
		last := func(n int) float64 {
			if len(h) == 0 {
				return 0
			}
			if n > len(h) {
				n = len(h)
			}
			s := 0.0
			for _, x := range h[len(h)-n:] {
				s += x
			}
			return s / float64(n)
		}
		qMu.Lock()
		qd := map[string]int{}
		for c, k := range classRank {
			qd[c] = queued[k]
		}
		sh := shed["Agent"]
		fr := free
		qMu.Unlock()
		netsMu.Lock()
		ns := []string{}
		for n := range nets {
			ns = append(ns, n)
		}
		netsMu.Unlock()
		sort.Strings(ns)
		nproc, _ := exec.Command("nproc").Output()
		ren := map[string]any{}
		for _, n := range ns {
			if ni := getNet(n); ni != nil {
				ni.mu.Lock()
				ren[n] = map[string]any{"tipIdx": ni.tipIdx, "baseIdx": ni.baseIdx, "renewals": append([]map[string]any(nil), ni.renewals...)}
				ni.mu.Unlock()
			}
		}
		writeJSON(w, map[string]any{"cpu10": last(10), "cpu60": last(60), "queue": qd, "shed": sh, "free": fr, "slots": slots, "nets": ns, "tips": ren,
			"nproc": strings.TrimSpace(string(nproc))})
	})
	http.HandleFunc("/op", func(w http.ResponseWriter, r *http.Request) {
		t0 := time.Now()
		qv := r.URL.Query()
		ni := getNet(qv.Get("net"))
		if ni == nil {
			http.Error(w, "no such network", 404)
			return
		}
		// Third pass: the client names the tip's position on the chain (the merge queue's clock, 10 merges a minute),
		// so every shard of the network moves its tip alike, in band.
		if n, err := strconv.Atoi(qv.Get("tip")); err == nil {
			ni.moveTip(n)
		}
		wait, ok := acquire(qv.Get("cls"))
		if !ok {
			http.Error(w, "shed", 503)
			return
		}
		defer release()
		var out map[string]any
		var err error
		switch qv.Get("kind") {
		case "push":
			out, err = receive(ni, qv.Get("arg"), true)
		case "human":
			out, err = receive(ni, qv.Get("arg"), false)
		case "merge":
			p := strings.Fields(qv.Get("arg"))
			var u usage
			t := time.Now()
			_, merr := run(&u, []string{"GIT_DIR=" + ni.dir}, nil, "merge-tree", "--write-tree", p[0], p[1])
			out = map[string]any{"mergeCpu": u.cpu, "mergeMs": time.Since(t).Milliseconds(), "conflict": merr != nil}
		case "miss":
			w.Header().Set("content-type", "application/octet-stream")
			out, err = miss(w, ni, qv.Get("arg"), qv.Get("mode"), t0)
			if err == nil {
				out["queueMs"], out["ms"] = wait.Milliseconds(), time.Since(t0).Milliseconds()
				b, _ := json.Marshal(out)
				w.Write(append([]byte("\nWP9STATS"), b...))
				return
			}
		default:
			err = fmt.Errorf("kind %q", qv.Get("kind"))
		}
		if err != nil {
			http.Error(w, err.Error(), 500)
			return
		}
		out["queueMs"], out["ms"] = wait.Milliseconds(), time.Since(t0).Milliseconds()
		writeJSON(w, out)
	})
	http.ListenAndServe(":8080", nil)
}
