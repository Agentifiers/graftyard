package main

// Maintenance yields to moved load (decisions/DECISIONS.md, "`maint-yield`"; design/DESIGN.md, Shape, git cache
// shards). Maintenance is the geometric repack and the commit-graph write after it (startRepack) and a roll-up's build
// (handleRoll). The shard object posts /maint/hold when a client's first try reaches this shard on a site other than
// the record's home: the home's shard is down, cold or failing, and this one carries its work.

import (
	"context"
	"io"
	"net/http"
	"os"
	"os/exec"
	"sort"
	"sync"
	"syscall"
	"time"
)

// yieldHold: how long one hold call holds maintenance (YIELD_HOLD, 60 s [guess]: two of B0-34's kill-phase tip
// moves). Each moved call renews it, so a failover holds for as long as it lasts. A var for the tests.
var yieldHold = 60 * time.Second

// maintDeferMax: how long maintenance may be held in all, from the first hold since the last completed maintenance
// (MAINT_DEFER_MAX, 6 min [guess]: a kill's 120 s and a rehydrate's 207 s with about 30 s of room). 10 min let B0-34
// mt19's site b reach 786 unkept packs while held, past the 751 that slowed merges, so the flip shortened it
// (`decisions/DECISIONS.md`, "`maint-yield`"). Past it moved load no longer stops the next run, and client CPU delays
// its start at most another yieldHold. A var for the tests.
var maintDeferMax = 6 * time.Minute

// maintClock is the hold's clock; the tests step it rather than sleep, so a loaded runner cannot let a hold lapse
// between two calls the test meant to fall inside it.
var maintClock = time.Now

// fanoutUp: FANOUT_UP, the client CPU p99 past which `fanout` adds shards (crates/git, shard.rs).
const fanoutUp = 0.75

var maint struct {
	sync.Mutex
	until     time.Time          // the hold ends; zero or past when not held
	firstHeld time.Time          // the first refused start or stopped run since the last completed repack
	cancel    context.CancelFunc // the running maintenance's stop; nil when none runs
	overdue   bool               // the running maintenance started past maintDeferMax: holds do not stop it
	// For the row (`/procs`'s "maint"): the longest continuous hold, the most unkept packs seen while held, and the
	// runs a hold stopped.
	heldFrom  time.Time
	longest   time.Duration
	maxUnkept int
	stops     int
	// waiting: the first start refused since the last one granted, zero when none waits (`deferred_s`); maxDeferred:
	// the longest deferral a granted start had waited, from firstHeld, the clock maintDeferMax bounds.
	waiting     time.Time
	maxDeferred time.Duration
}

// cpuP99 is the last minute's client CPU p99 (`/load`'s cpu_p99), at most 1.
func cpuP99() float64 {
	cpuSamples.Lock()
	s := append([]float64{}, cpuSamples.last...)
	cpuSamples.Unlock()
	if len(s) == 0 {
		return 0
	}
	sort.Float64s(s)
	return min(s[min(len(s)-1, len(s)*99/100)], 1)
}

// maintBegin starts a maintenance run if nothing holds it: not a hold younger than yieldHold unless maintenance has
// been deferred maintDeferMax already, and not client CPU above fanoutUp unless it has been deferred maintDeferMax
// plus yieldHold. Deferred counts from the first refused start or stopped run since the last completed repack. The
// caller holds the repack slot, so one run at a time is registered. ok false: start none now. end unregisters the run;
// reset says a repack ran to its end, which starts the deferral clock over (a roll's build does not: the repack the
// bound protects would otherwise wait past it behind rolls).
func maintBegin() (ctx context.Context, end func(reset bool), ok bool) {
	maint.Lock()
	defer maint.Unlock()
	now := maintClock()
	deferred := time.Duration(0)
	if !maint.firstHeld.IsZero() {
		deferred = now.Sub(maint.firstHeld)
	}
	held := now.Before(maint.until) && deferred < maintDeferMax
	busy := cpuP99() > fanoutUp && deferred < maintDeferMax+yieldHold
	if held || busy {
		if maint.firstHeld.IsZero() {
			maint.firstHeld = now
		}
		if maint.waiting.IsZero() {
			maint.waiting = now
		}
		return nil, nil, false
	}
	maint.waiting, maint.maxDeferred = time.Time{}, max(maint.maxDeferred, deferred)
	ctx, cancel := context.WithCancel(context.Background())
	maint.cancel, maint.overdue = cancel, deferred >= maintDeferMax
	return ctx, func(reset bool) {
		maint.Lock()
		maint.cancel, maint.overdue = nil, false
		if reset {
			maint.firstHeld = time.Time{}
		}
		maint.Unlock()
		cancel()
	}, true
}

