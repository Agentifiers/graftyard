package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

// resetMaint gives a test its own maintenance hold state and constants, and gives the next test a clean one. The
// hold's clock stands still until the test steps it (the returned func), so no sleep decides whether a hold lapsed.
func resetMaint(t *testing.T, hold, deferMax time.Duration) (step func(time.Duration)) {
	t.Helper()
	oldHold, oldDefer, oldClock := yieldHold, maintDeferMax, maintClock
	var mu sync.Mutex
	at := time.Now()
	maintClock = func() time.Time {
		mu.Lock()
		defer mu.Unlock()
		return at
	}
	step = func(d time.Duration) {
		mu.Lock()
		at = at.Add(d)
		mu.Unlock()
	}
	clear := func() {
		maint.Lock()
		maint.until, maint.firstHeld, maint.cancel, maint.overdue = time.Time{}, time.Time{}, nil, false
		maint.heldFrom, maint.longest, maint.maxUnkept, maint.stops = time.Time{}, 0, 0, 0
		maint.Unlock()
		cpuSamples.Lock()
		cpuSamples.last = nil
		cpuSamples.Unlock()
	}
	clear()
	yieldHold, maintDeferMax = hold, deferMax
	t.Cleanup(func() {
		yieldHold, maintDeferMax, maintClock = oldHold, oldDefer, oldClock
		clear()
	})
	return step
}

// sleeping waits until the planted repack's own `sleep secs` child runs, so a stop is tested on git with a child
// below it: git's command line names the alias too, and a stop sent before git forks reaches no child.
func sleeping(t *testing.T, secs string) {
	t.Helper()
	for deadline := time.Now().Add(20 * time.Second); time.Now().Before(deadline); time.Sleep(10 * time.Millisecond) {
		ents, _ := os.ReadDir("/proc")
		for _, e := range ents {
			b, err := os.ReadFile(filepath.Join("/proc", e.Name(), "cmdline"))
			if args := strings.Split(string(b), "\x00"); err == nil && len(args) > 1 &&
				filepath.Base(args[0]) == "sleep" && args[1] == secs {
				return
			}
		}
	}
	t.Fatalf("the planted repack's sleep %s never ran", secs)
}

func holdMaint(t *testing.T, url string) bool {
	t.Helper()
	r, err := http.Post(url+"/maint/hold?net=7", "", nil)
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	var v struct{ Stopped bool }
	if err := json.NewDecoder(r.Body).Decode(&v); err != nil || r.StatusCode != 200 {
		t.Fatalf("/maint/hold: %d %v", r.StatusCode, err)
	}
	return v.Stopped
}

// refsRepack sends a catch-up's last PUT /refs and says whether it started a repack.
func refsRepack(t *testing.T, url, tip string) bool {
	t.Helper()
	req, _ := http.NewRequest("PUT", url+"/refs?net=7", strings.NewReader(tip+" refs/heads/main\n"))
	r, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	var v struct{ Repack bool }
	if err := json.NewDecoder(r.Body).Decode(&v); err != nil || r.StatusCode != 200 {
		t.Fatalf("PUT /refs: %d %v", r.StatusCode, err)
	}
	return v.Repack
}

