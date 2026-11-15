package main

import (
	"bytes"
	"fmt"
	"math/rand/v2"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"testing"
)

func rangeObjects(t *testing.T, wt, tip, base string) []string {
	t.Helper()
	out, err := gitOut(wt, tip+"\n^"+base+"\n", "rev-list", "--objects", "--stdin")
	if err != nil {
		t.Fatal(err)
	}
	seen := map[string]bool{}
	var objs []string
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		if f := strings.Fields(line); len(f) > 0 && !seen[f[0]] {
			seen[f[0]] = true
			objs = append(objs, f[0])
		}
	}
	sort.Strings(objs)
	return objs
}

// moreHistory continues history's work tree, editing every file in every commit so the thin packs have useful deltas.
func moreHistory(t *testing.T, wt string, files, lines, commits int) []string {
	t.Helper()
	rng := rand.New(rand.NewPCG(7, 9))
	tips := make([]string, commits)
	for c := range commits {
		set := map[string]string{}
		for f := range files {
			p := fmt.Sprintf("d%d/f%d.txt", f%4, f)
			b, err := os.ReadFile(filepath.Join(wt, p))
			if err != nil {
				t.Fatal(err)
			}
			text := strings.Split(strings.TrimSuffix(string(b), "\n"), "\n")
			if len(text) != lines {
				t.Fatalf("%s has %d lines, want %d", p, len(text), lines)
			}
			for range 3 {
				text[rng.IntN(lines)] = fmt.Sprintf("more-%d %016x", c, rng.Uint64())
			}
			set[p] = strings.Join(text, "\n") + "\n"
		}
		tips[c] = commit(t, wt, set)
	}
	return tips
}

func ingestPack(t *testing.T, url, net string, pack []byte) string {
	t.Helper()
	sum := sumOf(pack)
	if code, body := postBody(t, url+"/ingest?net="+net+"&sha256="+sum, pack); code != 200 {
		t.Fatalf("ingest %s: %d %s", sum[:12], code, body)
	}
	return sum
}

func promote(t *testing.T, url, net string, sums []string) []byte {
	t.Helper()
	code, body := postBody(t, url+"/promote?net="+net, []byte(strings.Join(sums, "\n")+"\n"))
	if code != 200 {
		t.Fatalf("promote: %d %s", code, body)
	}
	return body
}

// A thin pack records only what its sender put in it, not the bases --fix-thin appended. This holds on direct ingest,
// when a validated pack is adopted, and in ingest-batch's one-pack branch.
func TestRecordKeepsOnlyThePacksOwnObjects(t *testing.T) {
	s := serve(t)
	wt := t.TempDir()
	base := history(t, wt, 6, 100, 3)
	tips := moreHistory(t, wt, 6, 100, 3)
	tip := tips[len(tips)-1]
	full, thin := selfPack(t, wt, base), packOfWt(t, wt, tip, base)
	want := rangeObjects(t, wt, tip, base)

	call(t, s, "POST", "/init?net=7")
	ingestPack(t, s.URL, "7", full)
	ingestPack(t, s.URL, "7", thin)
	if got := objsOf(t, "7", sumOf(thin)); !reflect.DeepEqual(got, want) {
		t.Fatalf("direct ingest recorded %d objects, the thin pack carries %d: extra %v", len(got), len(want), difference(got, want))
	}

	call(t, s, "POST", "/init?net=8")
	ingestPack(t, s.URL, "8", full)
	code, body := postBody(t, s.URL+"/validate?net=8&tips="+tip, thin)
	if code != 200 {
		t.Fatalf("validate: %d %s", code, body)
	}
	sum := sumOf(thin)
	code, body = postBody(t, s.URL+"/ingest?net=8&sha256="+sum, nil)
	if code != 200 || !bytes.Contains(body, []byte(`"adopted":true`)) {
		t.Fatalf("adopt: %d %s", code, body)
	}
	if got := objsOf(t, "8", sum); !reflect.DeepEqual(got, want) {
		t.Fatalf("adopted ingest recorded %d objects, the thin pack carries %d: extra %v", len(got), len(want), difference(got, want))
	}

	call(t, s, "POST", "/init?net=9")
	ingestPack(t, s.URL, "9", full)
	batch, sums := batchBody(thin)
	code, body = postBody(t, s.URL+"/ingest-batch?net=9", batch)
	if code != 200 {
		t.Fatalf("ingest-batch: %d %s", code, body)
	}
	if got := objsOf(t, "9", sums[0]); !reflect.DeepEqual(got, want) {
		t.Fatalf("batched ingest recorded %d objects, the thin pack carries %d: extra %v", len(got), len(want), difference(got, want))
	}
}

func difference(got, want []string) []string {
	have := map[string]bool{}
	for _, oid := range want {
		have[oid] = true
	}
	var extra []string
	for _, oid := range got {
		if !have[oid] {
			extra = append(extra, oid)
		}
	}
	return extra
}

