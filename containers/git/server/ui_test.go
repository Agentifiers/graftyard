package main

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

// The web app's /ui/* routes on a small real repository: ref resolution, trees with per-entry last commits, blame
// ranges, the pull-request diff against the merge base, compare counts, READMEs, and the refusals of untrusted input.

// uiGet calls a /ui route and decodes its JSON into v, answering the status and the body.
func uiGet(t *testing.T, s *httptest.Server, path string, v any) (int, string) {
	t.Helper()
	r, err := http.Get(s.URL + path)
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	b, err := io.ReadAll(r.Body)
	if err != nil {
		t.Fatal(err)
	}
	if r.StatusCode == 200 && v != nil {
		if err := json.Unmarshal(b, v); err != nil {
			t.Fatalf("%s: %v: %s", path, err, b)
		}
	}
	return r.StatusCode, string(b)
}

// commitAt commits files with msg at the Unix time at, so dates differ between commits.
func commitAt(t *testing.T, wt string, at string, files map[string]string, msg string) string {
	t.Helper()
	for p, c := range files {
		os.MkdirAll(filepath.Dir(filepath.Join(wt, p)), 0o755)
		if err := os.WriteFile(filepath.Join(wt, p), []byte(c), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	run(t, wt, "", "add", "-A")
	cmd := exec.Command("git", "commit", "-qm", msg)
	cmd.Dir = wt
	cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=Ann", "GIT_AUTHOR_EMAIL=ann@a", "GIT_COMMITTER_NAME=c",
		"GIT_COMMITTER_EMAIL=c@c", "GIT_AUTHOR_DATE=@"+at+" +0000", "GIT_COMMITTER_DATE=@"+at+" +0000")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("commit: %v: %s", err, out)
	}
	return run(t, wt, "", "rev-parse", "HEAD")
}

// uiRepo starts the server with network 7 empty and a work tree on main, and returns a push function that sends
// every branch and tag to the network and points its HEAD at main.
func uiRepo(t *testing.T) (*httptest.Server, string, func()) {
	t.Helper()
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	return s, wt, func() {
		run(t, wt, "", "push", "-q", "-f", repo("7"), "refs/heads/*:refs/heads/*", "refs/tags/*:refs/tags/*")
		run(t, repo("7"), "", "symbolic-ref", "HEAD", "refs/heads/main")
	}
}

type resolveOut struct {
	Ref, Path, Kind, Sha string
	Type                 *string
	DefaultBranch        string `json:"default_branch"`
}

func TestUIResolve(t *testing.T) {
	s, wt, push := uiRepo(t)
	c1 := commitMsg(t, wt, map[string]string{"README.md": "r\n", "src/lib.rs": "fn x() {}\n"}, "one")
	run(t, wt, "", "tag", "-a", "-m", "v1", "v1")
	// git refuses a branch "feature" beside "feature/x" (a directory/file conflict in the ref namespace), so the
	// shorter prefix is a tag on another commit.
	run(t, wt, "", "tag", "feature")
	run(t, wt, "", "checkout", "-qb", "feature/x")
	c2 := commitMsg(t, wt, map[string]string{"x/README.md": "x\n"}, "two")
	run(t, wt, "", "checkout", "-q", "main")
	push()

	cases := []struct {
		splat string
		want  resolveOut
		typ   string // "" for null
	}{
		{"main/src/lib.rs", resolveOut{Ref: "main", Path: "src/lib.rs", Kind: "branch", Sha: c1}, "file"},
		{"main", resolveOut{Ref: "main", Path: "", Kind: "branch", Sha: c1}, "dir"},
		{"feature/x/x/README.md", resolveOut{Ref: "feature/x", Path: "x/README.md", Kind: "branch", Sha: c2}, "file"},
		{"feature/README.md", resolveOut{Ref: "feature", Path: "README.md", Kind: "tag", Sha: c1}, "file"},
		{"v1/src", resolveOut{Ref: "v1", Path: "src", Kind: "tag", Sha: c1}, "dir"},
		{c2[:7] + "/x", resolveOut{Ref: c2[:7], Path: "x", Kind: "commit", Sha: c2}, "dir"},
		{c1 + "/README.md", resolveOut{Ref: c1, Path: "README.md", Kind: "commit", Sha: c1}, "file"},
		{"main/nope.txt", resolveOut{Ref: "main", Path: "nope.txt", Kind: "branch", Sha: c1}, ""},
	}
	for _, c := range cases {
		var o resolveOut
		code, body := uiGet(t, s, "/ui/resolve?net=7&splat="+url.QueryEscape(c.splat)+"&wait=1&repo=r", &o)
		if code != 200 {
			t.Fatalf("%s: %d %s", c.splat, code, body)
		}
		typ := ""
		if o.Type != nil {
			typ = *o.Type
		}
		if o.Ref != c.want.Ref || o.Path != c.want.Path || o.Kind != c.want.Kind || o.Sha != c.want.Sha ||
			typ != c.typ || o.DefaultBranch != "main" {
			t.Fatalf("%s: %s", c.splat, body)
		}
	}
	if code, body := uiGet(t, s, "/ui/resolve?net=7&splat=nope/README.md", nil); code != 404 {
		t.Fatalf("no ref: %d %s", code, body)
	}
	if code, body := uiGet(t, s, "/ui/resolve?net=7&splat=main/../x", nil); code != 400 {
		t.Fatalf("..: %d %s", code, body)
	}
}

