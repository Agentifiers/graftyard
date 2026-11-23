package main

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"math/rand/v2"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// Stored packs fetched several at a time and ingested in log order: /spool stages a pack with no lock, /spool-idx its
// stored index, and /ingest &spooled=1 loads that index without index-pack when checkIdx holds, or falls back to
// index-pack. /idx hands the promotion the index /validate made, and /connected is a rehydrate's backstop.

func sumOf(b []byte) string {
	s := sha256.Sum256(b)
	return hex.EncodeToString(s[:])
}

// storedAnswer is /ingest &spooled=1's answer.
type storedAnswer struct {
	Ingested bool    `json:"ingested"`
	Bytes    int64   `json:"bytes"`
	Idx      string  `json:"idx"`
	IdxError string  `json:"idxError"`
	IndexMs  float64 `json:"index_ms"`
	LockMs   float64 `json:"lock_ms"`
}

func putBody(t *testing.T, url string, body []byte) (int, []byte) {
	t.Helper()
	req, _ := http.NewRequest("PUT", url, bytes.NewReader(body))
	r, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	b, _ := io.ReadAll(r.Body)
	return r.StatusCode, b
}

// history commits a work tree of text files edited line by line, so its packs hold real deltas, and returns the tip.
func history(t *testing.T, wt string, files, lines, commits int) string {
	t.Helper()
	rng := rand.New(rand.NewPCG(1, 2))
	run(t, wt, "", "init", "-q", "-b", "main")
	text := make([][]string, files)
	for f := range text {
		for range lines {
			text[f] = append(text[f], fmt.Sprintf("%016x %016x %016x", rng.Uint64(), rng.Uint64(), rng.Uint64()))
		}
	}
	tip := ""
	for c := range commits {
		set := map[string]string{}
		for f := range text {
			if c > 0 {
				for range 3 {
					text[f][rng.IntN(lines)] = fmt.Sprintf("c%d %016x", c, rng.Uint64())
				}
			}
			set[fmt.Sprintf("d%d/f%d.txt", f%4, f)] = strings.Join(text[f], "\n") + "\n"
		}
		tip = commit(t, wt, set)
	}
	return tip
}

// selfPack packs everything tip reaches, not thin, as a promotion stores it.
func selfPack(t *testing.T, wt, tip string, opts ...string) []byte {
	t.Helper()
	out, err := gitOut(wt, tip+"\n", append([]string{"pack-objects", "--stdout", "--revs", "-q"}, opts...)...)
	if err != nil {
		t.Fatal(err)
	}
	return out
}

// idxFor indexes pack with git index-pack in a scratch repository, as the promotion's /validate does.
func idxFor(t *testing.T, pack []byte) []byte {
	t.Helper()
	dir := t.TempDir()
	run(t, dir, "", "init", "-q", "--bare", "scratch.git")
	f := filepath.Join(dir, "p.pack")
	if err := os.WriteFile(f, pack, 0o644); err != nil {
		t.Fatal(err)
	}
	run(t, filepath.Join(dir, "scratch.git"), "", "index-pack", "-o", filepath.Join(dir, "p.idx"), f)
	b, err := os.ReadFile(filepath.Join(dir, "p.idx"))
	if err != nil {
		t.Fatal(err)
	}
	return b
}

// stage spools pack to the shard for network net, with its idx when idx is not nil.
func stage(t *testing.T, url, net string, pack, idx []byte) string {
	t.Helper()
	sum := sumOf(pack)
	code, body := postBody(t, url+"/spool?net="+net+"&sha256="+sum, pack)
	var sp struct {
		Bytes int64 `json:"bytes"`
	}
	if json.Unmarshal(body, &sp); code != 200 || sp.Bytes != int64(len(pack)) {
		t.Fatalf("spool: %d %s", code, body)
	}
	if idx != nil {
		code, body := putBody(t, url+"/spool-idx?net="+net+"&sha256="+sum, idx)
		if code != 200 || !strings.Contains(string(body), fmt.Sprintf(`"bytes":%d`, len(idx))) {
			t.Fatalf("spool-idx: %d %s", code, body)
		}
	}
	return sum
}

func ingestStaged(t *testing.T, url, net, sum string) (int, storedAnswer, string) {
	t.Helper()
	code, body := postBody(t, url+"/ingest?net="+net+"&sha256="+sum+"&spooled=1", nil)
	var a storedAnswer
	json.Unmarshal(body, &a)
	return code, a, string(body)
}