// Promotion names the objects for delta grouping without adding or dropping any: the answer indexes strictly into an
// empty repository, so every delta base is present, and its index is exactly the six ranges' union.
func TestPromotedPackIsCompleteAndHoldsExactlyTheRange(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	base := history(t, wt, 12, 400, 3)
	// An unrelated root makes the exact range self-contained even to index-pack --strict, which otherwise insists on
	// the first range's excluded parent. The base pack is still present for thin packs to name as delta bases.
	run(t, wt, "", "checkout", "--orphan", "promoted")
	run(t, wt, "", "rm", "-q", "-r", "--cached", ".")
	tips := moreHistory(t, wt, 12, 400, 12)
	ingestPack(t, s.URL, "7", selfPack(t, wt, base))
	wantSet := map[string]bool{}
	var sums []string
	previous := base
	for i := 0; i < len(tips); i += 2 {
		tip := tips[i+1]
		sums = append(sums, ingestPack(t, s.URL, "7", packOfWt(t, wt, tip, previous)))
		for _, oid := range rangeObjects(t, wt, tip, previous) {
			wantSet[oid] = true
		}
		previous = tip
	}
	pack := promote(t, s.URL, "7", sums)

	dir := t.TempDir()
	run(t, dir, "", "init", "-q", "--bare", "check.git")
	check := filepath.Join(dir, "check.git")
	// No --fix-thin and no alternate: strict indexing itself proves the promoted pack is self-contained.
	run(t, check, string(pack), "index-pack", "--strict", "--stdin")
	idx, _ := filepath.Glob(filepath.Join(check, "objects", "pack", "pack-*.idx"))
	if len(idx) != 1 {
		t.Fatalf("index-pack left %v", idx)
	}
	got := indexObjects(t, idx[0])
	var want []string
	for oid := range wantSet {
		want = append(want, oid)
	}
	sort.Strings(want)
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("promoted pack has %d objects, want %d; extra %v, missing %v", len(got), len(want), difference(got, want), difference(want, got))
	}
	// The same ids, in handlePromote's order but with no names, isolate the other half of the change. This fixture's
	// input packs need no appended bases, so main's answer is exactly this unnamed pack.
	seenIDs := map[string]bool{}
	var ordered []string
	for _, sum := range sums {
		for _, oid := range objsOf(t, "7", sum) {
			if !seenIDs[oid] {
				seenIDs[oid] = true
				ordered = append(ordered, oid)
			}
		}
	}
	unnamed, err := gitOut(repo("7"), strings.Join(ordered, "\n")+"\n", "pack-objects", "--stdout", "-q", "--delta-base-offset")
	if err != nil {
		t.Fatal(err)
	}
	if len(pack) >= len(unnamed) {
		t.Fatalf("named promoted pack is %d bytes, unnamed is %d", len(pack), len(unnamed))
	}
}

func indexObjects(t *testing.T, idx string) []string {
	t.Helper()
	f, err := os.Open(idx)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	out, err := git(data, nil, f, "show-index")
	if err != nil {
		t.Fatal(err)
	}
	var objs []string
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		if f := strings.Fields(line); len(f) >= 2 {
			objs = append(objs, f[1])
		}
	}
	sort.Strings(objs)
	return objs
}

// Main listed every object of each fixed thin pack, bases included, and gave pack-objects no names. The promoted pack
// must be at least 8% smaller; this history measures 58%.
func TestPromotedPackIsSmallerThanMainsWay(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	base := history(t, wt, 12, 400, 3)
	tips := moreHistory(t, wt, 12, 400, 12)
	ingestPack(t, s.URL, "7", selfPack(t, wt, base))
	seen := map[string]bool{}
	var mainIDs, sums []string
	previous := base
	for i := 0; i < len(tips); i += 2 {
		tip := tips[i+1]
		before, _ := filepath.Glob(filepath.Join(repo("7"), "objects", "pack", "pack-*.idx"))
		// Real thin pushes, so --fix-thin appends bases that main's list repeated.
		pushed, err := gitOut(wt, tip+"\n^"+previous+"\n", "pack-objects", "--stdout", "--revs", "--thin")
		if err != nil {
			t.Fatal(err)
		}
		sums = append(sums, ingestPack(t, s.URL, "7", pushed))
		after, _ := filepath.Glob(filepath.Join(repo("7"), "objects", "pack", "pack-*.idx"))
		old := map[string]bool{}
		for _, idx := range before {
			old[idx] = true
		}
		var added []string
		for _, idx := range after {
			if !old[idx] {
				added = append(added, idx)
			}
		}
		if len(added) != 1 {
			t.Fatalf("ingest added indexes %v", added)
		}
		// This is main's object list: every id in the fixed pack's idx, including appended bases, deduplicated, unnamed.
		for _, oid := range indexObjects(t, added[0]) {
			if !seen[oid] {
				seen[oid] = true
				mainIDs = append(mainIDs, oid)
			}
		}
		previous = tip
	}
	mainWay, err := gitOut(repo("7"), strings.Join(mainIDs, "\n")+"\n", "pack-objects", "--stdout", "-q", "--delta-base-offset")
	if err != nil {
		t.Fatal(err)
	}
	promoted := promote(t, s.URL, "7", sums)
	t.Logf("promoted: %d bytes; main's way: %d bytes (%.1f%%)", len(promoted), len(mainWay), 100*float64(len(promoted))/float64(len(mainWay)))
	if len(promoted)*100 > len(mainWay)*92 {
		t.Fatalf("promoted pack is %d bytes, main's way is %d; want at most 92%%", len(promoted), len(mainWay))
	}
}
