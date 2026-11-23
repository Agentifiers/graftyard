package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"testing"
)

func batchBody(packs ...[]byte) ([]byte, []string) {
	var b bytes.Buffer
	var sums []string
	for _, p := range packs {
		s := sha256.Sum256(p)
		sums = append(sums, hex.EncodeToString(s[:]))
		fmt.Fprintf(&b, "%s %d\n", sums[len(sums)-1], len(p))
		b.Write(p)
	}
	return b.Bytes(), sums
}

func objsOf(t *testing.T, net, sum string) []string {
	t.Helper()
	b, err := os.ReadFile(filepath.Join(repo(net), "graft", sum+".objs"))
	if err != nil {
		t.Fatalf("no objects recorded for %s: %v", sum[:12], err)
	}
	o := strings.Fields(string(b))
	sort.Strings(o)
	return o
}

// /ingest-batch leaves the repository whole and records each pack's objects, in one call per run of packs: packs thin against earlier packs of the same call, two packs carrying the same object (which --strict refuses in one
// pack), a pack already ingested, and a pack whose base comes later in the call, which answers "later" and goes in
// once it is sent again.
func TestIngestBatchMatchesOneIngestPerPack(t *testing.T) {
	s := serve(t)
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commit(t, wt, map[string]string{"f": "base\n"})
	full, err := gitOut(wt, base+"\n", "pack-objects", "--stdout", "--revs", "-q")
	if err != nil {
		t.Fatal(err)
	}
	// Two branches from base each add the same blob, so packs a and b both carry it.
	run(t, wt, "", "checkout", "-q", "-b", "x", base)
	a := commit(t, wt, map[string]string{"same": "shared bytes\n", "x": "x\n"})
	run(t, wt, "", "checkout", "-q", "-b", "y", base)
	b := commit(t, wt, map[string]string{"same": "shared bytes\n", "y": "y\n"})
	c := commit(t, wt, map[string]string{"y": "y2\n"})
	d := commit(t, wt, map[string]string{"y": "y3\n"})
	pa, pb, pc, pd := packOfWt(t, wt, a, base), packOfWt(t, wt, b, base), packOfWt(t, wt, c, b), packOfWt(t, wt, d, c)

	call(t, s, "POST", "/init?net=7")
	type out struct {
		Results []string `json:"results"`
	}
	post := func(packs ...[]byte) (out, []string) {
		t.Helper()
		body, sums := batchBody(packs...)
		code, got := postBody(t, s.URL+"/ingest-batch?net=7", body)
		if code != 200 {
			t.Fatalf("ingest-batch: %d %s", code, got)
		}
		var o out
		if err := json.Unmarshal(got, &o); err != nil {
			t.Fatal(err)
		}
		return o, sums
	}
	o, _ := post(full)
	if want := []string{"done"}; !reflect.DeepEqual(o.Results, want) {
		t.Fatalf("first call %v, want %v", o.Results, want)
	}
	// pa and pb carry the same blob, and pd is thin against c, which no pack of the call brings.
	o, _ = post(pa, pb, pd)
	if want := []string{"done", "done", "later"}; !reflect.DeepEqual(o.Results, want) {
		t.Fatalf("second call %v, want %v", o.Results, want)
	}
	if _, err := os.Stat(filepath.Join(repo("7"), "objects", d[:2], d[2:])); err == nil {
		t.Fatal("a later pack left an object behind")
	}
	// pd before pc in one call: its base comes later in the same call. pa again: already here.
	o, _ = post(pd, pc, pa)
	if want := []string{"done", "done", "done"}; !reflect.DeepEqual(o.Results, want) {
		t.Fatalf("third call %v, want %v", o.Results, want)
	}
	// Each pack's objects are the ones it carries, the objects /promote packs; a base --fix-thin appended is an earlier
	// pack's.
	_, all := batchBody(full, pa, pb, pc, pd)
	for i, revs := range []string{base, a + "\n^" + base, b + "\n^" + base, c + "\n^" + b, d + "\n^" + c} {
		listed, err := gitOut(wt, revs+"\n", "rev-list", "--objects", "--stdin")
		if err != nil {
			t.Fatal(err)
		}
		var want []string
		for _, l := range strings.Split(strings.TrimSpace(string(listed)), "\n") {
			want = append(want, strings.Fields(l)[0])
		}
		sort.Strings(want)
		if got := objsOf(t, "7", all[i]); !reflect.DeepEqual(got, want) {
			t.Fatalf("pack %d recorded %v, it carries %v", i, got, want)
		}
	}
	putRefs(t, s.URL, d)
	if out := run(t, repo("7"), "", "fsck", "--strict", "--no-dangling"); out != "" {
		t.Fatalf("fsck: %s", out)
	}
	for _, tip := range []string{a, d} {
		run(t, repo("7"), "", "cat-file", "-e", tip)
	}

	// A body whose bytes do not match the sum it names fails the call.
	body, _ := batchBody(pa)
	body[len(body)-30] ^= 1
	if code, got := postBody(t, s.URL+"/ingest-batch?net=7", body); code != 422 || !strings.Contains(string(got), "checksum") {
		t.Fatalf("a corrupt pack answered %d: %s", code, got)
	}
}