type summary struct {
	Sha, Message, Date string
	Author             struct {
		Login     *string
		Name      string
		Email     string
		AvatarURL *string `json:"avatar_url"`
	}
}

type treeResp struct {
	Latest  summary
	Count   int
	Entries []struct {
		Name, Path, Type string
		Commit           summary
	}
	Truncated bool
}

func TestUITree(t *testing.T) {
	s, wt, push := uiRepo(t)
	c1 := commitMsg(t, wt, map[string]string{"readme.md": "r\n", "src/a.rs": "a\n", "src/b.rs": "b\n",
		"src/Zed/z.rs": "z\n", "docs/guide.md": "g\n", "Build.txt": "b\n"}, "one\n\nbody")
	c2 := commitMsg(t, wt, map[string]string{"src/a.rs": "a2\n"}, "two")
	c3 := commitMsg(t, wt, map[string]string{"docs/guide.md": "g2\n"}, "three")
	push()

	var o treeResp
	if code, body := uiGet(t, s, "/ui/tree?net=7&commit="+c3+"&path=", &o); code != 200 {
		t.Fatalf("root: %d %s", code, body)
	}
	// Directories first, then files, case-insensitively by name; each with the last commit that changed it.
	want := [][4]string{{"docs", "docs", "dir", c3}, {"src", "src", "dir", c2}, {"Build.txt", "Build.txt", "file", c1},
		{"readme.md", "readme.md", "file", c1}}
	if len(o.Entries) != len(want) || o.Latest.Sha != c3 || o.Count != 3 || o.Truncated {
		t.Fatalf("root: %+v", o)
	}
	for i, w := range want {
		e := o.Entries[i]
		if e.Name != w[0] || e.Path != w[1] || e.Type != w[2] || e.Commit.Sha != w[3] {
			t.Fatalf("root entry %d: %+v, want %v", i, e, w)
		}
	}
	if m := o.Entries[3].Commit; m.Message != "one\n\nbody" || m.Author.Name != "t" || m.Author.Email != "t@t" ||
		m.Author.Login != nil || m.Author.AvatarURL != nil || !strings.HasPrefix(m.Date, "2023-11-14T22:13:20") {
		t.Fatalf("summary: %+v", m)
	}

	o = treeResp{}
	if code, body := uiGet(t, s, "/ui/tree?net=7&commit="+c3+"&path=src", &o); code != 200 {
		t.Fatalf("src: %d %s", code, body)
	}
	want = [][4]string{{"Zed", "src/Zed", "dir", c1}, {"a.rs", "src/a.rs", "file", c2}, {"b.rs", "src/b.rs", "file", c1}}
	if len(o.Entries) != len(want) || o.Latest.Sha != c2 || o.Count != 2 {
		t.Fatalf("src: %+v", o)
	}
	for i, w := range want {
		e := o.Entries[i]
		if e.Name != w[0] || e.Path != w[1] || e.Type != w[2] || e.Commit.Sha != w[3] {
			t.Fatalf("src entry %d: %+v, want %v", i, e, w)
		}
	}

	for path, code := range map[string]int{"src/a.rs": 404, "nope": 404, "-src": 400, "src/../docs": 400,
		"src%00x": 400} {
		if got, body := uiGet(t, s, "/ui/tree?net=7&commit="+c3+"&path="+path, nil); got != code {
			t.Fatalf("tree %q: %d %s, want %d", path, got, body, code)
		}
	}
}

