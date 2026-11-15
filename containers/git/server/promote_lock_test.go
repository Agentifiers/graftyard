package main

import (
	"crypto/rand"
	"fmt"
	"io"
	"net/http"
	"strings"
	"testing"
	"time"
)

// A promotion whose reader has stopped reading holds no network lock. The archive's tee reads /promote's answer and
// hands it to the promotion's own validate, which waits for a catch-up before it reads; the catch-up's /ingest takes
// the lock alone. With the pack written under the shared lock, the stalled reader kept the lock, the /ingest waited
// on it, and every later validate and merge queued behind the waiting writer until the validate gave up (B0-31
// m3fr6, `tests/am1/results/promote-unlock.md`). Here the promoted pack is several megabytes of random bytes, more
// than the sockets buffer, and its reader stops after the first bytes: an /ingest and then a merge must still answer.
func TestPromoteWritesItsPackOutsideTheLock(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commit(t, wt, map[string]string{"README": "r\n"})
	ingestPack(t, s.URL, "7", selfPack(t, wt, base))
	putRefs(t, s.URL, base)
	// Random contents do not compress, so the promoted pack stays about as large as the blobs.
	files := map[string]string{}
	for i := range 4 {
		b := make([]byte, 2<<20)
		rand.Read(b)
		files[fmt.Sprintf("big%d.bin", i)] = string(b)
	}
	tip := commit(t, wt, files)
	sum := ingestPack(t, s.URL, "7", packOfWt(t, wt, tip, base))
	next := commit(t, wt, map[string]string{"after.txt": "after\n"})
	later := packOfWt(t, wt, next, tip)

	// A promotion refused for a pack this disk never ingested leaves no lock behind either.
	if code, body := postBody(t, s.URL+"/promote?net=7", []byte(strings.Repeat("0", 64)+"\n")); code != 409 {
		t.Fatalf("promote of an unknown pack: %d %s", code, body)
	}
	r, err := http.Post(s.URL+"/promote?net=7", "", strings.NewReader(sum+"\n"))
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	if r.StatusCode != 200 {
		b, _ := io.ReadAll(r.Body)
		t.Fatalf("promote: %d %s", r.StatusCode, b)
	}
	// The reader takes the first bytes and stops, as a tee whose validate waits does.
	if _, err := io.ReadFull(r.Body, make([]byte, 4096)); err != nil {
		t.Fatal(err)
	}

	done := make(chan string, 2)
	go func() {
		code, body := postBody(t, s.URL+"/ingest?net=7&sha256="+sumOf(later), later)
		done <- fmt.Sprintf("ingest %d %.80s", code, body)
	}()
	go func() {
		code, body := postBody(t, s.URL+"/merge?net=7&base="+base+"&head="+tip, nil)
		done <- fmt.Sprintf("merge %d %.80s", code, body)
	}()
	for range 2 {
		select {
		case got := <-done:
			if !strings.Contains(got, " 200 ") {
				t.Fatalf("%s while the promotion's reader waited", got)
			}
		case <-time.After(20 * time.Second):
			t.Fatal("an /ingest or a merge waited on a promotion whose reader had stopped reading")
		}
	}
	// The promotion still delivers its whole pack once its reader goes on.
	rest, err := io.ReadAll(r.Body)
	if err != nil || 4096+len(rest) < 8_000_000 {
		t.Fatalf("the rest of the promoted pack: %d bytes, %v", len(rest), err)
	}
}
