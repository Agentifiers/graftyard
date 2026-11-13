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