type blameResp struct {
	Path   string
	Lines  []string
	Ranges []struct {
		Start, End int
		Commit     summary
	}
	Oldest, Newest string
}

func TestUIBlame(t *testing.T) {
	s, wt, push := uiRepo(t)
	c1 := commitAt(t, wt, "1700000000", map[string]string{"f.txt": "a\nb\nc\nd\ne\n", "bin": "x\x00y"}, "one")
	c2 := commitAt(t, wt, "1700086400", map[string]string{"f.txt": "a\nb\nC\nD\ne\n"}, "two")
	push()
	d1, d2 := run(t, wt, "", "log", "-1", "--format=%cI", c1), run(t, wt, "", "log", "-1", "--format=%cI", c2)

	var o blameResp
	if code, body := uiGet(t, s, "/ui/blame?net=7&commit="+c2+"&path=f.txt", &o); code != 200 {
		t.Fatalf("blame: %d %s", code, body)
	}
	if o.Path != "f.txt" || strings.Join(o.Lines, ",") != "a,b,C,D,e" || len(o.Ranges) != 3 {
		t.Fatalf("blame: %+v", o)
	}
	want := []struct {
		start, end int
		sha        string
	}{{1, 2, c1}, {3, 4, c2}, {5, 5, c1}}
	for i, w := range want {
		r := o.Ranges[i]
		if r.Start != w.start || r.End != w.end || r.Commit.Sha != w.sha {
			t.Fatalf("range %d: %+v, want %v", i, r, w)
		}
	}
	if o.Ranges[1].Commit.Message != "two" || o.Ranges[1].Commit.Author.Name != "Ann" || o.Oldest != d1 ||
		o.Newest != d2 || d1 == d2 {
		t.Fatalf("blame commits: %+v, dates %s %s", o, d1, d2)
	}

	run(t, wt, "", "checkout", "-q", "-b", "sub")
	commitAt(t, wt, "1700090000", map[string]string{"d/x": "x\n"}, "dir")
	push()
	dirC := run(t, wt, "", "rev-parse", "HEAD")
	for q, code := range map[string]int{
		"commit=" + c2 + "&path=bin":                         422,
		"commit=" + dirC + "&path=d":                         404,
		"commit=" + c2 + "&path=nope":                        404,
		"commit=" + c2 + "&path=-f.txt":                      400,
		"commit=" + c2 + "&path=":                            400,
		"commit=" + c2[:39] + "&path=f.txt":                  400,
		"commit=" + c2 + "G&path=f.txt":                      400,
		"commit=" + strings.Repeat("ab", 20) + "&path=f.txt": 404,
	} {
		if got, body := uiGet(t, s, "/ui/blame?net=7&"+q, nil); got != code {
			t.Fatalf("blame %s: %d %s, want %d", q, got, body, code)
		}
	}
}

type diffResp struct {
	Files []struct {
		Filename         string
		PreviousFilename *string `json:"previous_filename"`
		Status           string
		Additions        int
		Deletions        int
		Changes          int
		Sha              string
		Patch            *string
	}
	Truncated bool
}

