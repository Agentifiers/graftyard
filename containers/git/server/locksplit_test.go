package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"math/rand/v2"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// lock-split (decisions/DECISIONS.md, "`lock-split`"): the additive writers (adopt, the index and spooled ingests,
// /ingest-batch, /refs, the multi-pack-index write) no longer exclude validates, so every validate must still answer as
// it would run alone. The test runs a script of writer steps on network 7 while validates and ls-refs run beside them:
// continuously, launched from inside each step at the points where safety rests on an order (between moveIn's renames,
// between /refs' transactions and its HEAD, after adopt's claim), and one per step held just after its ref snapshot
// until the step lands. Each answer must equal the serial answer, on network 8, at the refs of one state and the
// objects of the same or a later one, both between the last step done when it was asked and the last step started when
// it answered: objects only grow under a validate, and a catch-up lands its objects before its refs, so the shard
// passes through such states anyway. ls-refs must show one state's refs whole.

type vspec struct {
	pack []byte
	tips []string
	ups  []update
	disk bool
}

type refState struct {
	refs map[string]string
	head string
}

func validateReq(url, net string, v vspec) (int, string, error) {
	head := ""
	if len(v.tips) > 0 {
		head = strings.Join(v.tips, "\n") + "\n"
	}
	var d strings.Builder
	for _, u := range v.ups {
		fmt.Fprintf(&d, "%s %s %s\n", u.New, u.Old, u.Ref)
	}
	q := fmt.Sprintf("/validate?net=%s&tipsn=%d&detailn=%d", net, len(head), d.Len())
	if v.disk {
		q += "&disk=1"
	}
	r, err := http.Post(url+q, "", bytes.NewReader(append([]byte(head+d.String()), v.pack...)))
	if err != nil {
		return 0, "", err
	}
	defer r.Body.Close()
	b, err := io.ReadAll(r.Body)
	return r.StatusCode, string(b), err
}

// normalized is a validate's answer less what varies from run to run: its timings and a held request's id.
func normalized(code int, body string) string {
	switch {
	case code == 200:
		var v map[string]any
		if err := json.Unmarshal([]byte(body), &v); err != nil {
			return "200 " + body
		}
		delete(v, "lock_wait_ms")
		delete(v, "index_ms")
		delete(v, "walk_ms")
		b, _ := json.Marshal(v)
		return "200 " + string(b)
	case code == 409 && strings.Contains(body, `"missingBase":true`):
		return "409 missingBase"
	default:
		return fmt.Sprintf("%d %s", code, strings.SplitN(body, ":", 2)[0])
	}
}

// lsHeads is a v2 ls-refs of refs/heads/, its lines sorted.
func lsHeads(url, net string) (string, error) {
	pkt := func(l string) string { return fmt.Sprintf("%04x%s", len(l)+4, l) }
	req, _ := http.NewRequest("POST", url+"/upload-pack?net="+net,
		strings.NewReader(pkt("command=ls-refs\n")+"0001"+pkt("ref-prefix refs/heads/\n")+"0000"))
	req.Header.Set("git-protocol", "version=2")
	r, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", err
	}
	defer r.Body.Close()
	b, err := io.ReadAll(r.Body)
	if err != nil || r.StatusCode != 200 {
		return "", fmt.Errorf("ls-refs: %d %v %s", r.StatusCode, err, b)
	}
	var lines []string
	for len(b) >= 4 {
		var n int
		fmt.Sscanf(string(b[:4]), "%04x", &n)
		if n < 4 {
			b = b[4:]
			continue
		}
		lines = append(lines, strings.TrimSpace(string(b[4:n])))
		b = b[n:]
	}
	sort.Strings(lines)
	return strings.Join(lines, ","), nil
}

func (s refState) heads() string {
	var lines []string
	for ref, oid := range s.refs {
		if strings.HasPrefix(ref, "refs/heads/") {
			lines = append(lines, oid+" "+ref)
		}
	}
	sort.Strings(lines)
	return strings.Join(lines, ",")
}

