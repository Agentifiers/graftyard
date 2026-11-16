package main

import (
	"io"
	"net/http"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"testing"
	"time"
)

// A roll carries exactly its inputs' objects and is complete even when other packs on the shard hold related history.
// Its inputs are stored /promote answers, which already contain all their own delta bases, so index-pack in an empty
// repository needs neither --fix-thin nor an alternate and itself proves the rolled pack needs no external delta base.
func TestRollPacksExactlyItsInputs(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	call(t, s, "POST", "/init?net=8")
	wt := t.TempDir()
	base := history(t, wt, 4, 80, 2)
	// Start the promoted ranges at an unrelated root: the first range is self-contained even before /promote.
	run(t, wt, "", "checkout", "--orphan", "roll")
	run(t, wt, "", "rm", "-q", "-r", "--cached", ".")
	tips := moreHistory(t, wt, 4, 80, 6)
	ingestPack(t, s.URL, "8", selfPack(t, wt, base))

	var stored []string
	previous := base
	for i := 0; i < len(tips); i += 2 {
		tip := tips[i+1]
		raw := packOfWt(t, wt, tip, previous)
		sum := ingestPack(t, s.URL, "8", raw)
		complete := promote(t, s.URL, "8", []string{sum})
		storedSum := stage(t, s.URL, "7", complete, idxFor(t, complete))
		if code, answer, body := ingestStaged(t, s.URL, "7", storedSum); code != 200 || answer.Idx != "used" {
			t.Fatalf("ingest stored promotion: %d %+v %s", code, answer, body)
		}
		stored = append(stored, storedSum)
		previous = tip
	}
	if len(stored) != 3 {
		t.Fatalf("made %d stored promotions, want 3", len(stored))
	}

	// Roll the second and third, not the first. Only the stored promotions are in network 7; their raw source packs
	// remain in network 8, so excluding other packs cannot exclude a promoted object's duplicate in its source pack.
	sums := stored[1:]
	code, pack := postBody(t, s.URL+"/roll?net=7", []byte(strings.Join(sums, "\n")+"\n"))
	if code != 200 {
		t.Fatalf("roll: %d %.200s", code, pack)
	}
	dir := t.TempDir()
	run(t, dir, "", "init", "-q", "--bare", "check.git")
	check := filepath.Join(dir, "check.git")
	run(t, check, string(pack), "index-pack", "--stdin")
	idx, _ := filepath.Glob(filepath.Join(check, "objects", "pack", "pack-*.idx"))
	if len(idx) != 1 {
		t.Fatalf("index-pack left %v", idx)
	}
	got := indexObjects(t, idx[0])
	wantSet := map[string]bool{}
	for _, sum := range sums {
		for _, oid := range objsOf(t, "7", sum) {
			wantSet[oid] = true
		}
	}
	var want []string
	for oid := range wantSet {
		want = append(want, oid)
	}
	sort.Strings(want)
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("rolled pack has %d objects, want %d; extra %v, missing %v", len(got), len(want),
			difference(got, want), difference(want, got))
	}
}

func TestRollRefusesAnUnknownOrSinglePack(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	one := strings.Repeat("0", 64)
	if code, body := postBody(t, s.URL+"/roll?net=7", []byte(one+"\n")); code != 400 ||
		!strings.Contains(string(body), "a roll takes at least two packs") {
		t.Fatalf("one pack: %d %s", code, body)
	}
	if code, body := postBody(t, s.URL+"/roll?net=7", []byte(one+"\n"+one+"\n")); code != 400 {
		t.Fatalf("one distinct pack repeated: %d %s", code, body)
	}
	unknown := one + "\n" + strings.Repeat("1", 64) + "\n"
	if code, body := postBody(t, s.URL+"/roll?net=7", []byte(unknown)); code != 409 ||
		!strings.Contains(string(body), "was not ingested here with its name") {
		t.Fatalf("unknown packs: %d %s", code, body)
	}
}

func TestRollWaitsForTheRepack(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commit(t, wt, map[string]string{"f": "base\n"})
	first := ingestPack(t, s.URL, "7", selfPack(t, wt, base))
	tip := commit(t, wt, map[string]string{"f": "tip\n", "g": "new\n"})
	second := ingestPack(t, s.URL, "7", selfPack(t, wt, tip))

	type answer struct {
		code int
		body []byte
		err  error
	}
	done := make(chan answer, 1)
	repacking.Lock()
	locked := true
	defer func() {
		if locked {
			repacking.Unlock()
		}
	}()
	go func() {
		resp, err := http.Post(s.URL+"/roll?net=7", "application/octet-stream", strings.NewReader(first+"\n"+second+"\n"))
		if err != nil {
			done <- answer{err: err}
			return
		}
		body, readErr := io.ReadAll(resp.Body)
		resp.Body.Close()
		if err == nil {
			err = readErr
		}
		done <- answer{code: resp.StatusCode, body: body, err: err}
	}()
	select {
	case got := <-done:
		t.Fatalf("roll answered while the repack was held: %d %.200s %v", got.code, got.body, got.err)
	case <-time.After(200 * time.Millisecond):
	}
	repacking.Unlock()
	locked = false
	select {
	case got := <-done:
		if got.err != nil || got.code != 200 {
			t.Fatalf("roll after the repack: %d %.200s %v", got.code, got.body, got.err)
		}
	case <-time.After(20 * time.Second):
		t.Fatal("roll did not answer after the repack was released")
	}
}

// Keep the metadata's exact form honest independently of pack-objects: the .name is the bare pack name, not a path.
func TestRollRecordsThePackName(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	tip := commit(t, wt, map[string]string{"f": "contents\n"})
	pack := selfPack(t, wt, tip)
	sum := ingestPack(t, s.URL, "7", pack)
	idx, _ := filepath.Glob(filepath.Join(repo("7"), "objects", "pack", "pack-*.idx"))
	if len(idx) != 1 {
		t.Fatalf("ingest left indexes %v", idx)
	}
	want := strings.TrimSuffix(strings.TrimPrefix(filepath.Base(idx[0]), "pack-"), ".idx")
	b, err := os.ReadFile(filepath.Join(repo("7"), "graft", sum+".name"))
	if err != nil {
		t.Fatal(err)
	}
	if string(b) != want {
		t.Fatalf("recorded pack name %q, want %q", b, want)
	}
}