// /ingest-spooled-batch (catchup-lag) ingests staged stored packs as /ingest &spooled=1 would one by one: in order, a
// pack thin against a later pack of the call answering "later" and staying staged, two packs carrying the same object,
// a pack a validate kept aside moved in with no index-pack, and each pack's objects recorded under its own sum, which
// /promote and /have rely on. A pack neither staged nor here fails the call 404, leaving the others staged.
func TestIngestSpooledBatchMatchesOneSpooledIngestPerPack(t *testing.T) {
	s := serve(t)
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commit(t, wt, map[string]string{"f": "base\n"})
	full, err := gitOut(wt, base+"\n", "pack-objects", "--stdout", "--revs", "-q")
	if err != nil {
		t.Fatal(err)
	}
	run(t, wt, "", "checkout", "-q", "-b", "x", base)
	a := commit(t, wt, map[string]string{"same": "shared bytes\n", "x": "x\n"})
	run(t, wt, "", "checkout", "-q", "-b", "y", base)
	b := commit(t, wt, map[string]string{"same": "shared bytes\n", "y": "y\n"})
	c := commit(t, wt, map[string]string{"y": "y2\n"})
	d := commit(t, wt, map[string]string{"y": "y3\n"})
	e := commit(t, wt, map[string]string{"y": "y4\n"})
	pa, pb, pc, pd, pe := packOfWt(t, wt, a, base), packOfWt(t, wt, b, base), packOfWt(t, wt, c, b),
		packOfWt(t, wt, d, c), packOfWt(t, wt, e, d)
	call(t, s, "POST", "/init?net=7")
	_, sums := batchBody(full, pa, pb, pc, pd, pe)
	stage := func(i int, p []byte) {
		t.Helper()
		if code, got := postBody(t, s.URL+"/spool?net=7&sha256="+sums[i], p); code != 200 {
			t.Fatalf("spool %d: %d %s", i, code, got)
		}
	}
	type out struct {
		Results []string `json:"results"`
		Adopted int      `json:"adopted"`
	}
	batch := func(want int, is ...int) out {
		t.Helper()
		var body strings.Builder
		for _, i := range is {
			body.WriteString(sums[i] + "\n")
		}
		code, got := postBody(t, s.URL+"/ingest-spooled-batch?net=7", []byte(body.String()))
		if code != want {
			t.Fatalf("ingest-spooled-batch %v: %d %s", is, code, got)
		}
		var o out
		if code == 200 {
			if err := json.Unmarshal(got, &o); err != nil {
				t.Fatal(err)
			}
		}
		return o
	}
	staged := func(i int) bool {
		_, err := os.Stat(storedPath("7", sums[i], ".pack"))
		return err == nil
	}
	for i, p := range [][]byte{full, pa, pb, pd} {
		stage([]int{0, 1, 2, 4}[i], p)
	}
	// pd (index 4) is thin against c, which no pack of the call brings; pa and pb carry the same blob.
	if o := batch(200, 0, 1, 2, 4); !reflect.DeepEqual(o.Results, []string{"done", "done", "done", "later"}) {
		t.Fatalf("first call %v", o.Results)
	}
	if !staged(4) || staged(1) {
		t.Fatal("a later pack must stay staged and a done pack's staged file must go")
	}
	if _, err := os.Stat(filepath.Join(repo("7"), "objects", d[:2], d[2:])); err == nil {
		t.Fatal("a later pack left an object behind")
	}
	// pc's validate keeps it aside, so it moves in with no index-pack; pd (staged), its base now in the same call,
	// and pa again (already here) go in too. pe is neither staged nor here: the call fails before any of it.
	putRefs(t, s.URL, b)
	if code, body, err := validateReq(s.URL, "7", vspec{pack: pc, tips: []string{b}}); err != nil || code != 200 {
		t.Fatalf("pc's validate: %d %s %v", code, body, err)
	}
	batch(404, 3, 4, 5)
	if !staged(4) {
		t.Fatal("a failed call removed a staged pack")
	}
	o := batch(200, 3, 4, 1)
	if !reflect.DeepEqual(o.Results, []string{"done", "done", "done"}) || o.Adopted != 1 {
		t.Fatalf("second call %v, %d adopted, want 3 done and pc adopted", o.Results, o.Adopted)
	}
	for i, revs := range []string{base, a + "\n^" + base, b + "\n^" + base, c + "\n^" + b, d + "\n^" + c} {
		listed, err := gitOut(wt, revs+"\n", "rev-list", "--objects", "--stdin")
		if err != nil {
			t.Fatal(err)
		}
		var want []string
		for _, l := range strings.Split(strings.TrimSpace(string(listed)), "\n") {
			want = append(want, strings.Fields(l)[0])
		}
		sort.Strings(want)
		if got := objsOf(t, "7", sums[i]); !reflect.DeepEqual(got, want) {
			t.Fatalf("pack %d recorded %v, it carries %v", i, got, want)
		}
	}
	putRefs(t, s.URL, d)
	if out := run(t, repo("7"), "", "fsck", "--strict", "--no-dangling"); out != "" {
		t.Fatalf("fsck: %s", out)
	}
}