func TestUIDiff(t *testing.T) {
	s, wt, push := uiRepo(t)
	long := strings.Repeat("a line long enough to keep the rename similar\n", 20)
	fork := commitMsg(t, wt, map[string]string{"keep.txt": "k\n", "gone.txt": "g\n", "mod.txt": "1\n2\n3\n",
		"old name.txt": long}, "fork")
	run(t, wt, "", "checkout", "-qb", "pr")
	run(t, wt, "", "rm", "-q", "gone.txt")
	run(t, wt, "", "mv", "old name.txt", "new name.txt")
	head := commitMsg(t, wt, map[string]string{"new.txt": "n1\nn2\n", "mod.txt": "1\nTWO\n3\n",
		"bin.dat": "\x00\x01\x02"}, "pr")
	run(t, wt, "", "checkout", "-q", "main")
	// The base moves after the head branched: a two-dot diff would show keep.txt reverted.
	base := commitMsg(t, wt, map[string]string{"keep.txt": "k2\n"}, "base moved")
	push()
	gone := run(t, wt, "", "rev-parse", fork+":gone.txt")

	var o diffResp
	if code, body := uiGet(t, s, "/ui/diff?net=7&base="+base+"&head="+head+"&next=x", &o); code != 200 {
		t.Fatalf("diff: %d %s", code, body)
	}
	byName := map[string]int{}
	for i, f := range o.Files {
		byName[f.Filename] = i
	}
	if _, ok := byName["keep.txt"]; ok || len(o.Files) != 5 || o.Truncated {
		t.Fatalf("diff files: %+v", o)
	}
	check := func(name, status string, add, del int, prev string, patch string) {
		t.Helper()
		i, ok := byName[name]
		if !ok {
			t.Fatalf("no %s in %+v", name, o.Files)
		}
		f := o.Files[i]
		p, gotPrev := "", ""
		if f.Patch != nil {
			p = *f.Patch
		}
		if f.PreviousFilename != nil {
			gotPrev = *f.PreviousFilename
		}
		if f.Status != status || f.Additions != add || f.Deletions != del || f.Changes != add+del || gotPrev != prev ||
			p != patch {
			t.Fatalf("%s: %+v (prev %q, patch %q)", name, f, gotPrev, p)
		}
		blob := gone
		if status != "removed" {
			blob = run(t, wt, "", "rev-parse", head+":"+name)
		}
		if f.Sha != blob {
			t.Fatalf("%s: sha %s, want %s", name, f.Sha, blob)
		}
	}
	check("new.txt", "added", 2, 0, "", "@@ -0,0 +1,2 @@\n+n1\n+n2")
	check("gone.txt", "removed", 0, 1, "", "@@ -1 +0,0 @@\n-g")
	check("mod.txt", "modified", 1, 1, "", "@@ -1,3 +1,3 @@\n 1\n-2\n+TWO\n 3")
	check("new name.txt", "renamed", 0, 0, "old name.txt", "")
	check("bin.dat", "added", 0, 0, "", "")

	if code, body := uiGet(t, s, "/ui/diff?net=7&base="+base+"&head="+strings.Repeat("cd", 20), nil); code != 404 {
		t.Fatalf("missing head: %d %s", code, body)
	}
	if code, body := uiGet(t, s, "/ui/diff?net=7&base=main&head="+head, nil); code != 400 {
		t.Fatalf("a ref as base: %d %s", code, body)
	}
}