func TestValidateSerialEquivalence(t *testing.T) {
	s := serve(t)
	zero40 := strings.Repeat("0", 40)
	post := func(path string, body []byte) (int, string, error) {
		r, err := http.Post(s.URL+path, "application/octet-stream", bytes.NewReader(body))
		if err != nil {
			return 0, "", err
		}
		defer r.Body.Close()
		b, err := io.ReadAll(r.Body)
		return r.StatusCode, string(b), err
	}
	want200 := func(what string, code int, body string, err error) error {
		if err != nil || code != 200 {
			return fmt.Errorf("%s: %d %v %.300s", what, code, err, body)
		}
		return nil
	}

	// main: B0, C1..C6, one pack per step; x1, x2, x3: pushes beside it, validated only.
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	cs := []string{commit(t, wt, map[string]string{"README": "r\n", "pkg/a.go": "a\n"})}
	for i := 1; i <= 6; i++ {
		cs = append(cs, commit(t, wt, map[string]string{fmt.Sprintf("pkg/c%d.go", i): strings.Repeat(fmt.Sprintf("c%d\n", i), 200)}))
	}
	side := func(base, name string) string {
		run(t, wt, "", "checkout", "-q", "-B", "side-"+name, base)
		tip := commit(t, wt, map[string]string{"side/" + name: strings.Repeat(name+"\n", 300)})
		run(t, wt, "", "checkout", "-q", "main")
		return tip
	}
	x1, x2, x3 := side(cs[3], "x1"), side(cs[1], "x2"), side(cs[5], "x3")
	p3 := selfPack(t, wt, cs[3])
	idx3 := idxFor(t, p3)
	packs := map[int][]byte{1: packOfWt(t, wt, cs[1], cs[0]), 2: packOfWt(t, wt, cs[2], cs[1]), 3: p3,
		4: packOfWt(t, wt, cs[4], cs[3]), 5: packOfWt(t, wt, cs[5], cs[4]), 6: packOfWt(t, wt, cs[6], cs[5])}

	vs := []vspec{
		{packOfWt(t, wt, x1, cs[3]), []string{x1}, []update{{x1, zero40, "refs/heads/x1"}}, false},
		{packOfWt(t, wt, x1, cs[3]), []string{x1}, []update{{x1, zero40, "refs/heads/x1"}}, true},
		{nil, nil, []update{{cs[4], zero40, "refs/heads/feature"}}, false},
		{nil, nil, []update{{cs[6], cs[3], "refs/heads/main"}}, false},
		{packOfWt(t, wt, x2, cs[1]), []string{x2}, []update{{x2, cs[1], "refs/heads/main"}}, true},
		{packOfWt(t, wt, x3, cs[5]), []string{x3}, nil, false},
	}

	// The test's hooks act on network 7 during the concurrent run only.
	var concurrent atomic.Bool
	var aging, gateHeld atomic.Bool
	var gate sync.RWMutex // held by the test while it sets up a stall or an aged aside
	ingest := func(net string, p []byte) error {
		code, body, err := post("/ingest?net="+net+"&sha256="+sumOf(p), p)
		return want200("ingest", code, body, err)
	}
	refsPost := func(net, head string, lines ...string) error {
		q := "/refs?net=" + net
		if head != "" {
			q += "&head=" + head
		}
		code, body, err := post(q, []byte(strings.Join(lines, "\n")+"\n"))
		return want200("refs", code, body, err)
	}
	adopt := func(net string, p []byte, tip string, aged bool) error {
		code, body, err := validateReq(s.URL, net, vspec{pack: p, tips: []string{tip}})
		if err := want200("adopt's validate", code, body, err); err != nil {
			return err
		}
		if aged && concurrent.Load() {
			// An aside at the sweep's hour, so a validate beside the adopt would sweep it (sweepValidated) unless
			// adopt claimed it first. No other validate runs until adopt is moving the pack in.
			gate.Lock()
			gateHeld.Store(true)
			aging.Store(true)
			old := time.Now().Add(-2 * time.Hour)
			if err := os.Chtimes(filepath.Join(repo(net), "graft", "validated-"+sumOf(p)), old, old); err != nil {
				return err
			}
		}
		code, body, err = post("/ingest?net="+net+"&sha256="+sumOf(p), p)
		if aging.Swap(false); gateHeld.CompareAndSwap(true, false) {
			gate.Unlock()
		}
		if err := want200("adopt", code, body, err); err != nil {
			return err
		}
		if !strings.Contains(body, `"adopted":true`) {
			return fmt.Errorf("adopt indexed the pack instead: %s", body)
		}
		return nil
	}

	r0 := refState{map[string]string{"refs/heads/main": cs[0], "refs/heads/a": cs[0]}, "refs/heads/main"}
	r2 := refState{map[string]string{"refs/heads/main": cs[1], "refs/heads/a": cs[0]}, "refs/heads/main"}
	r5 := refState{map[string]string{"refs/heads/main": cs[3], "refs/heads/a/b": cs[3]}, "refs/heads/main"}
	r7 := refState{map[string]string{"refs/heads/main": cs[3], "refs/heads/a/b": cs[3], "refs/heads/trunk": cs[4]},
		"refs/heads/trunk"}
	r10 := refState{map[string]string{"refs/heads/main": cs[6], "refs/heads/a/b": cs[3], "refs/heads/trunk": cs[4]},
		"refs/heads/trunk"}
	type step struct {
		name string
		refs *refState // the refs after it, for a step that writes refs
		do   func(net string) error
	}
	steps := []step{
		{"ingest C1 (index)", nil, func(n string) error { return ingest(n, packs[1]) }},
		{"refs main=C1", &r2, func(n string) error { return refsPost(n, "", cs[1]+" refs/heads/main") }},
		{"adopt C2 (aged aside)", nil, func(n string) error { return adopt(n, packs[2], cs[2], true) }},
		{"spooled C3 (stored idx)", nil, func(n string) error {
			sum := stage(t, s.URL, n, packs[3], idx3)
			code, a, body := ingestStaged(t, s.URL, n, sum)
			if code != 200 || a.Idx != "used" {
				return fmt.Errorf("spooled: %d %s", code, body)
			}
			return nil
		}},
		{"refs a -> a/b, main=C3 (D/F)", &r5, func(n string) error {
			return refsPost(n, "", zero40+" refs/heads/a", cs[3]+" refs/heads/a/b", cs[3]+" refs/heads/main")
		}},
		{"ingest C4 with a multi-pack-index", nil, func(n string) error {
			midxOf(repo(n)).n = midxEvery - 1
			if err := ingest(n, packs[4]); err != nil {
				return err
			}
			midxWait(repo(n))
			if _, err := os.Stat(filepath.Join(repo(n), "objects", "pack", "multi-pack-index")); err != nil {
				return fmt.Errorf("no multi-pack-index: %v", err)
			}
			return nil
		}},
		{"refs trunk=C4, HEAD trunk", &r7, func(n string) error {
			return refsPost(n, "refs/heads/trunk", cs[4]+" refs/heads/trunk")
		}},
		{"adopt C5", nil, func(n string) error { return adopt(n, packs[5], cs[5], false) }},
		{"ingest-batch C6", nil, func(n string) error {
			p := packs[6]
			code, body, err := post("/ingest-batch?net="+n, append([]byte(fmt.Sprintf("%s %d\n", sumOf(p), len(p))), p...))
			return want200("ingest-batch", code, body, err)
		}},
		{"refs main=C6", &r10, func(n string) error { return refsPost(n, "", cs[6]+" refs/heads/main") }},
	}
	k := len(steps)
	// States 0..k: state i is after step i. rid[i]: the refs of state i (as the last refs step before it); oid[i]: its
	// objects (as the last object step before it).
	rid, oid := make([]int, k+1), make([]int, k+1)
	states := map[int]refState{0: r0}
	for i := 1; i <= k; i++ {
		rid[i], oid[i] = rid[i-1], oid[i-1]
		if st := steps[i-1]; st.refs != nil {
			rid[i] = i
			states[i] = *st.refs
		} else {
			oid[i] = i
		}
	}
	putState := func(net string, st refState) error {
		var b strings.Builder
		for ref, o := range st.refs {
			fmt.Fprintf(&b, "%s %s\n", o, ref)
		}
		req, _ := http.NewRequest("PUT", s.URL+"/refs?net="+net+"&head="+st.head, strings.NewReader(b.String()))
		r, err := http.DefaultClient.Do(req)
		if err != nil {
			return err
		}
		defer r.Body.Close()
		body, _ := io.ReadAll(r.Body)
		if r.StatusCode != 200 || !strings.Contains(string(body), `"skipped":0`) {
			return fmt.Errorf("PUT /refs: %d %s", r.StatusCode, body)
		}
		return nil
	}
	for _, n := range []string{"7", "8"} {
		call(t, s, "POST", "/init?net="+n)
		if err := ingest(n, selfPack(t, wt, cs[0])); err != nil {
			t.Fatal(err)
		}
		if err := putState(n, r0); err != nil {
			t.Fatal(err)
		}
	}

	// The serial answers, on network 8: for each objects state b, every refs state a <= b.
	type pair struct{ r, o int }
	serial := map[pair][]string{}
	for b := 0; b <= k; b++ {
		if b > 0 && steps[b-1].refs == nil {
			if err := steps[b-1].do("8"); err != nil {
				t.Fatalf("serial %s: %v", steps[b-1].name, err)
			}
		}
		for a := 0; a <= b; a++ {
			p := pair{rid[a], oid[b]}
			if _, done := serial[p]; done {
				continue
			}
			if err := putState("8", states[rid[a]]); err != nil {
				t.Fatal(err)
			}
			var got []string
			for _, v := range vs {
				code, body, err := validateReq(s.URL, "8", v)
				if err != nil {
					t.Fatal(err)
				}
				got = append(got, normalized(code, body))
			}
			ls, err := lsHeads(s.URL, "8")
			if err != nil {
				t.Fatal(err)
			}
			if ls != states[rid[a]].heads() {
				t.Fatalf("serial ls-refs at state %d: %s, want %s", a, ls, states[rid[a]].heads())
			}
			serial[p] = append(got, ls)
		}
	}
	// The script must tell states apart, or the comparison below proves nothing.
	distinct := map[string]bool{}
	for _, got := range serial {
		for i, g := range got {
			distinct[fmt.Sprint(i, g)] = true
		}
	}
	if len(distinct) < 3*len(vs) {
		t.Fatalf("the serial answers tell only %d answers apart across %d states: %v", len(distinct), len(serial), serial)
	}

	// The concurrent run on network 7.
	type rec struct {
		v             int // len(vs) for ls-refs
		before, after int
		at, got       string
	}
	var (
		started, completed atomic.Int64
		mu                 sync.Mutex
		recs               []rec
		probes             sync.WaitGroup
		hooks              sync.Map
	)
	ask := func(v int, at string) {
		before := int(completed.Load())
		var got string
		if v == len(vs) {
			ls, err := lsHeads(s.URL, "7")
			got = ls
			if err != nil {
				got = err.Error()
			}
		} else {
			code, body, err := validateReq(s.URL, "7", vs[v])
			got = normalized(code, body)
			if err != nil {
				got = err.Error()
			}
		}
		mu.Lock()
		recs = append(recs, rec{v, before, int(started.Load()), at, got})
		mu.Unlock()
	}
	// probe asks every validate and an ls-refs from inside a step, and lets the step go on once they answered or wait
	// passed (a deleter's section holds them off).
	probe := func(at string, wait time.Duration) {
		var wg sync.WaitGroup
		for v := 0; v <= len(vs); v++ {
			wg.Add(1)
			probes.Add(1)
			go func() {
				defer probes.Done()
				defer wg.Done()
				ask(v, at)
			}()
		}
		done := make(chan struct{})
		go func() { wg.Wait(); close(done) }()
		select {
		case <-done:
		case <-time.After(wait):
		}
	}
	type stallT struct {
		reached chan struct{}
		until   int64
	}
	var stall atomic.Pointer[stallT]
	rp7 := repo("7")
	oldPause := pauseAt
	t.Cleanup(func() { pauseAt = oldPause })
	pauseAt = func(at, rp string) {
		if rp != rp7 || !concurrent.Load() {
			return
		}
		n, _ := hooks.LoadOrStore(at, new(atomic.Int64))
		n.(*atomic.Int64).Add(1)
		switch at {
		case "validate-snapshot":
			if st := stall.Swap(nil); st != nil {
				close(st.reached)
				for t0 := time.Now(); completed.Load() < st.until && time.Since(t0) < time.Second; {
					time.Sleep(5 * time.Millisecond)
				}
			}
		case "adopt-claimed":
			if !aging.Load() {
				probe(at, 400*time.Millisecond)
			}
		case "movein.pack":
			if aging.Load() {
				// The aged aside's adopt is moving its pack in: a validate now sweeps hour-old asides.
				probe(at+" (aged)", 2*time.Second)
				if gateHeld.CompareAndSwap(true, false) {
					gate.Unlock()
				}
				return
			}
			probe(at, 400*time.Millisecond)
		default:
			probe(at, 400*time.Millisecond)
		}
	}
	concurrent.Store(true)
	var finished atomic.Bool
	var bg sync.WaitGroup
	for g := range 3 {
		bg.Add(1)
		go func() {
			defer bg.Done()
			rng := rand.New(rand.NewPCG(uint64(g), 7))
			for !finished.Load() {
				gate.RLock()
				ask(rng.IntN(len(vs)+1), "beside")
				gate.RUnlock()
			}
		}()
	}
	stalled := 0
	for i, st := range steps {
		// One validate per step held just after its ref snapshot until the step lands: the detail-only validate whose
		// answer turns on HEAD and on the refs for the HEAD move, the others in turn.
		gate.Lock()
		probes.Wait()
		if i == 2 {
			gate.Unlock()
		} else {
			v := i % len(vs)
			if i == 6 {
				v = 2
			}
			s := &stallT{reached: make(chan struct{}), until: int64(i + 1)}
			stall.Store(s)
			probes.Add(1)
			go func() {
				defer probes.Done()
				ask(v, "stalled")
			}()
			select {
			case <-s.reached:
				stalled++
			case <-time.After(5 * time.Second):
				gate.Unlock()
				t.Fatalf("step %d: no validate reached its snapshot", i+1)
			}
			gate.Unlock()
		}
		started.Add(1)
		if err := st.do("7"); err != nil {
			finished.Store(true)
			t.Fatalf("step %d, %s: %v", i+1, st.name, err)
		}
		completed.Add(1)
	}
	finished.Store(true)
	bg.Wait()
	probes.Wait()
	concurrent.Store(false)

	for _, at := range []string{"movein.pack", "movein.rev", "movein.idx", "refs-between", "refs-head", "adopt-claimed",
		"validate-snapshot"} {
		if n, ok := hooks.Load(at); !ok || n.(*atomic.Int64).Load() == 0 {
			t.Fatalf("no step paused at %s: the test exercised none of its window", at)
		}
	}
	if stalled < k-1 {
		t.Fatalf("%d validates held across a step, want %d", stalled, k-1)
	}
	bad := 0
	for _, r := range recs {
		ok := false
		var allowed []string
		for a := r.before; a <= r.after && !ok; a++ {
			for b := a; b <= r.after && !ok; b++ {
				want := serial[pair{rid[a], oid[b]}][r.v]
				ok = want == r.got
				allowed = append(allowed, fmt.Sprintf("refs %d objects %d: %.200s", rid[a], oid[b], want))
			}
		}
		if !ok {
			if bad++; bad <= 5 {
				what := "ls-refs"
				if r.v < len(vs) {
					what = fmt.Sprintf("validate %d", r.v)
				}
				t.Errorf("%s asked %s between steps %d and %d answered\n  %.300s\nwhich no serial run gives:\n  %s", what,
					r.at, r.before, r.after, r.got, strings.Join(allowed, "\n  "))
			}
		}
	}
	if bad > 0 {
		t.Fatalf("%d of %d answers match no serial state", bad, len(recs))
	}
	if len(recs) < 10*k {
		t.Fatalf("only %d answers recorded", len(recs))
	}

	// The end state: the last refs, every commit, a connected repository, and no claimed or discarded aside left.
	ls, err := lsHeads(s.URL, "7")
	if err != nil || ls != r10.heads() {
		t.Fatalf("refs at the end: %s %v, want %s", ls, err, r10.heads())
	}
	for _, c := range cs {
		run(t, rp7, "", "cat-file", "-e", c)
	}
	run(t, rp7, "", "fsck", "--connectivity-only", "--no-progress")
	if left, _ := filepath.Glob(filepath.Join(rp7, "graft", "adopting-*")); len(left) > 0 {
		t.Fatalf("claims left: %v", left)
	}
	t.Logf("%d answers across %d steps match the serial run", len(recs), k)
}
