package main

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// Session refs with real git clients (B0-37's fetch half): a host-owned ref is never in a v0 or v1 advertisement or a
// `--mirror` clone, v2 `ls-refs` lists a session ref only to a `ref-prefix` that begins with `refs/graftyard/sessions/`
// and the ref, and a session log stays fetchable by id.

// smartHTTP serves the shard's /info-refs and /upload-pack under git's smart-HTTP paths, as the edge does, forwarding
// Git-Protocol.
func smartHTTP(t *testing.T, s *httptest.Server, net string) string {
	t.Helper()
	h := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		path := "/upload-pack"
		method := http.MethodPost
		if strings.HasSuffix(r.URL.Path, "/info/refs") {
			if r.URL.Query().Get("service") != "git-upload-pack" {
				http.Error(w, "upload-pack only", 403)
				return
			}
			path, method = "/info-refs", http.MethodGet
		}
		req, _ := http.NewRequest(method, s.URL+path+"?net="+net, r.Body)
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

func TestSessionRefsOnlyToTheirPrefix(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=9")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	main := commit(t, wt, map[string]string{"f": "a\n"})
	log := commit(t, wt, map[string]string{"log": "session\n"})
	other := commit(t, wt, map[string]string{"x": "host\n"})
	run(t, wt, "", "push", "-q", repo("9"), main+":refs/heads/main", log+":refs/graftyard/sessions/12",
		log+":refs/graftyard/sessions/3", other+":refs/graftyard/other")
	url := smartHTTP(t, s, "9")
	cl := t.TempDir()
	lsRemote := func(proto string, args ...string) string {
		a := append([]string{"-c", "protocol.version=" + proto, "ls-remote", url}, args...)
		return run(t, cl, "", a...)
	}
	hidden := func(what, out string) {
		t.Helper()
		if strings.Contains(out, "refs/graftyard/") {
			t.Fatalf("%s lists a host-owned ref:\n%s", what, out)
		}
		if !strings.Contains(out, "refs/heads/main") {
			t.Fatalf("%s lacks refs/heads/main:\n%s", what, out)
		}
	}
	hidden("v0", lsRemote("0"))
	hidden("v1", lsRemote("1"))
	hidden("v2 without a prefix", lsRemote("2"))
	if out := lsRemote("0", "refs/graftyard/sessions/12"); out != "" {
		t.Fatalf("v0 with a pattern lists a session ref:\n%s", out)
	}
	// git 2.56's ls-remote matches its patterns locally and sends no `ref-prefix` (GIT_TRACE_PACKET), so the
	// prefixes go as raw v2 ls-refs requests.
	lsRefs := func(prefixes ...string) string {
		t.Helper()
		b := pkt("command=ls-refs\n") + "0001"
		for _, p := range prefixes {
			b += pkt("ref-prefix " + p + "\n")
		}
		req, _ := http.NewRequest("POST", url+"/git-upload-pack", strings.NewReader(b+"0000"))
		req.Header.Set("git-protocol", "version=2")
		r, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		defer r.Body.Close()
		out, _ := io.ReadAll(r.Body)
		var names []string
		for _, l := range strings.Split(string(out), "\n") {
			if f := strings.Fields(l); len(f) >= 2 {
				names = append(names, f[1])
			}
		}
		return strings.Join(names, " ")
	}
	for _, c := range []struct {
		prefixes []string
		want     string
	}{
		{nil, "HEAD refs/heads/main"},
		{[]string{"refs/"}, "refs/heads/main"},
		{[]string{"refs/graftyard/"}, ""},
		{[]string{"refs/graftyard/other"}, ""},
		{[]string{"refs/graftyard/sessions/12"}, "refs/graftyard/sessions/12"},
		{[]string{"refs/graftyard/sessions/1"}, "refs/graftyard/sessions/12"},
		{[]string{"refs/graftyard/sessions/"}, "refs/graftyard/sessions/12 refs/graftyard/sessions/3"},
		// A sibling prefix adds no session ref that begins with none of the session prefixes.
		{[]string{"refs/", "refs/graftyard/sessions/3"}, "refs/graftyard/sessions/3 refs/heads/main"},
		{[]string{"refs/heads/", "refs/graftyard/sessions/12"}, "refs/graftyard/sessions/12 refs/heads/main"},
	} {
		if got := lsRefs(c.prefixes...); got != c.want {
			t.Fatalf("ls-refs %q: %q, want %q", c.prefixes, got, c.want)
		}
	}
	// A real client: `git fetch` with a session refspec sends that `ref-prefix` and gets the session's tip, as
	// graftyard-workspace reads it; with a `refs/*` refspec it gets no host-owned ref.
	fr := t.TempDir()
	run(t, fr, "", "init", "-q", "--bare")
	run(t, fr, "", "-c", "protocol.version=2", "fetch", "-q", url, "refs/graftyard/sessions/12:refs/s")
	if got := run(t, fr, "", "rev-parse", "refs/s"); got != log {
		t.Fatalf("fetch of session 12: %s", got)
	}
	run(t, fr, "", "-c", "protocol.version=2", "fetch", "-q", url, "refs/*:refs/all/*")
	if refs := run(t, fr, "", "for-each-ref", "--format=%(refname)", "refs/all/"); refs != "refs/all/heads/main" {
		t.Fatalf("fetch refs/*: %s", refs)
	}
	// --mirror (v0 and v2) copies refs/heads/main and no host-owned ref.
	for _, proto := range []string{"0", "2"} {
		dir := t.TempDir()
		run(t, dir, "", "-c", "protocol.version="+proto, "clone", "-q", "--mirror", url, "m.git")
		refs := run(t, dir+"/m.git", "", "for-each-ref", "--format=%(refname)")
		if strings.Contains(refs, "refs/graftyard/") || !strings.Contains(refs, "refs/heads/main") {
			t.Fatalf("--mirror over v%s: %s", proto, refs)
		}
	}
	// A session log is fetchable by id (listing is the only gate), as graftyard-workspace reads its tip.
	fe := t.TempDir()
	run(t, fe, "", "init", "-q", "--bare")
	run(t, fe, "", "-c", "protocol.version=2", "fetch", "-q", url, log)
	if got := run(t, fe, "", "cat-file", "-t", log); got != "commit" {
		t.Fatalf("session log by id: %s", got)
	}
}

func TestSessionPrefixesParsesOnlyLsRefs(t *testing.T) {
	body := pkt("command=ls-refs\n") + "0001" + pkt("peel\n") + pkt("ref-prefix refs/heads/\n") +
		pkt("ref-prefix refs/graftyard/sessions/7\n") + "0000"
	if got := sessionPrefixes([]byte(body)); len(got) != 1 || got[0] != "refs/graftyard/sessions/7" {
		t.Fatalf("ls-refs: %q", got)
	}
	fetch := pkt("command=fetch\n") + "0001" + pkt("ref-prefix refs/graftyard/sessions/7\n") + "0000"
	if got := sessionPrefixes([]byte(fetch)); got != nil {
		t.Fatalf("fetch: %q", got)
	}
	if got := sessionPrefixes([]byte("zzzz")); got != nil {
		t.Fatalf("malformed: %q", got)
	}
}

func pkt(s string) string { return hex4(len(s)+4) + s }

func hex4(n int) string {
	const d = "0123456789abcdef"
	return string([]byte{d[n>>12&15], d[n>>8&15], d[n>>4&15], d[n&15]})
}
