package main

import (
	"encoding/json"
	"net/http"
	"os"
	"strings"
	"testing"
)

// GitCacheShard::merge for real merges and queue groups: each head on the previous result, by merge, squash, and
// rebase, deterministic, with a conflict reported at its index and the entries before it kept.

type chainOut struct {
	Steps    []chainStep    `json:"steps"`
	Conflict map[string]any `json:"conflict"`
	Pack     []byte         `json:"pack"`
	Engine   string         `json:"engine"`
}

func chain(t *testing.T, url, base, method string, heads ...string) (int, chainOut, string) {
	t.Helper()
	ns := make([]string, len(heads))
	for i := range heads {
		ns[i] = string(rune('1' + i))
	}
	r, err := http.Post(url+"/merge?net=7&base="+base+"&method="+method+"&heads="+strings.Join(heads, ",")+
		"&ns="+strings.Join(ns, ","), "", nil)
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	var b strings.Builder
	var buf [4096]byte
	for {
		n, err := r.Body.Read(buf[:])
		b.Write(buf[:n])
		if err != nil {
			break
		}
	}
	var o chainOut
	json.Unmarshal([]byte(b.String()), &o)
	return r.StatusCode, o, b.String()
}

func TestMergeChain(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commit(t, wt, map[string]string{"f": "a\nb\nc\n"})
	var heads []string
	for i, p := range []string{"one", "two", "three"} {
		run(t, wt, "", "checkout", "-qb", p, base)
		commit(t, wt, map[string]string{p: p + "\n"})
		heads = append(heads, commit(t, wt, map[string]string{p: p + " again\n", "x" + string(rune('0'+i)): "x\n"}))
	}
	run(t, wt, "", "checkout", "-qb", "clash", base)
	clash := commit(t, wt, map[string]string{"two": "other\n"})
	run(t, wt, "", "push", "-q", repo("7"), "main", "one", "two", "three", "clash")

	for _, m := range []string{"merge", "squash", "rebase"} {
		code, o, body := chain(t, s.URL, base, m, heads...)
		if code != 200 || len(o.Steps) != 3 || o.Conflict != nil || len(o.Pack) == 0 {
			t.Fatalf("%s: %d %s", m, code, body)
		}
		// The group pack indexes against the repository and has every result.
		idx := t.TempDir()
		run(t, idx, "", "init", "-q", "--bare")
		run(t, idx, "", "config", "core.alternateRefsCommand", "true")
		writeAlt(t, idx)
		run(t, idx, string(o.Pack), "index-pack", "--stdin", "--strict")
		last := o.Steps[2].Commit
		tree := run(t, idx, "", "ls-tree", "--name-only", last)
		for _, f := range []string{"f", "one", "two", "three", "x0", "x1", "x2"} {
			if !strings.Contains("\n"+tree+"\n", "\n"+f+"\n") {
				t.Fatalf("%s: the group tip lacks %s: %s", m, f, tree)
			}
		}
		count := run(t, idx, "", "rev-list", "--count", last, "^"+base)
		want := map[string]string{"merge": "9", "squash": "3", "rebase": "6"}[m]
		if count != want {
			t.Fatalf("%s: %s commits over base, want %s", m, count, want)
		}
		if first := run(t, idx, "", "rev-list", "--first-parent", "--count", last, "^"+base); m == "merge" && first != "3" {
			t.Fatalf("merge: first-parent chain %s", first)
		}
		if _, again, _ := chain(t, s.URL, base, m, heads...); again.Steps[2].Commit != last {
			t.Fatalf("%s is not deterministic: %s then %s", m, last, again.Steps[2].Commit)
		}
		if o.Steps[0].UpToDate != true || o.Steps[1].UpToDate {
			t.Fatalf("%s: upToDate %v %v", m, o.Steps[0].UpToDate, o.Steps[1].UpToDate)
		}
	}

	code, o, body := chain(t, s.URL, base, "merge", heads[0], clash, heads[1])
	if code != 200 || len(o.Steps) != 2 || o.Conflict == nil || o.Conflict["index"] != float64(2) {
		t.Fatalf("conflict: %d %s", code, body)
	}
	if code, _, _ := chain(t, s.URL, base, "octopus", heads[0]); code != 422 {
		t.Fatalf("an unknown method answered %d", code)
	}
}

func writeAlt(t *testing.T, idx string) {
	t.Helper()
	run(t, idx, "", "config", "core.repositoryformatversion", "0")
	if err := writeFile(idx+"/objects/info/alternates", repo("7")+"/objects\n"); err != nil {
		t.Fatal(err)
	}
}

func writeFile(p, s string) error {
	return os.WriteFile(p, []byte(s), 0o644)
}