// A repack planted during a kill (`maint-yield`): a slow geometric repack runs on the surviving site's shard when the
// lost site's moved load arrives. The hold stops it within a second, with the repack slot and the midx lock free and
// the repository whole; no repack starts until the hold ends, nor while client CPU is above FANOUT_UP; and once
// maintenance has been deferred maintDeferMax, the next repack runs to its end through moved load. On B0-34 mt18 a
// repack at nice 19 ran through the kill, held the midx lock about 330 s, and the shard's clone misses slowed to a
// median 3.3 to 12.6 s a minute (tests/am1/results/b034-mt18.md).
func TestMaintenanceYieldsToMovedLoad(t *testing.T) {
	s := serve(t)
	step := resetMaint(t, 1500*time.Millisecond, time.Hour)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	prev := commit(t, wt, map[string]string{"README": "r\n"})
	run(t, wt, "", "push", "-q", repo("7"), "main")
	for i := 0; i <= repackAt; i++ {
		tip := commit(t, wt, map[string]string{"f": fmt.Sprintf("%d\n", i)})
		idx := exec.Command("git", "index-pack", "--stdin", "--fix-thin")
		idx.Dir = repo("7")
		idx.Stdin = bytes.NewReader(packOfWt(t, wt, tip, prev))
		if out, err := idx.CombinedOutput(); err != nil {
			t.Fatalf("index-pack: %v %s", err, out)
		}
		prev = tip
	}
	fsckOK := func(when string) {
		t.Helper()
		if _, v := call(t, s, "POST", "/fsck?net=7"); v["ok"] != true {
			t.Fatalf("fsck %s: %v %v", when, v["out"], v["err"])
		}
	}
	// The planted repack: a real geometric repack after `delay` seconds, in one git whose alias runs the sleep and
	// the repack as its children, so only a stop that reaches the whole process group ends it at once.
	delay := 30
	ended := make(chan error, 8)
	old := runRepack
	runRepack = func(ctx context.Context, rp string) error {
		alias := fmt.Sprintf("alias.slowrepack=!sleep %d.%d && git repack -d -q --geometric=2 --write-midx", delay, 1)
		_, err := gitMaint(ctx, rp, []string{derivedEnv}, nil, "-c", alias, "slowrepack")
		ended <- err
		return err
	}
	t.Cleanup(func() { runRepack = old })
	running := func() bool {
		if repacking.TryLock() {
			repacking.Unlock()
			return false
		}
		return true
	}
	// Frees: the repack slot and the network's midx lock, both within 1 s of the hold.
	frees := func(when string) {
		t.Helper()
		select {
		case <-ended:
		case <-time.After(time.Second):
			t.Fatalf("%s: the repack still runs 1 s after moved load arrived", when)
		}
		deadline := time.Now().Add(time.Second)
		for running() || !midxOf(repo("7")).mu.TryLock() {
			if time.Now().After(deadline) {
				t.Fatalf("%s: the repack slot or the midx lock is still held 1 s after the stop", when)
			}
			time.Sleep(5 * time.Millisecond)
		}
		midxOf(repo("7")).mu.Unlock()
	}

	// 1. The kill: moved load arrives while the repack runs, and stops it.
	if !refsRepack(t, s.URL, prev) {
		t.Fatal("no repack started past repackAt")
	}
	sleeping(t, "30.1")
	if !holdMaint(t, s.URL) {
		t.Fatal("/maint/hold did not stop the running repack")
	}
	frees("the first hold")
	fsckOK("after the stopped repack")

	// 2. Held: no repack starts. Each call renews the hold, so one renewed past the first hold's end still holds.
	if refsRepack(t, s.URL, prev) {
		t.Fatal("a repack started while moved load held maintenance")
	}
	step(time.Second)
	holdMaint(t, s.URL)
	step(time.Second)
	if refsRepack(t, s.URL, prev) {
		t.Fatal("a repack started inside a renewed hold, past the first hold's end")
	}
	step(yieldHold)
	// Client CPU above FANOUT_UP starts none either.
	cpuSamples.Lock()
	cpuSamples.last = []float64{0.9, 0.9, 0.9}
	cpuSamples.Unlock()
	if refsRepack(t, s.URL, prev) {
		t.Fatal("a repack started with client CPU p99 above FANOUT_UP")
	}
	cpuSamples.Lock()
	cpuSamples.last = []float64{0.2, 0.2, 0.2}
	cpuSamples.Unlock()
	// The hold over and CPU low, the next /refs starts it again, and moved load stops it again.
	if !refsRepack(t, s.URL, prev) {
		t.Fatal("no repack once the hold ended")
	}
	sleeping(t, "30.1")
	if !holdMaint(t, s.URL) {
		t.Fatal("the second hold did not stop the repack")
	}
	frees("the second hold")
	fsckOK("after the second stop")

	// 3. The deferral bound: the first stop was 3.5 s ago on the hold's clock, past a 3 s maintDeferMax. The next
	// repack starts through a live hold and moved load does not stop it, so a long outage still gets its repack.
	// Client CPU above FANOUT_UP delays it at most another yieldHold: at 3.5 s it still does, at 5 s no longer.
	maintDeferMax = 3 * time.Second
	delay = 0
	holdMaint(t, s.URL)
	cpuSamples.Lock()
	cpuSamples.last = []float64{0.9, 0.9, 0.9}
	cpuSamples.Unlock()
	if refsRepack(t, s.URL, prev) {
		t.Fatal("client CPU did not delay the overdue repack inside maintDeferMax plus one yieldHold")
	}
	step(yieldHold)
	holdMaint(t, s.URL) // a live hold, which no longer holds a start past maintDeferMax
	cpuSamples.Lock()
	cpuSamples.last = []float64{0.9, 0.9, 0.9}
	cpuSamples.Unlock()
	if !refsRepack(t, s.URL, prev) {
		t.Fatal("no repack once maintenance had been deferred maintDeferMax")
	}
	if holdMaint(t, s.URL) {
		t.Fatal("moved load stopped the repack that ran past maintDeferMax")
	}
	select {
	case err := <-ended:
		if err != nil {
			t.Fatalf("the overdue repack: %v", err)
		}
	case <-time.After(30 * time.Second):
		t.Fatal("the overdue repack never ended")
	}
	repacking.Lock() // its commit-graph too
	repacking.Unlock()
	fsckOK("after the overdue repack")
	if n := unkeptPacks(repo("7")); n > 8 {
		t.Fatalf("%d unkept packs after the overdue repack: it did not roll the tail", n)
	}
	st := maintStats()
	// The longest hold is the first, renewed 1 s in: 1 + 1.5 s on the hold's clock.
	if st["stops"] != 2 || st["max_unkept_held"].(int) <= repackAt || st["longest_hold_s"].(float64) != 2.5 {
		t.Fatalf("/procs maint: %v", st)
	}
}

