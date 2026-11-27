package main

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"
)

// Forks in one network's repository: a fork's refs live under refs/namespaces/<fork id>/, a refns= request serves
// that fork's view with names stripped, and a root request never lists a fork's ref and answers byte for byte as it
// did before any namespace existed.

// smartHTTPQuery is smartHTTP with extra query parameters on every forwarded call (refns=).
func smartHTTPQuery(t *testing.T, s *httptest.Server, net, extra string) string {
	t.Helper()
	h := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		path, method := "/upload-pack", http.MethodPost
		if strings.HasSuffix(r.URL.Path, "/info/refs") {
			path, method = "/info-refs", http.MethodGet
		}
		req, _ := http.NewRequest(method, s.URL+path+"?net="+net+extra, r.Body)
		req.Header.Set("git-protocol", r.Header.Get("git-protocol"))
		req.Header.Set("content-encoding", r.Header.Get("content-encoding"))
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			http.Error(w, err.Error(), 502)
			return
		}
		defer resp.Body.Close()
		w.Header().Set("content-type", resp.Header.Get("content-type"))
		w.WriteHeader(resp.StatusCode)
		io.Copy(w, resp.Body)
	}))
	t.Cleanup(h.Close)
	return h.URL + "/o/r.git"
}

// nsDo answers a request's status, body, and headers.
func nsDo(t *testing.T, method, url, proto, body string) (int, string, http.Header) {
	t.Helper()
	req, _ := http.NewRequest(method, url, strings.NewReader(body))
	if proto != "" {
		req.Header.Set("git-protocol", proto)
	}
	r, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	b, _ := io.ReadAll(r.Body)
	return r.StatusCode, string(b), r.Header
}