func connected(t *testing.T, url, query string) map[string]any {
	t.Helper()
	code, body := postBody(t, url+"/connected?net=7"+query, nil)
	var v map[string]any
	if err := json.Unmarshal(body, &v); code != 200 || err != nil {
		t.Fatalf("connected: %d %s", code, body)
	}
	return v
}

// The repository holds tip and every object it reaches, and the pack's objects are recorded for /promote.
func assertIngested(t *testing.T, net, wt, tip, sum string) {
	t.Helper()
	run(t, repo(net), "", "cat-file", "-e", tip)
	run(t, repo(net), "", "rev-list", "--objects", tip)
	listed := strings.Split(run(t, wt, "", "rev-list", "--objects", tip), "\n")
	if got := objsOf(t, net, sum); len(got) != len(listed) {
		t.Fatalf("recorded %d objects, the pack carries %d", len(got), len(listed))
	}
	if left, _ := filepath.Glob(filepath.Join(data, "spool", "stored-*")); len(left) != 0 {
		t.Fatalf("spooled files left behind: %v", left)
	}
}

func TestSpooledIngestUsesStoredIdx(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	tip := history(t, wt, 16, 800, 12)
	pack := selfPack(t, wt, tip)
	if len(pack) < 200<<10 {
		t.Fatalf("pack is %d bytes, want a few hundred KB", len(pack))
	}
	sum := stage(t, s.URL, "7", pack, idxFor(t, pack))
	code, a, body := ingestStaged(t, s.URL, "7", sum)
	if code != 200 || !a.Ingested || a.Idx != "used" || a.IndexMs != 0 || a.Bytes != int64(len(pack)) ||
		!strings.Contains(body, `"lock_ms"`) {
		t.Fatalf("ingest: %d %s", code, body)
	}
	t.Logf("idx path: %s", strings.TrimSpace(body))
	assertIngested(t, "7", wt, tip, sum)
	// The pack went in under its trailer's name, with no index-pack of its own.
	name := hex.EncodeToString(pack[len(pack)-20:])
	if _, err := os.Stat(filepath.Join(repo("7"), "objects", "pack", "pack-"+name+".idx")); err != nil {
		t.Fatal(err)
	}

	// Already ingested: a second spool reads nothing into the disk.
	if code, body := postBody(t, s.URL+"/spool?net=7&sha256="+sum, pack); code != 200 || !strings.Contains(string(body), `"have":true`) {
		t.Fatalf("spool again: %d %s", code, body)
	}

	if v := connected(t, s.URL, ""); v["connected"] != true || v["refs"] != 0.0 {
		t.Fatalf("connected with no refs: %v", v)
	}
	putRefs(t, s.URL, tip)
	for _, q := range []string{"", "&objects=1", "&objects=tips"} {
		if v := connected(t, s.URL, q); v["connected"] != true || v["refs"] != 1.0 || v["error"] != nil {
			t.Fatalf("connected%s: %v", q, v)
		}
	}
	if out := run(t, repo("7"), "", "fsck", "--strict", "--no-dangling"); out != "" {
		t.Fatalf("fsck: %s", out)
	}
}

func TestSpooledIngestRejectsCorruptIdx(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	tip := history(t, wt, 6, 200, 6)
	pack := selfPack(t, wt, tip)
	idx := idxFor(t, pack)
	idx[len(idx)/2] ^= 0x40
	sum := stage(t, s.URL, "7", pack, idx)
	code, a, body := ingestStaged(t, s.URL, "7", sum)
	if code != 200 || !a.Ingested || a.Idx != "rejected" || a.IdxError == "" || !strings.Contains(body, `"lock_ms"`) {
		t.Fatalf("ingest: %d %s", code, body)
	}
	t.Logf("flipped byte: %s", a.IdxError)
	assertIngested(t, "7", wt, tip, sum)
}

