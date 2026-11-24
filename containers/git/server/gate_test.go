package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"runtime"
	"strconv"
	"strings"
	"testing"
	"time"
)

type gateResult struct {
	release    func()
	retryAfter int
	err        error
}

func acquireAsync(g *validateGate, ctx context.Context, agent bool, wait time.Duration) <-chan gateResult {
	result := make(chan gateResult, 1)
	go func() {
		release, retryAfter, err := g.acquire(ctx, agent, wait)
		result <- gateResult{release, retryAfter, err}
	}()
	return result
}

func waitGateQueue(t *testing.T, g *validateGate, agent bool, n int) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		g.mu.Lock()
		count := len(g.humans)
		if agent {
			count = len(g.agents)
		}
		g.mu.Unlock()
		if count == n {
			return
		}
		runtime.Gosched()
	}
	t.Fatalf("queue did not reach %d waiters (agent=%v)", n, agent)
}

func gateRunning(g *validateGate) int {
	g.mu.Lock()
	defer g.mu.Unlock()
	return g.running
}

func receiveGateResult(t *testing.T, result <-chan gateResult) gateResult {
	t.Helper()
	select {
	case got := <-result:
		return got
	case <-time.After(2 * time.Second):
		t.Fatal("acquire did not return")
		return gateResult{}
	}
}

func TestGateHumanWaiterPrecedesEarlierAgent(t *testing.T) {
	g := newValidateGate(1)
	held, _, err := g.acquire(context.Background(), false, 0)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	agent := acquireAsync(g, ctx, true, time.Second)
	waitGateQueue(t, g, true, 1)
	human := acquireAsync(g, ctx, false, 0)
	waitGateQueue(t, g, false, 1)

	held()
	select {
	case got := <-agent:
		if got.release != nil {
			got.release()
		}
		t.Fatal("earlier agent was granted before the human")
	case got := <-human:
		if got.err != nil {
			t.Fatalf("human acquire: %v", got.err)
		}
		got.release()
	case <-time.After(2 * time.Second):
		t.Fatal("human was not granted the released slot")
	}
	got := receiveGateResult(t, agent)
	if got.err != nil {
		t.Fatalf("agent acquire after human: %v", got.err)
	}
	got.release()
	if running := gateRunning(g); running != 0 {
		t.Fatalf("running = %d after both releases, want 0", running)
	}
}

func TestGateAgentDeadlineRefusesWithoutLeakingSlot(t *testing.T) {
	g := newValidateGate(1)
	held, _, err := g.acquire(context.Background(), false, 0)
	if err != nil {
		t.Fatal(err)
	}
	started := time.Now()
	release, retryAfter, err := g.acquire(context.Background(), true, 50*time.Millisecond)
	elapsed := time.Since(started)
	if !errors.Is(err, errValidateQueueFull) || release != nil {
		t.Fatalf("acquire = (release %t, %v), want queue-full refusal", release != nil, err)
	}
	if elapsed < 45*time.Millisecond || elapsed > time.Second {
		t.Fatalf("agent refusal took %v, want about 50ms", elapsed)
	}
	if retryAfter < 1 || retryAfter > 60 {
		t.Fatalf("Retry-After = %d, want [1, 60]", retryAfter)
	}
	if running := gateRunning(g); running != 1 {
		t.Fatalf("running = %d after refusal, want the one held slot", running)
	}
	held()

	started = time.Now()
	permits := make([]func(), 0, g.slots)
	for i := 0; i < g.slots; i++ {
		release, _, err = g.acquire(context.Background(), true, 0)
		if err != nil {
			t.Fatalf("acquire %d of %d after refusal leaked a slot: %v", i+1, g.slots, err)
		}
		permits = append(permits, release)
	}
	if elapsed := time.Since(started); elapsed > 100*time.Millisecond {
		t.Fatalf("available acquire took %v", elapsed)
	}
	if release, _, err = g.acquire(context.Background(), true, 0); !errors.Is(err, errValidateQueueFull) || release != nil {
		t.Fatalf("slots+1 acquire = (release %t, %v), want refusal", release != nil, err)
	}
	for _, release := range permits {
		release()
	}
	if running := gateRunning(g); running != 0 {
		t.Fatalf("running = %d at test end, want 0", running)
	}
}