func TestForkNamespaces(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	root := commit(t, wt, map[string]string{"f": "root\n"})
	fork := commit(t, wt, map[string]string{"f": "fork\n"})
	// The objects of both, then the root's refs alone: main and a tag, HEAD at main.
	run(t, wt, "", "push", "-q", repo("7"), fork+":refs/heads/objects")
	if v := refsCall(t, s.URL, "PUT", "&head=refs/heads/main", root+" refs/heads/main\n"+root+" refs/tags/v1\n"); v["skipped"].(float64) != 0 {
		t.Fatalf("root PUT: %v", v)
	}
	lsRefsBody := pkt("command=ls-refs\n") + "0001" + pkt("peel\n") + pkt("symrefs\n") + pkt("unborn\n") + "0000"
	forksNamed := false
	rootAnswers := func() []string {
		t.Helper()
		var out []string
		for _, c := range []struct{ method, path, proto, body string }{
			{"GET", "/info-refs?net=7", "", ""},
			{"GET", "/info-refs?net=7", "version=2", ""},
			{"POST", "/upload-pack?net=7", "version=2", lsRefsBody},
		} {
			// The shard names the network's forks on a root call once it has one (`forks=`), never before.
			if forksNamed {
				c.path += "&forks=42,43"
			}
			code, b, _ := nsDo(t, c.method, s.URL+c.path, c.proto, c.body)
			if code != 200 {
				t.Fatalf("%s %s: %d %s", c.method, c.path, code, b)
			}
			out = append(out, b)
		}
		return out
	}
	before := rootAnswers()

	// Fork 42: its default branch dev, at a commit no root ref names, and a branch named as the root's (main) at
	// another tip, written by the shard object through /refs like any ref, its HEAD by a namespaced head=.
	if v := refsCall(t, s.URL, "POST", "&head=refs/namespaces/42/refs/heads/dev",
		fork+" refs/namespaces/42/refs/heads/dev\n"+fork+" refs/namespaces/42/refs/heads/main\n"); v["skipped"].(float64) != 0 {
		t.Fatalf("fork POST: %v", v)
	}
	if h := run(t, repo("7"), "", "symbolic-ref", "refs/namespaces/42/HEAD"); h != "refs/namespaces/42/refs/heads/dev" {
		t.Fatalf("namespace HEAD %s", h)
	}
	if h := run(t, repo("7"), "", "symbolic-ref", "HEAD"); h != "refs/heads/main" {
		t.Fatalf("root HEAD moved: %s", h)
	}
	// A POST with only a namespaced head= moves that HEAD alone.
	refsCall(t, s.URL, "POST", "&head=refs/namespaces/42/refs/heads/main", "")
	if h := run(t, repo("7"), "", "symbolic-ref", "refs/namespaces/42/HEAD"); h != "refs/namespaces/42/refs/heads/main" {
		t.Fatalf("namespace HEAD after a HEAD-only POST: %s", h)
	}
	if h := run(t, repo("7"), "", "symbolic-ref", "HEAD"); h != "refs/heads/main" {
		t.Fatalf("root HEAD moved by a HEAD-only POST: %s", h)
	}
	refsCall(t, s.URL, "POST", "&head=refs/namespaces/42/refs/heads/dev", "")
	// A root PUT of its own set keeps the fork's namespace HEAD and default branch.
	refsCall(t, s.URL, "PUT", "&head=refs/heads/main", root+" refs/heads/main\n"+root+" refs/tags/v1\n"+
		fork+" refs/namespaces/42/refs/heads/dev\n"+fork+" refs/namespaces/42/refs/heads/main\n")
	if got := run(t, repo("7"), "", "rev-parse", "refs/namespaces/42/HEAD"); got != fork {
		t.Fatalf("fork HEAD after a PUT: %s", got)
	}

	// The root: no refs/namespaces/ ref anywhere, and the same bytes as before the fork existed.
	forksNamed = true
	after := rootAnswers()
	for i := range before {
		if strings.Contains(after[i], "refs/namespaces/") || strings.Contains(after[i], fork) {
			t.Fatalf("root answer %d lists a fork ref:\n%q", i, after[i])
		}
		if after[i] != before[i] {
			t.Fatalf("root answer %d changed:\nbefore %q\nafter  %q", i, before[i], after[i])
		}
	}

	// The fork's view: only its branches and HEAD, names stripped. git 2.56's ls-refs under GIT_NAMESPACE sends the
	// namespace HEAD twice, once as HEAD and once as the stripped refs/namespaces/42/HEAD, so lines are compared
	// once each.
	code, ls, _ := nsDo(t, "POST", s.URL+"/upload-pack?net=7&refns=42", "version=2", lsRefsBody)
	var lines []string
	for _, l := range strings.SplitAfter(ls, "\n") {
		if len(lines) == 0 || lines[len(lines)-1] != l {
			lines = append(lines, l)
		}
	}
	want := pkt(fork+" HEAD symref-target:refs/heads/dev\n") + pkt(fork+" refs/heads/dev\n") +
		pkt(fork+" refs/heads/main\n") + "0000"
	if got := strings.Join(lines, ""); code != 200 || got != want {
		t.Fatalf("refns ls-refs: %d\n%q\nwant %q", code, ls, want)
	}
	code, adv, _ := nsDo(t, "GET", s.URL+"/info-refs?net=7&refns=42", "", "")
	if code != 200 || strings.Contains(adv, "refs/namespaces/") || strings.Contains(adv, root) ||
		strings.Contains(adv, "refs/tags/v1") || !strings.Contains(adv, fork+" HEAD\x00") ||
		!strings.Contains(adv, "symref=HEAD:refs/heads/dev") || !strings.Contains(adv, fork+" refs/heads/dev\n") {
		t.Fatalf("refns v0 advertisement: %d %q", code, adv)
	}
	// Another fork, with no refs yet, sees nothing of 42's or the root's.
	if _, ls, _ := nsDo(t, "POST", s.URL+"/upload-pack?net=7&refns=43", "version=2", lsRefsBody); strings.Contains(ls, fork) || strings.Contains(ls, root) {
		t.Fatalf("refns=43 ls-refs: %q", ls)
	}

	// A clone through refns checks out the fork's default branch at the fork's tip, over v0 and v2.
	url := smartHTTPQuery(t, s, "7", "&refns=42")
	for _, proto := range []string{"0", "2"} {
		dir := t.TempDir()
		run(t, dir, "", "-c", "protocol.version="+proto, "clone", "-q", url, "c")
		if b := run(t, dir+"/c", "", "symbolic-ref", "HEAD"); b != "refs/heads/dev" {
			t.Fatalf("v%s clone checked out %s", proto, b)
		}
		if got := run(t, dir+"/c", "", "rev-parse", "HEAD"); got != fork {
			t.Fatalf("v%s clone at %s, want %s", proto, got, fork)
		}
		if refs := run(t, dir+"/c", "", "for-each-ref", "--format=%(refname)"); strings.Contains(refs, "namespaces") {
			t.Fatalf("v%s clone refs: %s", proto, refs)
		}
	}

	// compose=only on a fork is never composed.
	code, _, hdr := nsDo(t, "POST", s.URL+"/upload-pack?net=7&refns=42&compose=only&tail=1", "version=2",
		pkt("command=fetch\n")+"0001"+pkt("want "+fork+"\n")+pkt("done\n")+"0000")
	if code != 409 || hdr.Get("x-graft-compose") != "stock: fork" {
		t.Fatalf("refns compose=only: %d %q", code, hdr.Get("x-graft-compose"))
	}
	// Nor a root's beside forks: the chain's tip check would take a fork's tip for the root's.
	code, _, hdr = nsDo(t, "POST", s.URL+"/upload-pack?net=7&forks=42&compose=only&tail=1", "version=2",
		pkt("command=fetch\n")+"0001"+pkt("want "+fork+"\n")+pkt("done\n")+"0000")
	if code != 409 || hdr.Get("x-graft-compose") != "stock: fork" {
		t.Fatalf("root beside forks compose=only: %d %q", code, hdr.Get("x-graft-compose"))
	}

	// A push to the fork's main over the fork's main tip: valid against the fork's view, behind against the root's,
	// whose main is at another commit.
	next := commit(t, wt, map[string]string{"f": "next\n"})
	pack := packOfWt(t, wt, next, fork)
	validate := func(extra string) (int, string) {
		head := next + "\n"
		detail := next + " " + fork + " refs/heads/main\n"
		code, b, _ := nsDo(t, "POST", s.URL+"/validate?net=7&disk=1"+extra+"&tipsn="+strconv.Itoa(len(head))+"&detailn="+strconv.Itoa(len(detail)),
			"", head+detail+string(pack))
		return code, b
	}
	if code, b := validate("&refns=42"); code != 200 || !strings.Contains(b, `"detail"`) || !strings.Contains(b, `"ff":true`) {
		t.Fatalf("refns validate: %d %s", code, b)
	}
	if code, b := validate(""); code != 409 || !strings.Contains(b, "behind") {
		t.Fatalf("root validate of the fork's old tip: %d %s", code, b)
	}

	// A root push of a commit only the fork's refs reach (a fork pull request's merge), with the pack a client
	// sends against the root's advertisement: its new commits are the root's to report, so a root snapshot that
	// names the forks leaves their refs out of what it excludes.
	z := strings.Repeat("0", 40)
	fromFork := packOfWt(t, wt, fork, root)
	for _, c := range []struct {
		extra string
		want  int
	}{{"&forks=42", 1}, {"", 0}} {
		head, detail := fork+"\n", fork+" "+z+" refs/heads/from-fork\n"
		code, b, _ := nsDo(t, "POST", s.URL+"/validate?net=7&disk=1"+c.extra+"&tipsn="+strconv.Itoa(len(head))+
			"&detailn="+strconv.Itoa(len(detail)), "", head+detail+string(fromFork))
		if code != 200 || strings.Count(b, `"oid":"`+fork+`"`) != c.want {
			t.Fatalf("root validate of a fork's commit%s: %d %s", c.extra, code, b)
		}
	}

	// A root ref of its own under refs/namespaces/, in a namespace no fork holds, is listed by the root as before.
	refsCall(t, s.URL, "POST", "", root+" refs/namespaces/9/refs/heads/x\n")
	if _, ls, _ := nsDo(t, "POST", s.URL+"/upload-pack?net=7&forks=42,43", "version=2", lsRefsBody); !strings.Contains(ls, root+" refs/namespaces/9/refs/heads/x\n") {
		t.Fatalf("root ls-refs hid its own refs/namespaces/ ref: %q", ls)
	}

	// A bad refns is 400 on every endpoint that takes one, and a malformed namespaced head= on /refs.
	for _, p := range []string{"/info-refs?net=7&refns=x", "/info-refs?net=7&refns=", "/upload-pack?net=7&refns=-1",
		"/validate?net=7&refns=12345678901234567890", "/validate?net=7&refns=4%2F2", "/info-refs?net=7&forks=4,x",
		"/upload-pack?net=7&forks=", "/validate?net=7&forks=1/2"} {
		if code, b, _ := nsDo(t, "POST", s.URL+p, "version=2", lsRefsBody); code != 400 || !strings.Contains(b, "bad ") {
			t.Fatalf("%s: %d %s", p, code, b)
		}
	}
	for _, h := range []string{"refs/namespaces/x/refs/heads/dev", "refs/namespaces/42/refs/tags/v1", "refs/namespaces/42/HEAD",
		"refs/namespaces/42/refs/heads/../x"} {
		if code, _, _ := nsDo(t, "POST", s.URL+"/refs?net=7&head="+h, "", ""); code != 400 {
			t.Fatalf("head=%s: %d", h, code)
		}
	}
}