// MAINT_DEFER_MAX covers a failover and keeps a held shard under the pack count that slowed merges. It must last
// B0-34's kill phase (120 s) and a rehydrated home's warm-up (207 s at most, DESIGN's git caches data-center row), so
// the home takes its load back before a repack runs. And at mt19's rate, site b's unkept packs going from 25 to 786
// while held from +67 s to +547 s (1.59 a second, tests/am1/results/b034-mt19.md), it must end before a hold that
// starts at 25 reaches 751 (`decisions/DECISIONS.md`, "`maint-yield`", the flip). 10 minutes failed the second.
func TestMaintDeferMaxFitsAFailoverAndThePackLine(t *testing.T) {
	const kill, rehydrate = 120 * time.Second, 207 * time.Second
	if maintDeferMax < kill+rehydrate {
		t.Fatalf("maintDeferMax %v is shorter than a kill and a rehydrate, %v", maintDeferMax, kill+rehydrate)
	}
	const start, perS, line = 25.0, (786.0 - 25.0) / (547.0 - 67.0), 751.0
	if at := start + perS*maintDeferMax.Seconds(); at >= line {
		t.Fatalf("maintDeferMax %v lets a held shard reach %.0f unkept packs at mt19's rate, past %v", maintDeferMax, at, line)
	}
}

// A held roll-up answers 503 `maint-held`, so the archive's roll tries the next site's shard (`promote_moves_on`),
// and once the hold ends the same roll answers its pack.
func TestAHeldRollAnswers503(t *testing.T) {
	s := serve(t)
	step := resetMaint(t, 500*time.Millisecond, time.Hour)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commit(t, wt, map[string]string{"f": "base\n"})
	first := ingestPack(t, s.URL, "7", selfPack(t, wt, base))
	tip := commit(t, wt, map[string]string{"f": "tip\n", "g": "new\n"})
	second := ingestPack(t, s.URL, "7", selfPack(t, wt, tip))
	roll := func() (int, string) {
		r, err := http.Post(s.URL+"/roll?net=7", "", strings.NewReader(first+"\n"+second+"\n"))
		if err != nil {
			t.Fatal(err)
		}
		defer r.Body.Close()
		b, _ := io.ReadAll(r.Body)
		return r.StatusCode, string(b)
	}
	holdMaint(t, s.URL)
	if code, body := roll(); code != 503 || !strings.Contains(body, "maint-held") {
		t.Fatalf("a roll while held: %d %.100s", code, body)
	}
	step(500 * time.Millisecond)
	if code, body := roll(); code != 200 {
		t.Fatalf("a roll after the hold: %d %.100s", code, body)
	}
}
