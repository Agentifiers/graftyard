package main

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// The container runs at most stockMax stock fetches at once and answers the next 429 at once, with x-graft-stock:
// full, so the shard object can hold it in line instead of the lock (B0-34 m1b34: up to 15 to 21 stock upload-packs on
// one container, 2.6 GB each, queued a catch-up 29 to 36 s and every validate behind it). A composed answer, an
// ls-refs and an advertisement take no slot, and a slot frees when its fetch ends.
func TestStockFetchesPastStockMaxAreRefusedAtOnce(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	tip := history(t, wt, 10, 20, 2)
	pushTip(t, wt)
	up := s.URL + "/upload-pack?net=7"

	// upload-pack waits for `release` before it reads; any other git runs at once.
	real, err := exec.LookPath("git")
	if err != nil {
		t.Fatal(err)
	}
	bin, gate := t.TempDir(), t.TempDir()
	release := filepath.Join(gate, "release")
	wrap := "#!/bin/sh\n" +
		"for a in \"$@\"; do if [ \"$a\" = upload-pack ]; then touch " + gate + "/held.$$; " +
		"while [ ! -e " + release + " ]; do sleep 0.05; done; fi; done\n" +
		"exec " + real + " \"$@\"\n"
	if err := os.WriteFile(filepath.Join(bin, "git"), []byte(wrap), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Cleanup(func() { os.WriteFile(release, nil, 0o644) })
	client := &http.Client{Timeout: 5 * time.Second}

	post := func(body string) (int, string, []byte) {
		req, _ := http.NewRequest("POST", up, strings.NewReader(body))
		req.Header.Set("git-protocol", "version=2")
		r, err := client.Do(req)
		if err != nil {
			return -1, err.Error(), nil
		}
		defer r.Body.Close()
		b, _ := io.ReadAll(r.Body)
		return r.StatusCode, r.Header.Get("x-graft-stock"), b
	}
	// A filter makes the fetch not composable: stock, as a clone a chain cannot answer is.
	stock := fetchBody(tip, "filter blob:none")
	held := make(chan int, int(stockMax))
	for i := int64(0); i < stockMax; i++ {
		go func() { c, _, _ := post(stock); held <- c }()
	}
	for i := 0; ; i++ {
		if n, _ := filepath.Glob(filepath.Join(gate, "held.*")); int64(len(n)) == stockMax {
			break
		}
		if i > 400 {
			t.Fatalf("the first %d stock fetches never reached upload-pack", stockMax)
		}
		time.Sleep(10 * time.Millisecond)
	}

	// Both slots held: the next stock fetch is refused at once, not queued on the network's lock.
	t0 := time.Now()
	code, mark, _ := post(stock)
	if code != 429 || mark != "full" {
		t.Fatalf("stock fetch past stockMax: %d %q", code, mark)
	}
	if d := time.Since(t0); d > time.Second {
		t.Fatalf("the refusal took %v", d)
	}
	// A v2 ls-refs takes no slot; with the wrapper it holds too, so only its start is checked: it reaches git.
	go post(pkt("command=ls-refs\n") + "0001" + pkt("ref-prefix refs/heads/\n") + "0000")
	for i := 0; ; i++ {
		if n, _ := filepath.Glob(filepath.Join(gate, "held.*")); int64(len(n)) > stockMax {
			break
		}
		if i > 400 {
			t.Fatal("an ls-refs was refused or queued while the stock slots were full")
		}
		time.Sleep(10 * time.Millisecond)
	}

	// Released: both held fetches finish, and a slot is free again.
	os.WriteFile(release, nil, 0o644)
	for i := int64(0); i < stockMax; i++ {
		if c := <-held; c != 200 {
			t.Fatalf("a held stock fetch answered %d", c)
		}
	}
	if code, _, b := post(stock); code != 200 || len(b) < 32 {
		t.Fatalf("after the slots freed: %d, %d bytes", code, len(b))
	}
	if n := stockRunning.Load(); n != 0 {
		t.Fatalf("%d slots still counted with nothing running", n)
	}
}

// A chain step whose chain tree is past composeDiffMax entries from the new tip builds a base there, not a part: on
// m1b34 a base built on a one-file row push's tip took 88,000-entry parts at every move, and every miss after it went
// over the cap to stock. After the step, the want that was over the cap composes.
func TestComposeStepOnAFarChainBuildsABase(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	history(t, wt, 40, 20, 2)
	// The chain's base on a lone one-file commit, as HEAD on the first ref left it.
	run(t, wt, "", "checkout", "-q", "--orphan", "row")
	run(t, wt, "", "rm", "-rqf", ".")
	lone := commit(t, wt, map[string]string{"a1h/0.txt": "x\n"})
	run(t, wt, "", "push", "-q", repo("7"), lone+":refs/heads/a1h-0")
	run(t, wt, "", "checkout", "-q", "main")
	tip := pushTip(t, wt)
	if o := compose(t, s.URL, "&start=1&tip="+lone); o["head"].(map[string]any)["base"] != lone {
		t.Fatalf("base %v", o["head"])
	}
	defer func(v int) { composeDiffMax = v }(composeDiffMax)
	composeDiffMax = 20 // the tip's 40 files and 5 trees against a one-file base; a move of one file is 3 entries
	up := s.URL + "/upload-pack?net=7"
	if r, _ := uploadV2(t, up, fetchBody(tip)); r.Header.Get("x-graft-compose") != "stock: over the diff cap" {
		t.Fatalf("the tip against the lone base: %q", r.Header.Get("x-graft-compose"))
	}
	h := compose(t, s.URL, "")["head"].(map[string]any)
	if h["base"] != tip || len(h["parts"].([]any)) != 0 {
		t.Fatalf("the step on a far chain built %v, want a base at %s", h, tip)
	}
	r, b := uploadV2(t, up, fetchBody(tip))
	if c := r.Header.Get("x-graft-compose"); c != "composed" {
		t.Fatalf("after the step: %q", c)
	}
	cloneInto(t, b, tip, run(t, wt, "", "rev-parse", "HEAD^{tree}"))
	// A near move stays a part.
	next := commit(t, wt, map[string]string{"d0/f0.txt": "moved\n"})
	pushTip(t, wt)
	h = compose(t, s.URL, "")["head"].(map[string]any)
	if h["base"] != tip || len(h["parts"].([]any)) != 1 || h["parts"].([]any)[0] != next {
		t.Fatalf("a near move: %v", h)
	}
}

// /ingest and /ingest-batch answer midx_ms, the background multi-pack-index writes that ended since the network's last
// answer: 0 until the midxEvery-th ingest's write has ended, then its time once, on the next answer, and 0 again.
// m1b34's 35 s exclusive sections on site b could not be pinned on the midx write without it, and mt7's
// /ingest-batch catch-ups reported none (tests/am1/results/b034-mt7.md).
func TestIngestReportsTheMidxWrite(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	r := newMidxRepo(t, s.URL)
	midx := func(code int, body []byte, what string) float64 {
		t.Helper()
		var o struct {
			Midx *float64 `json:"midx_ms"`
		}
		json.Unmarshal(body, &o)
		if code != 200 || o.Midx == nil {
			t.Fatalf("%s answered no midx_ms: %d %s", what, code, body)
		}
		return *o.Midx
	}
	ingest := func(i int) float64 {
		_, pack := r.next()
		code, body := postBody(t, s.URL+"/ingest?net=7&sha256="+sumOf(pack), pack)
		return midx(code, body, fmt.Sprintf("ingest %d", i))
	}
	for i := 2; i <= midxEvery; i++ {
		if ms := ingest(i); ms != 0 {
			t.Fatalf("ingest %d: midx_ms %v before any write ended", i, ms)
		}
	}
	midxWait(repo("7"))
	if ms := ingest(midxEvery + 1); ms <= 0 {
		t.Fatalf("the ingest after the write ended: midx_ms %v, want its time", ms)
	}
	if ms := ingest(midxEvery + 2); ms != 0 {
		t.Fatalf("the next ingest: midx_ms %v, want 0 (the write reported once)", ms)
	}
	// /ingest-batch the same way.
	midxOf(repo("7")).n = midxEvery - 1
	batch := func() float64 {
		_, pack := r.next()
		body, _ := batchBody(pack)
		code, out := postBody(t, s.URL+"/ingest-batch?net=7", body)
		return midx(code, out, "ingest-batch")
	}
	if ms := batch(); ms != 0 {
		t.Fatalf("the batch that started a write: midx_ms %v", ms)
	}
	midxWait(repo("7"))
	if ms := batch(); ms <= 0 {
		t.Fatalf("the batch after the write ended: midx_ms %v, want its time", ms)
	}
}