func TestUICompare(t *testing.T) {
	s, wt, push := uiRepo(t)
	fork := commitMsg(t, wt, map[string]string{"f": "0\n"}, "fork")
	run(t, wt, "", "checkout", "-qb", "pr")
	var head string
	for _, n := range []string{"1", "2", "3"} {
		head = commitMsg(t, wt, map[string]string{"h" + n: n}, "head "+n)
	}
	run(t, wt, "", "checkout", "-q", "main")
	var base string
	for _, n := range []string{"1", "2"} {
		base = commitMsg(t, wt, map[string]string{"b" + n: n}, "base "+n)
	}
	run(t, wt, "", "checkout", "-q", "--orphan", "lone")
	lone := commitMsg(t, wt, map[string]string{"l": "l"}, "lone")
	push()

	var o struct {
		MergeBase    *string `json:"merge_base"`
		AheadBy      int     `json:"ahead_by"`
		BehindBy     int     `json:"behind_by"`
		Additions    int     `json:"additions"`
		Deletions    int     `json:"deletions"`
		ChangedFiles int     `json:"changed_files"`
	}
	if code, body := uiGet(t, s, "/ui/compare?net=7&base="+base+"&head="+head, &o); code != 200 {
		t.Fatalf("compare: %d %s", code, body)
	}
	// From the merge base: head's three one-line files, and none of base's b1 and b2.
	if o.MergeBase == nil || *o.MergeBase != fork || o.AheadBy != 3 || o.BehindBy != 2 || o.Additions != 3 ||
		o.Deletions != 0 || o.ChangedFiles != 3 {
		t.Fatalf("compare: %+v", o)
	}
	o.MergeBase = nil
	if code, body := uiGet(t, s, "/ui/compare?net=7&base="+base+"&head="+lone, &o); code != 200 || o.MergeBase != nil ||
		o.AheadBy != 1 || o.BehindBy != 3 {
		t.Fatalf("unrelated: %d %s", code, body)
	}
	if code, body := uiGet(t, s, "/ui/compare?net=7&base="+strings.Repeat("ef", 20)+"&head="+head, nil); code != 404 {
		t.Fatalf("missing base: %d %s", code, body)
	}

	// A rename with an edit, a deletion, and a binary file, while base edits the renamed file's old lines after the
	// fork. GHES counts a pull request from the merge base: the rename is one file (+1 -1), the deletion -2, the
	// binary file one file with no lines; base's own edit counts nowhere. A diff from base's tip would also count it.
	run(t, wt, "", "checkout", "-q", "main")
	fork2 := commitMsg(t, wt, map[string]string{"old.txt": "a\nb\nc\nd\ne\nf\ng\nh\n", "gone.txt": "x\ny\n"}, "fork 2")
	run(t, wt, "", "checkout", "-qb", "pr2")
	run(t, wt, "", "mv", "old.txt", "new.txt")
	run(t, wt, "", "rm", "-q", "gone.txt")
	head2 := commitMsg(t, wt, map[string]string{"new.txt": "a\nb\nc\nd\ne\nf\ng\nH\n", "logo.bin": "\x00\x01\x02"}, "pr2")
	run(t, wt, "", "checkout", "-q", "main")
	base2 := commitMsg(t, wt, map[string]string{"old.txt": "A\nb\nc\nd\ne\nf\ng\nh\n", "more.txt": "1\n2\n3\n"}, "base 2")
	push()
	if code, body := uiGet(t, s, "/ui/compare?net=7&base="+base2+"&head="+head2, &o); code != 200 {
		t.Fatalf("compare 2: %d %s", code, body)
	}
	if o.MergeBase == nil || *o.MergeBase != fork2 || o.AheadBy != 1 || o.BehindBy != 1 || o.Additions != 1 ||
		o.Deletions != 3 || o.ChangedFiles != 3 {
		t.Fatalf("compare 2: %+v", o)
	}
}

func TestUIReadme(t *testing.T) {
	s, wt, push := uiRepo(t)
	c := commitMsg(t, wt, map[string]string{"readme.md": "# root\n", "README.txt": "plain\n",
		"docs/README": "bare\n", "docs/README.rst": "rst\n", "docs/readmeish": "no\n", "src/main.rs": "fn main() {}\n"},
		"one")
	push()
	var o struct {
		Name, Path, Sha, Text string
		Size                  int64
	}
	if code, body := uiGet(t, s, "/ui/readme?net=7&commit="+c+"&path=", &o); code != 200 {
		t.Fatalf("root: %d %s", code, body)
	}
	if o.Name != "readme.md" || o.Path != "readme.md" || o.Text != "# root\n" || o.Size != 7 ||
		o.Sha != run(t, wt, "", "rev-parse", c+":readme.md") {
		t.Fatalf("root: %+v", o)
	}
	if code, body := uiGet(t, s, "/ui/readme?net=7&commit="+c+"&path=docs", &o); code != 200 {
		t.Fatalf("docs: %d %s", code, body)
	}
	if o.Name != "README.rst" || o.Path != "docs/README.rst" || o.Text != "rst\n" {
		t.Fatalf("docs: %+v", o)
	}
	for path, code := range map[string]int{"src": 404, "nope": 404, "readme.md": 404, "-x": 400} {
		if got, body := uiGet(t, s, "/ui/readme?net=7&commit="+c+"&path="+path, nil); got != code {
			t.Fatalf("readme %q: %d %s, want %d", path, got, body, code)
		}
	}
}

func TestUIBlob(t *testing.T) {
	s, wt, push := uiRepo(t)
	c := commitMsg(t, wt, map[string]string{".github/workflows/ci.yml": "on: push\njobs: {}\n",
		"src/main.rs": "fn main() {}\n", "logo.bin": "\x89PNG\x00\x01"}, "one")
	push()
	var o struct {
		Path, Sha, Text string
		Size            int64
	}
	if code, body := uiGet(t, s, "/ui/blob?net=7&commit="+c+"&path=.github/workflows/ci.yml", &o); code != 200 {
		t.Fatalf("ci.yml: %d %s", code, body)
	}
	if o.Path != ".github/workflows/ci.yml" || o.Text != "on: push\njobs: {}\n" || o.Size != 18 ||
		o.Sha != run(t, wt, "", "rev-parse", c+":.github/workflows/ci.yml") {
		t.Fatalf("ci.yml: %+v", o)
	}
	for path, code := range map[string]int{"src": 404, "nope": 404, "logo.bin": 422, "-x": 400, "": 400} {
		if got, body := uiGet(t, s, "/ui/blob?net=7&commit="+c+"&path="+path, nil); got != code {
			t.Fatalf("blob %q: %d %s, want %d", path, got, body, code)
		}
	}
}