// A fork's HEAD is a symbolic ref under refs/ (refs/namespaces/<id>/HEAD), which no page lists: a rehydrate's PUT
// start=1 keeps it and the branch it names, even as the page moves that branch (one transaction would otherwise hold
// a delete through the symref beside the branch's update), and GET /refs lists it no more than HEAD.
func TestRefsPutKeepsNamespaceHead(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	a := commit(t, wt, map[string]string{"f": "a\n"})
	b := commit(t, wt, map[string]string{"f": "b\n"})
	run(t, wt, "", "push", "-q", repo("7"), a+":refs/heads/main", a+":refs/namespaces/42/refs/heads/dev",
		b+":refs/heads/objects")
	refsCall(t, s.URL, "POST", "&head=refs/namespaces/42/refs/heads/dev", "")
	nsHead := func() string {
		t.Helper()
		return run(t, repo("7"), "", "for-each-ref", "--format=%(objectname) %(symref)", "refs/namespaces/42/HEAD")
	}
	if got := nsHead(); got != a+" refs/namespaces/42/refs/heads/dev" {
		t.Fatalf("namespace HEAD before: %q", got)
	}
	// GET lists the refs, not the symref.
	code, got, _ := nsDo(t, "GET", s.URL+"/refs?net=7", "", "")
	if code != 200 || strings.Contains(got, "refs/namespaces/42/HEAD") || !strings.Contains(got, a+" refs/namespaces/42/refs/heads/dev\n") {
		t.Fatalf("GET /refs: %d %q", code, got)
	}
	// Pages: the first lists the namespace branch, moved to b; the second the root's.
	if v := refsCall(t, s.URL, "PUT", "&start=1&repack=0", b+" refs/namespaces/42/refs/heads/dev\n"); v["skipped"].(float64) != 0 {
		t.Fatalf("page 1: %v", v)
	}
	if v := refsCall(t, s.URL, "PUT", "&done=1&repack=0&head=refs/heads/main", a+" refs/heads/main\n"); v["skipped"].(float64) != 0 {
		t.Fatalf("page 2: %v", v)
	}
	if got := run(t, repo("7"), "", "for-each-ref", "--format=%(objectname) %(refname)"); got != a+" refs/heads/main\n"+
		b+" refs/namespaces/42/HEAD\n"+b+" refs/namespaces/42/refs/heads/dev" {
		t.Fatalf("refs after the PUT: %q", got)
	}
	if got := nsHead(); got != b+" refs/namespaces/42/refs/heads/dev" {
		t.Fatalf("namespace HEAD after: %q", got)
	}
	// And a single-body PUT that leaves the branch where it is.
	refsCall(t, s.URL, "PUT", "&repack=0", a+" refs/heads/main\n"+b+" refs/namespaces/42/refs/heads/dev\n")
	if got := nsHead(); got != b+" refs/namespaces/42/refs/heads/dev" {
		t.Fatalf("namespace HEAD after a whole PUT: %q", got)
	}
}