// Exercise both orders of the deadline/grant race, not just a deadline while the holder stays put.
func TestGateDeadlineGrantRaceDoesNotLeakSlot(t *testing.T) {
	for i := 0; i < 300; i++ {
		g := newValidateGate(1)
		held, _, err := g.acquire(context.Background(), false, 0)
		if err != nil {
			t.Fatal(err)
		}
		const wait = time.Millisecond
		result := acquireAsync(g, context.Background(), true, wait)
		waitGateQueue(t, g, true, 1)
		time.Sleep(wait)
		held()
		got := receiveGateResult(t, result)
		if !errors.Is(got.err, errValidateQueueFull) && got.err != nil {
			t.Fatalf("round %d: unexpected error: %v", i, got.err)
		}
		if got.release != nil {
			got.release()
		}
		if running := gateRunning(g); running != 0 {
			t.Fatalf("round %d: deadline/grant race left running = %d", i, running)
		}
		release, _, err := g.acquire(context.Background(), true, 0)
		if err != nil {
			t.Fatalf("round %d: slot unusable after deadline/grant race: %v", i, err)
		}
		release()
	}
}

func TestGateAgentsBeyondQueueLimitRefusedAtOnce(t *testing.T) {
	g := newValidateGate(1)
	g.average = 0.001 // validates that run 1 ms: the count limit binds, not the drain time (`slot-bound`)
	held, _, err := g.acquire(context.Background(), false, 0)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	waiting := make([]<-chan gateResult, 0, agentQueuePerSlot*g.slots)
	for i := 0; i < agentQueuePerSlot*g.slots; i++ {
		waiting = append(waiting, acquireAsync(g, ctx, true, 500*time.Millisecond))
		waitGateQueue(t, g, true, i+1)
	}
	started := time.Now()
	release, retryAfter, err := g.acquire(context.Background(), true, 500*time.Millisecond)
	elapsed := time.Since(started)
	if !errors.Is(err, errValidateQueueFull) || release != nil {
		t.Fatalf("overflow acquire = (release %t, %v), want queue-full refusal", release != nil, err)
	}
	if retryAfter < 1 || retryAfter > 60 {
		t.Fatalf("Retry-After = %d, want [1, 60]", retryAfter)
	}
	if elapsed > 100*time.Millisecond {
		t.Fatalf("overflow refusal took %v, want well under 500ms deadline", elapsed)
	}
	cancel()
	for _, result := range waiting {
		got := receiveGateResult(t, result)
		if got.release != nil {
			got.release()
		}
	}
	held()
	release, _, err = g.acquire(context.Background(), true, 0)
	if err != nil {
		t.Fatalf("slot leaked after queued agents left: %v", err)
	}
	release()
}

func TestGateCancelledHumanLeavesQueueWithoutSlot(t *testing.T) {
	g := newValidateGate(1)
	held, _, err := g.acquire(context.Background(), false, 0)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	result := acquireAsync(g, ctx, false, 0)
	waitGateQueue(t, g, false, 1)
	cancel()
	got := receiveGateResult(t, result)
	if !errors.Is(got.err, context.Canceled) || got.release != nil {
		t.Fatalf("cancelled human = (release %t, %v), want context cancellation without a permit", got.release != nil, got.err)
	}
	if running := gateRunning(g); running != 1 {
		t.Fatalf("cancelled human changed running to %d, want 1", running)
	}
	held()
	release, _, err := g.acquire(context.Background(), true, 0)
	if err != nil {
		t.Fatalf("cancelled human consumed or retained a slot: %v", err)
	}
	release()
}

type observedBody struct{ read bool }

func (b *observedBody) Read([]byte) (int, error) {
	b.read = true
	return 0, io.EOF
}

func (b *observedBody) Close() error { return nil }