// An idx valid on its own but made for other bytes of the same objects: its pack checksum is not this pack's trailer.
func TestSpooledIngestRejectsAnotherPacksIdx(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	tip := history(t, wt, 6, 200, 6)
	pack := selfPack(t, wt, tip)
	other := selfPack(t, wt, tip, "--window=0")
	if bytes.Equal(pack, other) || !bytes.Equal(pack[:12], other[:12]) {
		t.Fatal("want two different packs of the same objects")
	}
	sum := stage(t, s.URL, "7", pack, idxFor(t, other))
	code, a, body := ingestStaged(t, s.URL, "7", sum)
	if code != 200 || a.Idx != "rejected" || !strings.Contains(a.IdxError, "idx is for pack") {
		t.Fatalf("ingest: %d %s", code, body)
	}
	assertIngested(t, "7", wt, tip, sum)
}

// A thin pack spooled before its base answers 409 and stays spooled, so the same call works once the base is here.
func TestSpooledThinPackWaitsForBase(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	base := history(t, wt, 4, 200, 2)
	tip := commit(t, wt, map[string]string{"d0/f0.txt": "changed\n" + run(t, wt, "", "show", "HEAD:d0/f0.txt")})
	full, thin := selfPack(t, wt, base), packOfWt(t, wt, tip, base)
	sum := stage(t, s.URL, "7", thin, nil)
	if code, _, body := ingestStaged(t, s.URL, "7", sum); code != 409 {
		t.Fatalf("thin pack before its base: %d %s", code, body)
	}
	if _, err := os.Stat(storedPath("7", sum, ".pack")); err != nil {
		t.Fatalf("a 409 dropped the spooled pack: %v", err)
	}
	if code, body := postBody(t, s.URL+"/ingest?net=7&sha256="+sumOf(full), full); code != 200 {
		t.Fatalf("ingest base: %d %s", code, body)
	}
	code, a, body := ingestStaged(t, s.URL, "7", sum)
	if code != 200 || !a.Ingested || a.Idx != "none" {
		t.Fatalf("thin pack after its base: %d %s", code, body)
	}
	run(t, repo("7"), "", "rev-list", "--objects", tip)
	if _, err := os.Stat(storedPath("7", sum, ".pack")); !os.IsNotExist(err) {
		t.Fatalf("spooled pack left behind: %v", err)
	}
	if code, _, body := ingestStaged(t, s.URL, "7", sumOf(full)); code != 200 || !strings.Contains(body, `"ingested":false`) {
		t.Fatalf("an ingested pack: %d %s", code, body)
	}
	if code, _, body := ingestStaged(t, s.URL, "7", strings.Repeat("ab", 32)); code != 404 || !strings.Contains(body, "not spooled") {
		t.Fatalf("a pack never spooled: %d %s", code, body)
	}
}

// Neither path holds the network's lock alone across index-pack: the stored idx's path holds it for two renames and the
// record, and the index-pack path (B0-34, `indexIngest`) indexes under the lock shared and holds it alone only to move
// the pack in and record it, so its lock_ms is far below its index_ms.
func TestSpooledIdxHoldsLockShorter(t *testing.T) {
	s := serve(t)
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	rng := rand.New(rand.NewPCG(3, 4))
	files := map[string][]byte{}
	for i := range 80 {
		b := make([]byte, 64<<10)
		for j := range b {
			b[j] = byte(rng.Uint32())
		}
		files[fmt.Sprintf("bin/%02d", i)] = b
	}
	tip := ""
	for c := range 6 {
		set := map[string]string{}
		for p, b := range files {
			if c > 0 {
				at := rng.IntN(len(b) - 512)
				for j := range 512 {
					b[at+j] = byte(rng.Uint32())
				}
			}
			set[p] = string(b)
		}
		tip = commit(t, wt, set)
	}
	pack := selfPack(t, wt, tip)
	if len(pack) < 5<<20 {
		t.Fatalf("pack is %d bytes, want at least 5 MB", len(pack))
	}
	idx := idxFor(t, pack)

	call(t, s, "POST", "/init?net=7")
	code, used, body := ingestStaged(t, s.URL, "7", stage(t, s.URL, "7", pack, idx))
	if code != 200 || used.Idx != "used" {
		t.Fatalf("idx path: %d %s", code, body)
	}
	call(t, s, "POST", "/init?net=8")
	code, indexed, body := ingestStaged(t, s.URL, "8", stage(t, s.URL, "8", pack, nil))
	if code != 200 || indexed.Idx != "none" {
		t.Fatalf("index-pack path: %d %s", code, body)
	}
	t.Logf("%d-byte pack: lock_ms %v with the stored idx, %v with index-pack (index_ms %v)", len(pack), used.LockMs,
		indexed.LockMs, indexed.IndexMs)
	if 4*indexed.LockMs >= indexed.IndexMs {
		t.Fatalf("index-pack path held the lock alone %v ms of its %v ms", indexed.LockMs, indexed.IndexMs)
	}
	for _, net := range []string{"7", "8"} {
		run(t, repo(net), "", "rev-list", "--objects", tip)
	}
}