// handleMaintHold is POST /maint/hold?net=N: holds maintenance yieldHold from now and stops a running one, unless it
// started overdue. {"stopped": whether a run was stopped}. A hold that stops nothing starts no deferral clock: a
// failover with no maintenance due defers nothing, and the clock starts at the first start it refuses.
func handleMaintHold(w http.ResponseWriter, r *http.Request) {
	net := r.URL.Query().Get("net")
	unkept := -1
	if netRe.MatchString(net) {
		if _, err := os.Stat(repo(net)); err == nil {
			unkept = unkeptPacks(repo(net))
		}
	}
	maint.Lock()
	defer maint.Unlock()
	now := maintClock()
	if !now.Before(maint.until) {
		maint.heldFrom = now
	}
	maint.until = now.Add(yieldHold)
	maint.longest = max(maint.longest, maint.until.Sub(maint.heldFrom))
	maint.maxUnkept = max(maint.maxUnkept, unkept)
	stopped := false
	if maint.cancel != nil && !maint.overdue {
		maint.cancel()
		maint.stops++
		stopped = true
		if maint.firstHeld.IsZero() {
			maint.firstHeld = now
		}
	}
	jsonOut(w, map[string]any{"stopped": stopped})
}

// maintStats is `/procs`'s "maint", for the row: whether maintenance is held or running now, the longest continuous
// hold (to its end, so with its last yieldHold), the most unkept packs any hold call saw, the runs holds stopped, how
// long a start has waited now (`deferred_s`: since the first start refused after the last one granted, 0 when none
// waits), the longest deferral a granted start had waited (`max_deferred_s`, from the first refused start or stopped
// run since the last completed repack: the clock maintDeferMax bounds), and the client CPU p99 maintBegin reads. A
// hold's span is not its deferral: B0-34 mt21's 573 s hold on site b had its run start inside the bound (G41). Counted
// from firstHeld to now, `deferred_s` ran through mt22's overdue repack and the idle after load to 1,485 s while the
// bound held at about 420.
func maintStats() map[string]any {
	cpu := cpuP99()
	maint.Lock()
	defer maint.Unlock()
	now, waited := maintClock(), 0.0
	if !maint.waiting.IsZero() {
		waited = now.Sub(maint.waiting).Seconds()
	}
	return map[string]any{"held": now.Before(maint.until), "running": maint.cancel != nil,
		"longest_hold_s": maint.longest.Seconds(), "max_unkept_held": maint.maxUnkept, "stops": maint.stops,
		"deferred_s": waited, "max_deferred_s": maint.maxDeferred.Seconds(), "cpu_p99": cpu}
}

// gitMaint runs one maintenance git at nice 19 in its own process group, which ctx's end stops with SIGTERM: the
// repack's pack-objects and the roll's build are children, and git writes each new pack, index, midx and commit-graph
// under a temporary name and renames it in whole, so a stop at any point leaves the repository whole
// (`tests/maintyield/stop.sh`, 100 of 100).
func gitMaint(ctx context.Context, dir string, env []string, stdin io.Reader, args ...string) ([]byte, error) {
	cmd := exec.CommandContext(ctx, "nice", append([]string{"-n", "19", "git"}, args...)...)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	cmd.Cancel = func() error { return syscall.Kill(-cmd.Process.Pid, syscall.SIGTERM) }
	cmd.WaitDelay = 10 * time.Second
	return runGit(cmd, nil, dir, env, stdin, args...)
}
