package main

import (
	"bytes"
	"encoding/json"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

// The test merge with real git: a clean merge answers a commit whose pack holds only its new objects and leaves the
// repository unchanged, a conflict answers its paths, and the tree-diff check refuses a merge that brings in a change
// neither side made.

func run(t *testing.T, dir string, stdin string, args ...string) string {
	t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@t", "GIT_COMMITTER_NAME=t",
		"GIT_COMMITTER_EMAIL=t@t", "GIT_AUTHOR_DATE=@1700000000 +0000", "GIT_COMMITTER_DATE=@1700000000 +0000")
	cmd.Stdin = strings.NewReader(stdin)
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("git %v: %v: %s", args, err, out)
	}
	return strings.TrimSpace(string(out))
}

// commit writes files over the work tree's and commits them, returning the commit.
func commit(t *testing.T, wt string, files map[string]string) string {
	t.Helper()
	for p, c := range files {
		os.MkdirAll(filepath.Dir(filepath.Join(wt, p)), 0o755)
		if err := os.WriteFile(filepath.Join(wt, p), []byte(c), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	run(t, wt, "", "add", "-A")
	run(t, wt, "", "commit", "-qm", "c")
	return run(t, wt, "", "rev-parse", "HEAD")
}

type mergeOut struct {
	Merge    string   `json:"merge"`
	Pack     []byte   `json:"pack"`
	Conflict []string `json:"conflict"`
	CPUMs    float64  `json:"cpuMs"`
}

func merge(t *testing.T, url, base, head string) (int, mergeOut, string) {
	t.Helper()
	r, err := http.Post(url+"/merge?net=7&base="+base+"&head="+head, "", nil)
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	var b bytes.Buffer
	b.ReadFrom(r.Body)
	var o mergeOut
	json.Unmarshal(b.Bytes(), &o)
	return r.StatusCode, o, b.String()
}

func TestMerge(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	root := commit(t, wt, map[string]string{"f": "a\nb\nc\n", "g": "x\n"})
	run(t, wt, "", "checkout", "-qb", "h")
	head := commit(t, wt, map[string]string{"f": "a\nb\nC\n", "n": "new\n"})
	run(t, wt, "", "checkout", "-q", "main")
	base := commit(t, wt, map[string]string{"f": "A\nb\nc\n"})
	run(t, wt, "", "checkout", "-q", "h")
	clash := commit(t, wt, map[string]string{"g": "head\n"})
	run(t, wt, "", "checkout", "-q", "main")
	other := commit(t, wt, map[string]string{"g": "base\n"})
	run(t, wt, "", "push", "-q", repo("7"), "main", "h")
	before := run(t, repo("7"), "", "count-objects", "-v")

	code, o, body := merge(t, s.URL, base, head)
	if code != 200 || o.Merge == "" || len(o.Conflict) != 0 {
		t.Fatalf("clean merge: %d %s", code, body)
	}
	if o.CPUMs <= 0 || o.CPUMs >= 10000 {
		t.Fatalf("clean merge cpuMs %v", o.CPUMs)
	}
	// The pack holds the merge commit and its new trees and blobs, not base's or head's: it indexes against the
	// repository and adds exactly the merge.
	idx := t.TempDir()
	run(t, idx, "", "init", "-q", "--bare")
	run(t, idx, "", "config", "core.alternateRefsCommand", "true")
	alt := filepath.Join(idx, "objects", "info", "alternates")
	os.WriteFile(alt, []byte(filepath.Join(repo("7"), "objects")+"\n"), 0o644)
	run(t, idx, string(o.Pack), "index-pack", "--stdin", "--strict")
	if got := run(t, idx, "", "cat-file", "-p", o.Merge+"^{tree}"); !strings.Contains(got, "\tn") {
		t.Fatalf("merge tree lacks head's new file: %s", got)
	}
	if got := run(t, idx, "", "show", "-s", "--format=%P", o.Merge); got != base+" "+head {
		t.Fatalf("parents %q", got)
	}
	if f := run(t, idx, "", "show", o.Merge+":f"); f != "A\nb\nC" {
		t.Fatalf("merged f is %q", f)
	}
	objs := run(t, idx, "", "count-objects", "-v")
	if !strings.Contains(objs, "in-pack: 3") {
		t.Fatalf("the pack is not just the merge commit, its tree, and f: %s", objs)
	}
	// Deterministic: the same pair gives the same commit.
	if _, again, _ := merge(t, s.URL, base, head); again.Merge != o.Merge {
		t.Fatalf("merge %s then %s", o.Merge, again.Merge)
	}
	if after := run(t, repo("7"), "", "count-objects", "-v"); after != before {
		t.Fatalf("the repository changed: %s then %s", before, after)
	}

	code, o, body = merge(t, s.URL, other, clash)
	if code != 200 || o.Merge != "" || len(o.Conflict) != 1 || o.Conflict[0] != "g" {
		t.Fatalf("conflict: %d %s", code, body)
	}
	if o.CPUMs <= 0 || o.CPUMs >= 10000 {
		t.Fatalf("conflict cpuMs %v", o.CPUMs)
	}

	if code, _, _ := merge(t, s.URL, base, strings.Repeat("e", 40)); code != 409 {
		t.Fatalf("a missing head answered %d", code)
	}

	// The check itself: a merge commit whose tree carries a change neither side made is refused, and so is one that
	// drops head's change.
	var env []string
	bad := func(tree string) string {
		return run(t, repo("7"), "m\n", "commit-tree", tree, "-p", base, "-p", head)
	}
	extra := run(t, wt, "", "rev-parse", root+"^{tree}")
	if err := treeDiffCheck(repo("7"), env, base, head, bad(extra), nil); err == nil {
		t.Fatal("a merge with root's tree passed the tree-diff check")
	}
	if err := treeDiffCheck(repo("7"), env, base, head, bad(run(t, wt, "", "rev-parse", base+"^{tree}")), nil); err == nil {
		t.Fatal("a merge that drops head's change passed the tree-diff check")
	}
}
