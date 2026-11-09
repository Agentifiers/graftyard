package main

import (
	"encoding/json"
	"net/http"
	"strings"
	"testing"
)

func refsCall(t *testing.T, url, method, query, body string) map[string]any {
	t.Helper()
	req, _ := http.NewRequest(method, url+"/refs?net=7"+query, strings.NewReader(body))
	r, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	if r.StatusCode != 200 {
		t.Fatalf("%s /refs%s: %d", method, query, r.StatusCode)
	}
	var v map[string]any
	json.NewDecoder(r.Body).Decode(&v)
	return v
}

// A rehydrate's ref set in pages (PUT start=1 .. done=1) ends as the whole set, deleting refs no page lists; a
// catch-up's changes (POST) delete and set; and a ref naming an object the shard does not hold is skipped and counted,
// so the other refs still apply (repo:65537's made-up refs failed every catch-up whole, tests/m3/results/m3-wp5.md).
func TestRefsPagedAndChanges(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	a := commit(t, wt, map[string]string{"f": "a\n"})
	b := commit(t, wt, map[string]string{"f": "b\n"})
	run(t, wt, "", "push", "-q", repo("7"), "main", "main:refs/heads/stale")
	missing := strings.Repeat("1", 40)

	// Two pages: the first clears refs/heads/stale (not listed), the second holds a ref to a missing object.
	v := refsCall(t, s.URL, "PUT", "&start=1", a+" refs/heads/a\n"+b+" refs/heads/main\n")
	if v["skipped"].(float64) != 0 {
		t.Fatalf("page 1 skipped %v", v)
	}
	v = refsCall(t, s.URL, "PUT", "&done=1", b+" refs/heads/z\n"+missing+" refs/heads/fake\n")
	if v["skipped"].(float64) != 1 {
		t.Fatalf("page 2: %v", v)
	}
	got := map[string]string{}
	for _, l := range strings.Split(run(t, repo("7"), "", "for-each-ref", "--format=%(objectname) %(refname)"), "\n") {
		if f := strings.Fields(l); len(f) == 2 {
			got[f[1]] = f[0]
		}
	}
	want := map[string]string{"refs/heads/a": a, "refs/heads/main": b, "refs/heads/z": b}
	if len(got) != len(want) {
		t.Fatalf("refs %v, want %v", got, want)
	}
	for k, o := range want {
		if got[k] != o {
			t.Fatalf("refs %v, want %v", got, want)
		}
	}
	if h := run(t, repo("7"), "", "symbolic-ref", "HEAD"); h != "refs/heads/main" {
		t.Fatalf("HEAD %s", h)
	}

	// A catch-up's changes: a delete (the zero oid), a move, and a new ref under a deleted name's directory.
	zero := strings.Repeat("0", 40)
	v = refsCall(t, s.URL, "POST", "", zero+" refs/heads/z\n"+a+" refs/heads/main\n"+b+" refs/heads/z/new\n")
	if v["skipped"].(float64) != 0 {
		t.Fatalf("changes: %v", v)
	}
	if out := run(t, repo("7"), "", "for-each-ref", "--format=%(refname) %(objectname)", "refs/heads/z", "refs/heads/z/new", "refs/heads/main"); out != "refs/heads/main "+a+"\nrefs/heads/z/new "+b {
		t.Fatalf("after changes: %q", out)
	}
	// The zero oid is a delete only in a change set, never in a PUT of the whole set.
	req, _ := http.NewRequest("PUT", s.URL+"/refs?net=7", strings.NewReader(zero+" refs/heads/a\n"))
	r, _ := http.DefaultClient.Do(req)
	r.Body.Close()
	if r.StatusCode != 400 {
		t.Fatalf("PUT of the zero oid: %d", r.StatusCode)
	}
}