func TestSpoolChecksumMismatchLeavesNothing(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	// A stored file a cut-off catch-up left two hours ago goes on the next call; a fresh one stays.
	os.MkdirAll(filepath.Join(data, "spool"), 0o755)
	stale, fresh := storedPath("7", strings.Repeat("1", 64), ".pack"), storedPath("7", strings.Repeat("2", 64), ".idx")
	os.WriteFile(stale, []byte("x"), 0o644)
	os.WriteFile(fresh, []byte("x"), 0o644)
	old := time.Now().Add(-2 * time.Hour)
	os.Chtimes(stale, old, old)

	code, body := postBody(t, s.URL+"/spool?net=7&sha256="+strings.Repeat("0", 64), []byte("PACK not really"))
	if code != 422 || !strings.HasPrefix(string(body), "checksum") {
		t.Fatalf("wrong sum: %d %s", code, body)
	}
	left, _ := filepath.Glob(filepath.Join(data, "spool", "*"))
	if len(left) != 1 || left[0] != fresh {
		t.Fatalf("spool holds %v, want only %s", left, fresh)
	}
}

// A ref whose commit's parent is gone: deleting the base's pack after the refs were set.
func TestConnectedFalseWhenHistoryMissing(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	base := history(t, wt, 3, 50, 1)
	tip := commit(t, wt, map[string]string{"new.txt": "new\n"})
	full := selfPack(t, wt, base)
	if code, body := postBody(t, s.URL+"/ingest?net=7&sha256="+sumOf(full), full); code != 200 {
		t.Fatalf("ingest base: %d %s", code, body)
	}
	basePacks, _ := filepath.Glob(filepath.Join(repo("7"), "objects", "pack", "pack-*.pack"))
	if len(basePacks) != 1 {
		t.Fatalf("packs %v", basePacks)
	}
	thin := packOfWt(t, wt, tip, base)
	if code, body := postBody(t, s.URL+"/ingest?net=7&sha256="+sumOf(thin), thin); code != 200 {
		t.Fatalf("ingest tip: %d %s", code, body)
	}
	putRefs(t, s.URL, tip)
	if v := connected(t, s.URL, ""); v["connected"] != true {
		t.Fatalf("whole: %v", v)
	}
	if v := connected(t, s.URL, "&objects=tips"); v["connected"] != true {
		t.Fatalf("whole, tips: %v", v)
	}
	stem := strings.TrimSuffix(basePacks[0], ".pack")
	for _, ext := range []string{".pack", ".idx", ".rev"} {
		os.Remove(stem + ext)
	}
	for _, q := range []string{"", "&objects=1", "&objects=tips"} {
		v := connected(t, s.URL, q)
		if v["connected"] != false || v["error"] == nil || v["refs"] != 1.0 {
			t.Fatalf("connected%s with the base gone: %v", q, v)
		}
	}
}

// /idx answers the index /validate wrote for a self-contained pack, which checks against that pack's own bytes: the
// index a promotion stores beside its pack.
func TestIdxOfValidatedPack(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	base := history(t, wt, 3, 50, 1)
	run(t, wt, "", "push", "-q", repo("7"), "main")
	tip := commit(t, wt, map[string]string{"new.txt": "new\n"})
	pack, err := gitOut(wt, tip+"\n^"+base+"\n", "pack-objects", "--stdout", "--revs", "-q")
	if err != nil {
		t.Fatal(err)
	}
	code, body := postBody(t, s.URL+"/validate?net=7&derived=1&tips="+tip, pack)
	if code != 200 {
		t.Fatalf("validate: %d %s", code, body)
	}
	sum := sumOf(pack)
	r, err := http.Get(s.URL + "/idx?net=7&sha256=" + sum)
	if err != nil {
		t.Fatal(err)
	}
	idx, _ := io.ReadAll(r.Body)
	r.Body.Close()
	if r.StatusCode != 200 || r.Header.Get("content-type") != "application/octet-stream" {
		t.Fatalf("idx: %d %s", r.StatusCode, idx)
	}
	kept, _ := filepath.Glob(filepath.Join(repo("7"), "graft", "validated-"+sum, "pack", "pack-*.idx"))
	want, _ := os.ReadFile(kept[0])
	if !bytes.Equal(idx, want) {
		t.Fatal("idx is not the validated pack's index")
	}
	if why := checkIdx(idx, pack[:12], pack[len(pack)-20:]); why != "" {
		t.Fatalf("the validated idx fails its own pack: %s", why)
	}
	r, err = http.Get(s.URL + "/idx?net=7&sha256=" + strings.Repeat("0", 64))
	if err != nil {
		t.Fatal(err)
	}
	r.Body.Close()
	if r.StatusCode != 404 {
		t.Fatalf("unknown sum: %d", r.StatusCode)
	}
}

