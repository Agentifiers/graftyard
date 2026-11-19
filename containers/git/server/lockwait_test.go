package main

import (
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"testing"
	"time"
)

// Catch-up writers and validates report how long they waited for the network's locks, so a hosted run can tell a
// catch-up's writer waiting behind another writer or a deleter from validates waiting behind a deleter. The test holds
// a lock itself for hold: the writer lock against the writers and the lock alone against a validate, and each answer's
// wait must cover most of it; the same calls uncontended must report a small wait. With lock-split
// (decisions/DECISIONS.md, "`lock-split`") a writer no longer waits for the lock held shared, as by running validates
// and readers: behind that hold every writer must answer while it is held.
func TestLockWait(t *testing.T) {
	const hold = 300 * time.Millisecond
	minWait := float64((hold - 50*time.Millisecond).Milliseconds())
	const maxIdle = 100.0

	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commit(t, wt, map[string]string{"README": "r\n"})
	ingestPack(t, s.URL, "7", selfPack(t, wt, base))
	putRefs(t, s.URL, base)
	prev := base
	next := func(name string) (string, []byte) {
		tip := commit(t, wt, map[string]string{name: strings.Repeat(name, 500) + "\n"})
		p := packOfWt(t, wt, tip, prev)
		prev = tip
		return tip, p
	}
	decode := func(what string, code int, body []byte) map[string]any {
		t.Helper()
		if code != 200 {
			t.Fatalf("%s: %d %s", what, code, body)
		}
		var v map[string]any
		if err := json.Unmarshal(body, &v); err != nil {
			t.Fatalf("%s: %v: %s", what, err, body)
		}
		return v
	}
	wait := func(what string, v map[string]any, field string) float64 {
		t.Helper()
		ms, ok := v[field].(float64)
		if !ok {
			t.Fatalf("%s: no %s in %v", what, field, v)
		}
		return ms
	}
	refsPost := func(tip string) (int, []byte) {
		return postBody(t, s.URL+"/refs?net=7", []byte(tip+" refs/heads/main\n"))
	}
	ingest := func(p []byte) (int, []byte) { return postBody(t, s.URL+"/ingest?net=7&sha256="+sumOf(p), p) }
	batch := func(p []byte) (int, []byte) {
		return postBody(t, s.URL+"/ingest-batch?net=7", append([]byte(fmt.Sprintf("%s %d\n", sumOf(p), len(p))), p...))
	}
	validate := func(tip string, p []byte) (int, []byte) {
		code, _, body := validateDetail(t, s.URL, []string{tip}, nil, p)
		return code, []byte(body)
	}
	// contended runs f while the test holds a lock of the network for hold, the writer lock or the lock alone, and
	// answers f's status and body; f must not answer before the lock is let go.
	contended := func(what string, excl bool, f func() (int, []byte)) (int, []byte) {
		t.Helper()
		l := sync.Locker(writer("7"))
		if excl {
			l = lock("7")
		}
		l.Lock()
		type answer struct {
			code int
			body []byte
		}
		done := make(chan answer, 1)
		go func() {
			c, b := f()
			done <- answer{c, b}
		}()
		time.Sleep(hold)
		early := false
		select {
		case a := <-done:
			early = true
			done <- a
		default:
		}
		l.Unlock()
		a := <-done
		if early {
			t.Fatalf("%s answered while the test held the lock: %d %s", what, a.code, a.body)
		}
		return a.code, a.body
	}

	// Uncontended: every answer carries its wait, and it is small.
	tip, p := next("idle")
	code, body := ingest(p)
	v := decode("ingest", code, body)
	for _, f := range []string{"lock_wait_ms", "lock_excl_wait_ms"} {
		if ms := wait("ingest", v, f); ms >= maxIdle {
			t.Fatalf("uncontended ingest: %s %v", f, ms)
		}
	}
	code, body = refsPost(tip)
	if ms := wait("refs", decode("refs", code, body), "lock_wait_ms"); ms >= maxIdle {
		t.Fatalf("uncontended refs: lock_wait_ms %v", ms)
	}
	vtip, vp := next("validate")
	prev = tip // the validated tip is not ingested; the next packs build on the refs' tip
	run(t, wt, "", "reset", "-q", "--hard", tip)
	code, body = validate(vtip, vp)
	if ms := wait("validate", decode("validate", code, body), "lock_wait_ms"); ms >= maxIdle {
		t.Fatalf("uncontended validate: lock_wait_ms %v", ms)
	}
	code, _, dbody := validateDetail(t, s.URL, nil, []update{{New: tip, Old: base, Ref: "refs/heads/main"}}, nil)
	if ms := wait("detail-only validate", decode("detail-only validate", code, []byte(dbody)), "lock_wait_ms"); ms >= maxIdle {
		t.Fatalf("uncontended detail-only validate: lock_wait_ms %v", ms)
	}
	tip, p = next("batch")
	code, body = batch(p)
	if ms := wait("ingest-batch", decode("ingest-batch", code, body), "lock_wait_ms"); ms >= maxIdle {
		t.Fatalf("uncontended ingest-batch: lock_wait_ms %v", ms)
	}

	// Writers beside the lock held shared, as beside running validates and slow readers: each answers meanwhile.
	beside := func(what string, f func() (int, []byte)) {
		t.Helper()
		l := lock("7")
		l.RLock()
		defer l.RUnlock()
		done := make(chan string, 1)
		go func() {
			c, b := f()
			done <- fmt.Sprintf("%d %s", c, b)
		}()
		select {
		case got := <-done:
			if !strings.HasPrefix(got, "200 ") {
				t.Fatalf("%s beside the lock held shared: %s", what, got)
			}
		case <-time.After(10 * time.Second):
			t.Fatalf("%s waited for the network's lock held shared", what)
		}
	}
	tip, p = next("beside")
	beside("ingest", func() (int, []byte) { return ingest(p) })
	beside("refs", func() (int, []byte) { return refsPost(tip) })
	_, p = next("beside-batch")
	beside("ingest-batch", func() (int, []byte) { return batch(p) })

	// Writers behind the writer lock held, as behind another catch-up's writer.
	tip, p = next("ingest")
	code, body = contended("ingest", false, func() (int, []byte) { return ingest(p) })
	v = decode("ingest", code, body)
	for _, f := range []string{"lock_wait_ms", "lock_excl_wait_ms"} {
		if ms := wait("ingest", v, f); ms < minWait {
			t.Fatalf("ingest held off %v: %s %v", hold, f, ms)
		}
	}
	code, body = contended("refs", false, func() (int, []byte) { return refsPost(tip) })
	if ms := wait("refs", decode("refs", code, body), "lock_wait_ms"); ms < minWait {
		t.Fatalf("refs held off %v: lock_wait_ms %v", hold, ms)
	}
	_, p = next("batch2")
	code, body = contended("ingest-batch", false, func() (int, []byte) { return batch(p) })
	if ms := wait("ingest-batch", decode("ingest-batch", code, body), "lock_wait_ms"); ms < minWait {
		t.Fatalf("ingest-batch held off %v: lock_wait_ms %v", hold, ms)
	}

	// A validate behind the lock held alone, as behind a catch-up's writer.
	vtip, vp = next("validate2")
	code, body = contended("validate", true, func() (int, []byte) { return validate(vtip, vp) })
	if ms := wait("validate", decode("validate", code, body), "lock_wait_ms"); ms < minWait {
		t.Fatalf("validate held off %v: lock_wait_ms %v", hold, ms)
	}
}
