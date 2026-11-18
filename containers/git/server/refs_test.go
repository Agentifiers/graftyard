package main

import (
	"encoding/json"
	"io"
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
	// A malformed head= is refused before anything changes.
	req, _ := http.NewRequest("PUT", s.URL+"/refs?net=7&done=1&head=HEAD", strings.NewReader(a+" refs/heads/a\n"))
	if r, _ := http.DefaultClient.Do(req); r.StatusCode != 400 {
		t.Fatalf("PUT with head=HEAD: %d", r.StatusCode)
	}
	// The zero oid is a delete only in a change set, never in a PUT of the whole set.
	req, _ = http.NewRequest("PUT", s.URL+"/refs?net=7", strings.NewReader(zero+" refs/heads/a\n"))
	r, _ := http.DefaultClient.Do(req)
	r.Body.Close()
	if r.StatusCode != 400 {
		t.Fatalf("PUT of the zero oid: %d", r.StatusCode)
	}
}

// G26: HEAD is the repository's default branch, which the shard object names (head=), not refs/heads/main because it
// exists, both in the v2 ls-refs and the v0 advertisement a client that does not opt in reads. A later POST names it
// again, so a default branch changed in a Settings entry moves HEAD on the next catch-up; one not created yet is an
// unborn HEAD, as GHES answers for it.
func TestRefsHeadIsTheDefaultBranch(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	a := commit(t, wt, map[string]string{"f": "a\n"})
	b := commit(t, wt, map[string]string{"f": "b\n"})
	run(t, wt, "", "push", "-q", repo("7"), a+":refs/heads/dev", b+":refs/heads/main")
	head := func() string { return run(t, repo("7"), "", "symbolic-ref", "HEAD") }
	lsRefs := func() string {
		req, _ := http.NewRequest("POST", s.URL+"/upload-pack?net=7",
			strings.NewReader(pkt("command=ls-refs\n")+"0001"+pkt("symrefs\n")+pkt("unborn\n")+"0000"))
		req.Header.Set("git-protocol", "version=2")
		r, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		defer r.Body.Close()
		b, _ := io.ReadAll(r.Body)
		return string(b)
	}
	advertised := func() string {
		r, err := http.Get(s.URL + "/info-refs?net=7")
		if err != nil {
			t.Fatal(err)
		}
		defer r.Body.Close()
		b, _ := io.ReadAll(r.Body)
		return string(b)
	}

	// Both branches exist, main sorts after dev, and the default is dev.
	refsCall(t, s.URL, "PUT", "&start=1&done=1&head=refs/heads/dev", a+" refs/heads/dev\n"+b+" refs/heads/main\n")
	if h := head(); h != "refs/heads/dev" {
		t.Fatalf("HEAD %s after a PUT naming dev", h)
	}
	if ls := lsRefs(); !strings.Contains(ls, a+" HEAD symref-target:refs/heads/dev\n") {
		t.Fatalf("v2 ls-refs: %q", ls)
	}
	if adv := advertised(); !strings.Contains(adv, a+" HEAD\x00") || !strings.Contains(adv, "symref=HEAD:refs/heads/dev") {
		t.Fatalf("v0 advertisement: %q", adv)
	}
	// A catch-up over a Settings entry that changed the default to main: no ref changes, only head=.
	refsCall(t, s.URL, "POST", "&head=refs/heads/main", "")
	if h := head(); h != "refs/heads/main" {
		t.Fatalf("HEAD %s after an empty POST naming main", h)
	}
	// And with ref changes beside it.
	refsCall(t, s.URL, "POST", "&head=refs/heads/dev", b+" refs/heads/dev\n")
	if h := head(); h != "refs/heads/dev" {
		t.Fatalf("HEAD %s after a POST naming dev", h)
	}
	// A default branch not created yet: HEAD is unborn there, not moved to a branch that exists.
	refsCall(t, s.URL, "PUT", "&start=1&done=1&head=refs/heads/trunk", a+" refs/heads/dev\n"+b+" refs/heads/main\n")
	if h := head(); h != "refs/heads/trunk" {
		t.Fatalf("HEAD %s after a PUT naming trunk", h)
	}
	if ls := lsRefs(); !strings.Contains(ls, "unborn HEAD symref-target:refs/heads/trunk\n") {
		t.Fatalf("v2 ls-refs with an unborn default: %q", ls)
	}
	// No head=: a POST leaves HEAD alone, and a PUT keeps the old rule (an object that predates G26).
	refsCall(t, s.URL, "POST", "", a+" refs/heads/main\n")
	if h := head(); h != "refs/heads/trunk" {
		t.Fatalf("HEAD %s after a POST naming none", h)
	}
	refsCall(t, s.URL, "PUT", "&start=1&done=1", a+" refs/heads/dev\n"+b+" refs/heads/main\n")
	if h := head(); h != "refs/heads/main" {
		t.Fatalf("HEAD %s after a PUT naming none", h)
	}
}