func TestCheckIdx(t *testing.T) {
	wt := t.TempDir()
	tip := history(t, wt, 3, 50, 3)
	pack := selfPack(t, wt, tip)
	idx := idxFor(t, pack)
	head, trailer := pack[:12], pack[len(pack)-20:]
	if why := checkIdx(idx, head, trailer); why != "" {
		t.Fatalf("good idx: %s", why)
	}
	for name, bad := range map[string][]byte{
		"truncated":  idx[:len(idx)-8],
		"short":      idx[:100],
		"bad magic":  append([]byte{0}, idx[1:]...),
		"other pack": idxFor(t, selfPack(t, wt, tip+"~1")),
	} {
		if why := checkIdx(bad, head, trailer); why == "" {
			t.Fatalf("%s idx passed", name)
		} else {
			t.Logf("%s: %s", name, why)
		}
	}
	if why := checkIdx(append([]byte{0}, idx[1:]...), head, trailer); !strings.Contains(why, "magic") {
		t.Fatalf("bad magic: %s", why)
	}
	// The object count is the pack header's.
	h := append([]byte(nil), head...)
	h[11]++
	if why := checkIdx(idx, h, trailer); !strings.Contains(why, "objects") {
		t.Fatalf("count mismatch: %q", why)
	}
}

// A spooled promotion is kept before git can see it. The geometric repack takes no network lock, so one that lists the
// packs as the promotion's index is renamed in must already find its `.keep`: otherwise it rolls the promotion with the
// other unkept packs, and the pack /roll and /promote name by `graft/<sum>.name` is gone.
func TestSpooledPromotionIsKeptBeforeItIsVisible(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	files := func(tag string) map[string]string {
		set := map[string]string{}
		for i := range 10 {
			set[fmt.Sprintf("f%d.txt", i)] = fmt.Sprintf("%s %d\n", tag, i)
		}
		return set
	}
	// An unkept pack already on disk, as large as the promotion, so a geometric repack rolls the two together.
	wa := t.TempDir()
	run(t, wa, "", "init", "-q", "-b", "main")
	run(t, repo("7"), string(selfPack(t, wa, commit(t, wa, files("a")))), "index-pack", "--stdin")
	wb := t.TempDir()
	run(t, wb, "", "init", "-q", "-b", "main")
	tip := commit(t, wb, files("b"))
	pack := selfPack(t, wb, tip)
	sum := stage(t, s.URL, "7", pack, idxFor(t, pack))
	ran := false
	old := packVisible
	packVisible = func(rp string) {
		ran = true
		if err := runRepack(context.Background(), rp); err != nil {
			t.Errorf("repack: %v", err)
		}
	}
	defer func() { packVisible = old }()
	code, a, body := ingestStaged(t, s.URL, "7", sum)
	if !ran {
		t.Fatalf("the hook never ran: %d %s", code, body)
	}
	if code != 200 || a.Idx != "used" {
		t.Fatalf("ingest: %d %s", code, body)
	}
	dst := filepath.Join(repo("7"), "objects", "pack", "pack-"+hex.EncodeToString(pack[len(pack)-20:]))
	for _, ext := range []string{".pack", ".idx", ".keep"} {
		if _, err := os.Stat(dst + ext); err != nil {
			t.Fatalf("the promotion's %s is gone after a repack ran as it became visible: %v", ext, err)
		}
	}
	run(t, repo("7"), "", "cat-file", "-e", tip)
}