func TestUICommits(t *testing.T) {
	s, wt, push := uiRepo(t)
	commitMsg(t, wt, map[string]string{"f": "0\n"}, "fork")
	run(t, wt, "", "checkout", "-qb", "pr")
	h1 := commitMsg(t, wt, map[string]string{"h1": "1\n"}, "head 1")
	run(t, wt, "", "checkout", "-q", "main")
	commitMsg(t, wt, map[string]string{"b1": "1\n"}, "base 1")
	run(t, wt, "", "checkout", "-q", "pr")
	run(t, wt, "", "merge", "-q", "--no-edit", "main")
	m := run(t, wt, "", "rev-parse", "HEAD")
	// The last head commit's message is not UTF-8. git commit would re-encode it, so the object is written by hand.
	commitMsg(t, wt, map[string]string{"h2": "2\n"}, "scratch")
	tree := run(t, wt, "", "rev-parse", "HEAD^{tree}")
	obj := "tree " + tree + "\nparent " + m + "\nauthor t <t@t> 1700000000 +0000\n" +
		"committer t <t@t> 1700000000 +0000\n\nhead 2 \xff\n"
	out, err := gitOut(wt, obj, "hash-object", "-t", "commit", "-w", "--stdin")
	if err != nil {
		t.Fatal(err)
	}
	h2 := strings.TrimSpace(string(out))
	run(t, wt, "", "reset", "-q", "--hard", h2)
	run(t, wt, "", "checkout", "-q", "main")
	// The base moves after the head branched and after the merge.
	base := commitMsg(t, wt, map[string]string{"b2": "2\n"}, "base 2")
	push()

	var o struct {
		Commits []struct {
			Sha string
			Raw string
		}
		Truncated bool
	}
	if code, body := uiGet(t, s, "/ui/commits?net=7&base="+base+"&head="+h2+"&wait=1", &o); code != 200 {
		t.Fatalf("commits: %d %s", code, body)
	}
	if len(o.Commits) != 3 || o.Commits[0].Sha != h1 || o.Commits[1].Sha != m || o.Commits[2].Sha != h2 ||
		o.Truncated {
		t.Fatalf("commits: %+v, want %s %s %s", o, h1, m, h2)
	}
	raw, err := gitOut(wt, "", "cat-file", "commit", m)
	if err != nil {
		t.Fatal(err)
	}
	if o.Commits[1].Raw != string(raw) || strings.Count(o.Commits[1].Raw, "\nparent ") != 2 {
		t.Fatalf("merge raw %q, want %q", o.Commits[1].Raw, raw)
	}
	raw, err = gitOut(wt, "", "cat-file", "commit", h2)
	if err != nil {
		t.Fatal(err)
	}
	if want := strings.ReplaceAll(string(raw), "\xff", "\uFFFD"); o.Commits[2].Raw != want || want == string(raw) {
		t.Fatalf("non-UTF-8 raw %q, want %q", o.Commits[2].Raw, want)
	}

	// Nothing to list: head is behind base.
	if code, body := uiGet(t, s, "/ui/commits?net=7&base="+base+"&head="+base, &o); code != 200 ||
		len(o.Commits) != 0 || !strings.Contains(body, `"commits":[]`) {
		t.Fatalf("empty: %d %s", code, body)
	}
	missing := strings.Repeat("ab", 20)
	for q, code := range map[string]int{"base=" + missing + "&head=" + h2: 404, "base=" + base + "&head=" + missing: 404,
		"base=main&head=" + h2: 400} {
		if got, body := uiGet(t, s, "/ui/commits?net=7&"+q, nil); got != code {
			t.Fatalf("commits %s: %d %s, want %d", q, got, body, code)
		}
	}
}