func TestGateHTTPAgentRefusalDoesNotReadBody(t *testing.T) {
	oldData, oldGate, oldWait := data, validateAdmission, agentValidateWait
	defer func() {
		data, validateAdmission, agentValidateWait = oldData, oldGate, oldWait
	}()
	data = t.TempDir()
	if err := os.MkdirAll(repo("55"), 0o755); err != nil {
		t.Fatal(err)
	}
	validateAdmission = newValidateGate(1)
	held, _, err := validateAdmission.acquire(context.Background(), false, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer held()
	agentValidateWait = 20 * time.Millisecond
	body := &observedBody{}
	r := httptest.NewRequest(http.MethodPost, "/validate?net=55&class=agent", body)
	w := httptest.NewRecorder()
	handleValidate(w, r)
	if w.Code != http.StatusTooManyRequests {
		t.Fatalf("status = %d, want 429; body %q", w.Code, w.Body.String())
	}
	retryAfter, err := strconv.Atoi(w.Header().Get("Retry-After"))
	if err != nil || retryAfter < 1 {
		t.Fatalf("Retry-After = %q, want whole seconds >= 1", w.Header().Get("Retry-After"))
	}
	if !strings.Contains(w.Body.String(), fmt.Sprintf("validate queue full; retry after %d s", retryAfter)) {
		t.Fatalf("body = %q", w.Body.String())
	}
	if body.read {
		t.Fatal("refused validate read its request body")
	}
}

// `slot-bound`, B0-33 h1A: a full pool shard held 1,306 agent validates its whole 2 s and then refused them. An agent
// is now refused at once when the agents queued ahead of it would not drain within its wait at the gate's measured run
// time, and still queues when they would.
func TestGateAgentRefusedAtOnceWhenTheQueueWillNotDrainInItsWait(t *testing.T) {
	g := newValidateGate(1)
	g.average = 0.5 // a validate runs 0.5 s: the slot frees twice a second
	held, _, err := g.acquire(context.Background(), false, 0)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	// Three agents queue: 0, 0.5 and 1 s of queue ahead of each, all inside a 5 s wait.
	var waiting []<-chan gateResult
	for i := 0; i < 3; i++ {
		waiting = append(waiting, acquireAsync(g, ctx, true, 5*time.Second))
		waitGateQueue(t, g, true, i+1)
	}
	// A fourth with 1.2 s to wait has 1.5 s of queue ahead: refused at once, not after 1.2 s.
	started := time.Now()
	release, retryAfter, err := g.acquire(context.Background(), true, 1200*time.Millisecond)
	if elapsed := time.Since(started); !errors.Is(err, errValidateQueueFull) || release != nil || elapsed > 300*time.Millisecond {
		t.Fatalf("acquire = (release %t, %v) after %v, want a refusal at once", release != nil, err, elapsed)
	}
	if retryAfter < 1 || retryAfter > 60 {
		t.Fatalf("Retry-After = %d, want [1, 60]", retryAfter)
	}
	// One with 2 s to wait, 1.5 s ahead, still queues: the slot frees within its wait.
	queued := acquireAsync(g, ctx, true, 2*time.Second)
	waitGateQueue(t, g, true, 4)
	select {
	case got := <-queued:
		t.Fatalf("an agent whose queue drains within its wait was answered at once: %v", got.err)
	case <-time.After(200 * time.Millisecond):
	}
	cancel()
	for _, r := range append(waiting, queued) {
		if got := receiveGateResult(t, r); got.release != nil {
			got.release()
		}
	}
	held()
	if running := gateRunning(g); running != 0 {
		t.Fatalf("running = %d at test end, want 0", running)
	}
}

// The edge's slot_ms (what the push's bound leaves) caps an agent's slot wait, never past agentValidateWait.
func TestGateHTTPSlotMsCapsTheAgentWait(t *testing.T) {
	if agentWait("") != agentValidateWait || agentWait("x") != agentValidateWait || agentWait("-5") != agentValidateWait {
		t.Fatal("a missing or bad slot_ms is not agentValidateWait")
	}
	if agentWait("0") != 0 || agentWait("1433") != 1433*time.Millisecond || agentWait("99999") != agentValidateWait {
		t.Fatalf("agentWait: 0 -> %v, 1433 -> %v, 99999 -> %v", agentWait("0"), agentWait("1433"), agentWait("99999"))
	}
	oldData, oldGate, oldWait := data, validateAdmission, agentValidateWait
	defer func() {
		data, validateAdmission, agentValidateWait = oldData, oldGate, oldWait
	}()
	data = t.TempDir()
	if err := os.MkdirAll(repo("56"), 0o755); err != nil {
		t.Fatal(err)
	}
	validateAdmission = newValidateGate(1)
	held, _, err := validateAdmission.acquire(context.Background(), false, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer held()
	agentValidateWait = 5 * time.Second
	for _, c := range []struct {
		slot     string
		min, max time.Duration
	}{{"&slot_ms=40", 30 * time.Millisecond, time.Second}, {"&slot_ms=0", 0, 300 * time.Millisecond}} {
		started := time.Now()
		w := httptest.NewRecorder()
		handleValidate(w, httptest.NewRequest(http.MethodPost, "/validate?net=56&class=agent"+c.slot, &observedBody{}))
		if elapsed := time.Since(started); w.Code != http.StatusTooManyRequests || elapsed < c.min || elapsed > c.max {
			t.Fatalf("%s: %d after %v, want 429 within [%v, %v]", c.slot, w.Code, elapsed, c.min, c.max)
		}
	}
}
