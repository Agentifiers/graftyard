package main

// The web app's read-only routes (/ui/*): ref resolution, trees, blame, diffs, compare, and READMEs. Every query
// value is untrusted: commit ids must match hexRe, paths pass uiPath, and every git command gets --end-of-options or
// -- before a revision or a path and runs with literal pathspecs, so a path is never an option, a glob, or magic.

import (
	"bytes"
	"fmt"
	"net/http"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"
)

var (
	abbrevRe = regexp.MustCompile(`^[0-9a-f]{7,64}$`)
	// literal makes every pathspec match its path byte for byte: no globs, no :(magic).
	literal = []string{"GIT_LITERAL_PATHSPECS=1"}
)

const (
	treeMaxEntries = 1000
	treeMaxWalk    = 5000
	countMax       = 100000
	blobMaxBytes   = 1 << 20
	blameMaxLines  = 20000
	diffMaxFiles   = 3000
	diffMaxPatch   = 5 << 20
	diffBatch      = 100
)

type commitAuthor struct {
	Login     *string `json:"login"`
	Name      string  `json:"name"`
	Email     string  `json:"email"`
	AvatarURL *string `json:"avatar_url"`
}

type commitSummary struct {
	Sha     string       `json:"sha"`
	Message string       `json:"message"`
	Date    string       `json:"date"`
	Author  commitAuthor `json:"author"`
}

// uiPath is a repository path from a query: "" (the root) or slash-separated names, none empty, "." or "..", that
// does not start with '-' and holds no NUL.
func uiPath(p string) bool {
	if p == "" {
		return true
	}
	if p[0] == '-' || strings.ContainsRune(p, 0) {
		return false
	}
	for _, s := range strings.Split(p, "/") {
		if s == "" || s == "." || s == ".." {
			return false
		}
	}
	return true
}

// commitOf is the commit rev names, peeled, or "" when there is none. rev is a full ref name or a full commit id.
func commitOf(rp, rev string) string {
	out, err := git(rp, nil, nil, "rev-parse", "--verify", "-q", "--end-of-options", rev+"^{commit}")
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

// treeEntry is one line of `git ls-tree -l`.
type treeEntry struct {
	mode, typ, oid, path string
	size                 int64
}

func parseLsTree(out []byte) []treeEntry {
	var es []treeEntry
	for _, rec := range strings.Split(string(out), "\x00") {
		meta, path, ok := strings.Cut(rec, "\t")
		f := strings.Fields(meta)
		if !ok || len(f) != 4 {
			continue
		}
		size, _ := strconv.ParseInt(f[3], 10, 64)
		es = append(es, treeEntry{mode: f[0], typ: f[1], oid: f[2], path: path, size: size})
	}
	return es
}

// entryAt is the tree entry at path p of commit c; ok is false when there is none.
func entryAt(rp, c, p string) (treeEntry, bool, error) {
	// ls-tree takes a "--" after --end-of-options as a path, so it gets none; p cannot start with '-' anyway.
	out, err := git(rp, literal, nil, "ls-tree", "-z", "-l", "--end-of-options", c, p)
	if err != nil {
		return treeEntry{}, false, err
	}
	for _, e := range parseLsTree(out) {
		if e.path == p {
			return e, true, nil
		}
	}
	return treeEntry{}, false, nil
}

// listDir is the entries of directory p ("" the root) of commit c, with full paths.
func listDir(rp, c, p string) ([]treeEntry, error) {
	args := []string{"ls-tree", "-z", "-l", "--end-of-options", c}
	if p != "" {
		args = append(args, p+"/")
	}
	out, err := git(rp, literal, nil, args...)
	if err != nil {
		return nil, err
	}
	return parseLsTree(out), nil
}

// dirAt answers 404 and false unless p is "" or a directory at commit c, which must be a commit.
func dirAt(w http.ResponseWriter, rp, c, p string) bool {
	if commitOf(rp, c) == "" {
		fail(w, 404, "no commit %s", c)
		return false
	}
	if p == "" {
		return true
	}
	e, ok, err := entryAt(rp, c, p)
	if err != nil {
		fail(w, 500, "%v", err)
		return false
	}
	if !ok || e.typ != "tree" {
		fail(w, 404, "%s is not a directory at %s", p, c)
		return false
	}
	return true
}

// summaries reads the CommitSummary of each id with one git log. ids must be full commit ids.
func summaries(rp string, ids []string) (map[string]commitSummary, error) {
	res := map[string]commitSummary{}
	var in strings.Builder
	var order []string
	for _, id := range ids {
		if _, seen := res[id]; !seen {
			res[id] = commitSummary{}
			order = append(order, id)
			in.WriteString(id + "\n")
		}
	}
	if len(order) == 0 {
		return res, nil
	}
	// %B cannot hold a NUL (git formats it as a C string), so -z and NUL-separated fields split unambiguously.
	out, err := git(rp, nil, strings.NewReader(in.String()), "log", "--no-walk=unsorted", "--stdin", "-z",
		"--format=%H%x00%cI%x00%an%x00%ae%x00%B")
	if err != nil {
		return nil, err
	}
	f := strings.Split(string(out), "\x00")
	if len(f) < 5*len(order) {
		return nil, fmt.Errorf("git log printed %d fields for %d commits", len(f), len(order))
	}
	for i := 0; i+4 < len(f); i += 5 {
		s := commitSummary{Sha: f[i], Date: f[i+1], Message: strings.TrimRight(f[i+4], "\n"),
			Author: commitAuthor{Name: f[i+2], Email: f[i+3]}}
		if _, want := res[s.Sha]; !want {
			return nil, fmt.Errorf("git log printed %q", s.Sha)
		}
		res[s.Sha] = s
	}
	return res, nil
}

// isBinary is git's own test: a NUL in the first 8000 bytes, or the diff attribute unset (as `binary` does) for p at c.
func isBinary(rp, c, p string, b []byte) bool {
	if bytes.IndexByte(b[:min(len(b), 8000)], 0) >= 0 {
		return true
	}
	out, err := git(rp, nil, nil, "check-attr", "-z", "--source="+c, "diff", "--", p)
	if err != nil {
		return false
	}
	f := strings.Split(string(out), "\x00")
	return len(f) >= 3 && f[2] == "unset"
}

// mergeBase is the merge base of a and b, "" when their histories share no commit.
func mergeBase(rp, a, b string) (string, error) {
	out, err := git(rp, nil, nil, "merge-base", "--end-of-options", a, b)
	if err != nil {
		// merge-base exits 1, printing nothing, when there is no merge base.
		if len(bytes.TrimSpace(out)) == 0 && strings.Contains(err.Error(), "exit status 1") {
			return "", nil
		}
		return "", err
	}
	return strings.TrimSpace(string(out)), nil
}

func handleUIResolve(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	splat := strings.TrimRight(r.URL.Query().Get("splat"), "/")
	if splat == "" || !uiPath(splat) {
		fail(w, 400, "bad splat")
		return
	}
	segs := strings.Split(splat, "/")
	l := lock(net)
	l.RLock()
	defer l.RUnlock()
	// Every prefix's branch and tag in one for-each-ref; its patterns also match longer names, so only exact
	// names count.
	args := []string{"for-each-ref", "--format=%(refname)", "--end-of-options"}
	for i := 1; i <= len(segs); i++ {
		p := strings.Join(segs[:i], "/")
		args = append(args, "refs/heads/"+p, "refs/tags/"+p)
	}
	out, err := git(rp, nil, nil, args...)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	refs := map[string]bool{}
	for _, ln := range strings.Split(string(out), "\n") {
		refs[ln] = true
	}
	var prefix, kind, sha string
	for i := len(segs); i >= 1 && sha == ""; i-- {
		prefix = strings.Join(segs[:i], "/")
		if refs["refs/heads/"+prefix] {
			kind, sha = "branch", commitOf(rp, "refs/heads/"+prefix)
		}
		if sha == "" && refs["refs/tags/"+prefix] {
			kind, sha = "tag", commitOf(rp, "refs/tags/"+prefix)
		}
		if sha == "" && abbrevRe.MatchString(prefix) {
			kind, sha = "commit", commitByPrefix(rp, prefix)
		}
	}
	if sha == "" {
		fail(w, 404, "no ref or commit names a prefix of %q", splat)
		return
	}
	path := strings.TrimPrefix(strings.TrimPrefix(splat, prefix), "/")
	var typ *string
	if path == "" {
		typ = ptr("dir")
	} else if e, ok, err := entryAt(rp, sha, path); err != nil {
		fail(w, 500, "%v", err)
		return
	} else if ok {
		typ = ptr(map[bool]string{true: "dir", false: "file"}[e.typ == "tree"])
	}
	var def *string
	if head, err := git(rp, nil, nil, "symbolic-ref", "-q", "HEAD"); err == nil {
		def = ptr(strings.TrimPrefix(strings.TrimSpace(string(head)), "refs/heads/"))
	}
	jsonOut(w, map[string]any{"ref": prefix, "path": path, "kind": kind, "sha": sha, "type": typ,
		"default_branch": def})
}

func ptr(s string) *string { return &s }

// commitByPrefix is the one commit whose id starts with the hex prefix, or "". It reads objects only, never refs, so
// a ref named like a sha cannot stand in for one.
func commitByPrefix(rp, prefix string) string {
	out, err := git(rp, nil, nil, "rev-parse", "--disambiguate="+prefix)
	if err != nil || len(bytes.TrimSpace(out)) == 0 {
		return ""
	}
	typed, err := git(rp, nil, bytes.NewReader(out), "cat-file", "--batch-check=%(objectname) %(objecttype)")
	if err != nil {
		return ""
	}
	found := ""
	for _, l := range strings.Split(string(typed), "\n") {
		if id, t, _ := strings.Cut(l, " "); t == "commit" {
			if found != "" {
				return "" // ambiguous
			}
			found = id
		}
	}
	return found
}

type treeOut struct {
	Name   string        `json:"name"`
	Path   string        `json:"path"`
	Type   string        `json:"type"`
	Commit commitSummary `json:"commit"`
	commit string
}

// sortEntries puts directories first, then files, each by name case-insensitively, then byte for byte.
func sortEntries(es []treeEntry) {
	sort.SliceStable(es, func(i, j int) bool {
		di, dj := es[i].typ == "tree", es[j].typ == "tree"
		if di != dj {
			return di
		}
		a, b := baseName(es[i].path), baseName(es[j].path)
		if la, lb := strings.ToLower(a), strings.ToLower(b); la != lb {
			return la < lb
		}
		return a < b
	})
}

func baseName(p string) string { return p[strings.LastIndexByte(p, '/')+1:] }

func handleUITree(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	q := r.URL.Query()
	c, p := q.Get("commit"), q.Get("path")
	if !hexRe.MatchString(c) || !uiPath(p) {
		fail(w, 400, "bad commit or path")
		return
	}
	l := lock(net)
	l.RLock()
	defer l.RUnlock()
	if !dirAt(w, rp, c, p) {
		return
	}
	es, err := listDir(rp, c, p)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	sortEntries(es)
	truncated := len(es) > treeMaxEntries
	if truncated {
		es = es[:treeMaxEntries]
	}
	spec := []string{"--"}
	if p != "" {
		spec = append(spec, p)
	}
	cnt, err := git(rp, literal, nil, append([]string{"rev-list", "--count", "--max-count=" + strconv.Itoa(countMax),
		"--end-of-options", c}, spec...)...)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	count, _ := strconv.Atoi(strings.TrimSpace(string(cnt)))

	// One walk of P's history, newest first: each entry gets the first commit that changes a path under it. -c lists
	// a merge's path only when the merge differs from every parent there, so a merge does not take the commits it
	// brought in.
	out, err := git(rp, literal, nil, append([]string{"log", "--format=%H", "--raw", "-c", "--no-renames", "-z",
		"--max-count=" + strconv.Itoa(treeMaxWalk), "--end-of-options", c}, spec...)...)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	pending := map[string]int{}
	for i, e := range es {
		pending[baseName(e.path)] = i
	}
	entries := make([]treeOut, len(es))
	latest := ""
	if p == "" {
		latest = c
	}
	cur, expectPath := "", false
	prefix := ""
	if p != "" {
		prefix = p + "/"
	}
	for _, tok := range strings.Split(string(out), "\x00") {
		if len(pending) == 0 {
			break
		}
		if expectPath {
			expectPath = false
			name, _, _ := strings.Cut(strings.TrimPrefix(tok, prefix), "/")
			if i, ok := pending[name]; ok && strings.HasPrefix(tok, prefix) {
				entries[i].commit = cur
				delete(pending, name)
			}
			continue
		}
		tok = strings.TrimPrefix(tok, "\n")
		switch {
		case tok == "":
		case tok[0] == ':':
			expectPath = true
		case hexRe.MatchString(tok):
			cur = tok
			if latest == "" {
				latest = tok
			}
		default:
			fail(w, 500, "git log printed %q", tok)
			return
		}
	}
	if latest == "" {
		latest = c
	}
	ids := []string{latest}
	for i := range entries {
		if entries[i].commit == "" {
			entries[i].commit = latest
		}
		ids = append(ids, entries[i].commit)
	}
	sums, err := summaries(rp, ids)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	for i, e := range es {
		t := "file"
		if e.typ == "tree" {
			t = "dir"
		}
		entries[i].Name, entries[i].Path, entries[i].Type = baseName(e.path), e.path, t
		entries[i].Commit = sums[entries[i].commit]
	}
	res := map[string]any{"latest": sums[latest], "count": count, "entries": entries}
	if truncated {
		res["truncated"] = true
	}
	jsonOut(w, res)
}

type blameRange struct {
	Start  int           `json:"start"`
	End    int           `json:"end"`
	Commit commitSummary `json:"commit"`
	sha    string
}

func handleUIBlame(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	q := r.URL.Query()
	c, p := q.Get("commit"), q.Get("path")
	if !hexRe.MatchString(c) || p == "" || !uiPath(p) {
		fail(w, 400, "bad commit or path")
		return
	}
	l := lock(net)
	l.RLock()
	defer l.RUnlock()
	if commitOf(rp, c) == "" {
		fail(w, 404, "no commit %s", c)
		return
	}
	e, ok, err := entryAt(rp, c, p)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	if !ok || e.typ == "tree" {
		fail(w, 404, "%s is not a file at %s", p, c)
		return
	}
	if e.typ != "blob" {
		fail(w, 422, "%s is a submodule", p)
		return
	}
	if e.size > blobMaxBytes {
		fail(w, 422, "%s is over %d bytes", p, blobMaxBytes)
		return
	}
	content, err := git(rp, nil, nil, "cat-file", "blob", e.oid)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	if isBinary(rp, c, p, content) {
		fail(w, 422, "%s is binary", p)
		return
	}
	lines := strings.Split(strings.TrimSuffix(string(content), "\n"), "\n")
	if len(content) == 0 {
		lines = []string{}
	}
	if len(lines) > blameMaxLines {
		fail(w, 422, "%s has over %d lines", p, blameMaxLines)
		return
	}
	// blame mistakes a path after --end-of-options and -- for a revision, so the (checked) commit goes bare and the
	// path after --.
	out, err := git(rp, nil, nil, "blame", "--porcelain", c, "--", p)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	// Each line's header is "<sha> <orig line> <final line> [<group size>]"; metadata lines start with a key that is
	// never hex, and content lines with a tab.
	owner := make([]string, len(lines))
	for _, l := range strings.Split(string(out), "\n") {
		if strings.HasPrefix(l, "\t") {
			continue
		}
		f := strings.Fields(l)
		if len(f) < 3 || !hexRe.MatchString(f[0]) {
			continue
		}
		n, err := strconv.Atoi(f[2])
		if err != nil || n < 1 || n > len(owner) {
			fail(w, 500, "blame printed %q", l)
			return
		}
		owner[n-1] = f[0]
	}
	ranges := []blameRange{}
	for i, sha := range owner {
		if sha == "" {
			fail(w, 500, "blame left line %d without a commit", i+1)
			return
		}
		if k := len(ranges) - 1; k >= 0 && ranges[k].sha == sha {
			ranges[k].End = i + 1
		} else {
			ranges = append(ranges, blameRange{Start: i + 1, End: i + 1, sha: sha})
		}
	}
	sums, err := summaries(rp, owner)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	var oldest, newest *string
	var ot, nt time.Time
	for i := range ranges {
		s := sums[ranges[i].sha]
		ranges[i].Commit = s
		t, err := time.Parse(time.RFC3339, s.Date)
		if err != nil {
			continue
		}
		if oldest == nil || t.Before(ot) {
			oldest, ot = ptr(s.Date), t
		}
		if newest == nil || t.After(nt) {
			newest, nt = ptr(s.Date), t
		}
	}
	jsonOut(w, map[string]any{"path": p, "lines": lines, "ranges": ranges, "oldest": oldest, "newest": newest})
}

type diffFile struct {
	Filename         string `json:"filename"`
	PreviousFilename string `json:"previous_filename,omitempty"`
	Status           string `json:"status"`
	Additions        int    `json:"additions"`
	Deletions        int    `json:"deletions"`
	Changes          int    `json:"changes"`
	Sha              string `json:"sha"`
	Patch            string `json:"patch,omitempty"`
	oldOid, newOid   string
	typeChange, bin  bool
}

// diffFiles parses `git diff -z --raw --numstat`: the raw records, then the numstat records in the same order.
func diffFiles(out []byte) ([]diffFile, error) {
	tok := strings.Split(string(out), "\x00")
	var fs []diffFile
	i := 0
	for ; i < len(tok) && strings.HasPrefix(tok[i], ":"); i++ {
		h := strings.Fields(tok[i][1:])
		if len(h) != 5 || i+1 >= len(tok) {
			return nil, fmt.Errorf("diff printed %q", tok[i])
		}
		f := diffFile{oldOid: h[2], newOid: h[3], Sha: h[3]}
		switch st := h[4][0]; st {
		case 'R', 'C':
			if i+2 >= len(tok) {
				return nil, fmt.Errorf("diff printed %q", tok[i])
			}
			f.PreviousFilename, f.Filename = tok[i+1], tok[i+2]
			f.Status = map[byte]string{'R': "renamed", 'C': "copied"}[st]
			i += 2
		default:
			f.Filename = tok[i+1]
			i++
			switch st {
			case 'A':
				f.Status = "added"
			case 'D':
				f.Status, f.Sha = "removed", h[2]
			case 'T':
				f.Status, f.typeChange = "changed", true
			default:
				f.Status = "modified"
				if h[2] == h[3] {
					f.Status = "changed" // the mode alone changed
				}
			}
		}
		fs = append(fs, f)
	}
	for k := range fs {
		if i >= len(tok) {
			return nil, fmt.Errorf("diff printed %d numstat records for %d files", k, len(fs))
		}
		n := strings.SplitN(tok[i], "\t", 3)
		if len(n) != 3 {
			return nil, fmt.Errorf("diff printed %q", tok[i])
		}
		i++
		if n[2] == "" {
			i += 2 // a rename's old and new path follow
		}
		if n[0] == "-" {
			fs[k].bin = true
			continue
		}
		a, err1 := strconv.Atoi(n[0])
		d, err2 := strconv.Atoi(n[1])
		if err1 != nil || err2 != nil {
			return nil, fmt.Errorf("diff printed %q", tok[i-1])
		}
		fs[k].Additions, fs[k].Deletions, fs[k].Changes = a, d, a+d
	}
	return fs, nil
}

// patches splits a `git diff --full-index` patch into each file's hunks, keyed by "<old oid>..<new oid>" from the
// file's index line. A line starting "diff --git " is always a file header: hunk lines start with ' ', '+', '-',
// or '\'. A file without hunks (binary, mode only, an exact rename) has no key.
func patches(out []byte) map[string]string {
	res := map[string]string{}
	secs := strings.Split("\n"+string(out), "\ndiff --git ")
	for _, s := range secs[1:] {
		at := strings.Index(s, "\n@@")
		if at < 0 {
			continue
		}
		for _, l := range strings.Split(s[:at], "\n") {
			if f := strings.Fields(l); len(f) >= 2 && f[0] == "index" {
				res[f[1]] = strings.TrimSuffix(s[at+1:], "\n")
				break
			}
		}
	}
	return res
}

func handleUIDiff(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	q := r.URL.Query()
	base, head := q.Get("base"), q.Get("head")
	if !hexRe.MatchString(base) || !hexRe.MatchString(head) {
		fail(w, 400, "bad base or head")
		return
	}
	l := lock(net)
	l.RLock()
	defer l.RUnlock()
	if commitOf(rp, base) == "" || commitOf(rp, head) == "" {
		fail(w, 404, "no commit %s or %s", base, head)
		return
	}
	mb, err := mergeBase(rp, base, head)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	if mb == "" {
		fail(w, 422, "%s and %s share no history", base, head)
		return
	}
	diff := []string{"-c", "core.quotePath=false", "diff", "-M", "--no-color", "--no-ext-diff", "--no-textconv"}
	out, err := git(rp, literal, nil, append(diff, "-z", "--raw", "--numstat", "--no-abbrev", "--end-of-options",
		mb, head, "--")...)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	files, err := diffFiles(out)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	truncated := len(files) > diffMaxFiles
	if truncated {
		files = files[:diffMaxFiles]
	}
	// Patches in batches of files, limited to their paths (both sides of a rename, so it pairs again), stopping at
	// the byte cap. A changed line is at least 2 bytes ("+\n"), so a file that cannot fit is not diffed at all.
	used := 0
	for start := 0; start < len(files) && used <= diffMaxPatch; {
		var batch []int
		spec := []string{}
		for ; start < len(files) && len(batch) < diffBatch; start++ {
			f := &files[start]
			if f.bin || f.Changes == 0 {
				continue
			}
			batch = append(batch, start)
			spec = append(spec, f.Filename)
			if f.PreviousFilename != "" {
				spec = append(spec, f.PreviousFilename)
			}
		}
		if len(batch) == 0 {
			break
		}
		out, err := git(rp, literal, nil, append(append(diff, "--full-index", "--end-of-options", mb, head, "--"),
			spec...)...)
		if err != nil {
			fail(w, 500, "%v", err)
			return
		}
		ps := patches(out)
		for _, k := range batch {
			f := &files[k]
			if used+2*f.Changes > diffMaxPatch {
				used = diffMaxPatch + 1
				truncated = true
				break
			}
			p := ps[f.oldOid+".."+f.newOid]
			if f.typeChange {
				// A type change is two patches: the old entry's removal, then the new one's addition.
				z := strings.Repeat("0", len(f.oldOid))
				p = strings.Trim(ps[f.oldOid+".."+z]+"\n"+ps[z+".."+f.newOid], "\n")
			}
			if used+len(p) > diffMaxPatch {
				used = diffMaxPatch + 1
				truncated = true
				break
			}
			used += len(p)
			f.Patch = p
		}
	}
	if files == nil {
		files = []diffFile{}
	}
	jsonOut(w, map[string]any{"files": files, "truncated": truncated})
}

func handleUICompare(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	q := r.URL.Query()
	base, head := q.Get("base"), q.Get("head")
	if !hexRe.MatchString(base) || !hexRe.MatchString(head) {
		fail(w, 400, "bad base or head")
		return
	}
	l := lock(net)
	l.RLock()
	defer l.RUnlock()
	if commitOf(rp, base) == "" || commitOf(rp, head) == "" {
		fail(w, 404, "no commit %s or %s", base, head)
		return
	}
	mb, err := mergeBase(rp, base, head)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	out, err := git(rp, nil, nil, "rev-list", "--left-right", "--count", "--end-of-options", base+"..."+head, "--")
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	f := strings.Fields(string(out))
	if len(f) != 2 {
		fail(w, 500, "rev-list printed %q", out)
		return
	}
	behind, _ := strconv.Atoi(f[0])
	ahead, _ := strconv.Atoi(f[1])
	var m *string
	if mb != "" {
		m = &mb
	}
	jsonOut(w, map[string]any{"merge_base": m, "ahead_by": ahead, "behind_by": behind})
}

const prCommitsMax = 250 // GitHub lists at most 250 of a pull request's commits

type rawCommit struct {
	Sha string `json:"sha"`
	Raw string `json:"raw"`
}

// handleUICommits is a pull request's commits: those H reaches and B does not, oldest first, raw.
func handleUICommits(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	q := r.URL.Query()
	base, head := q.Get("base"), q.Get("head")
	if !hexRe.MatchString(base) || !hexRe.MatchString(head) {
		fail(w, 400, "bad base or head")
		return
	}
	l := lock(net)
	l.RLock()
	defer l.RUnlock()
	if commitOf(rp, base) == "" || commitOf(rp, head) == "" {
		fail(w, 404, "no commit %s or %s", base, head)
		return
	}
	// --max-count would keep the newest, before --reverse, so the whole list is read and its oldest kept.
	out, err := git(rp, nil, nil, "rev-list", "--reverse", "--topo-order", "--end-of-options", base+".."+head, "--")
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	ids := strings.Fields(string(out))
	truncated := len(ids) > prCommitsMax
	if truncated {
		ids = ids[:prCommitsMax]
	}
	commits := []rawCommit{}
	if len(ids) > 0 {
		out, err = git(rp, nil, strings.NewReader(strings.Join(ids, "\n")+"\n"), "cat-file", "--batch")
		if err != nil {
			fail(w, 500, "%v", err)
			return
		}
		// Each object is "<oid> <type> <size>\n", its size bytes, and "\n".
		for _, id := range ids {
			hdr, rest, ok := bytes.Cut(out, []byte("\n"))
			f := strings.Fields(string(hdr))
			if !ok || len(f) != 3 || f[0] != id || f[1] != "commit" {
				fail(w, 500, "cat-file printed %q for %s", hdr, id)
				return
			}
			n, err := strconv.Atoi(f[2])
			if err != nil || n < 0 || n+1 > len(rest) || rest[n] != '\n' {
				fail(w, 500, "cat-file printed a bad size %q for %s", f[2], id)
				return
			}
			commits = append(commits, rawCommit{Sha: id, Raw: strings.ToValidUTF8(string(rest[:n]), "\uFFFD")})
			out = rest[n+1:]
		}
	}
	jsonOut(w, map[string]any{"commits": commits, "truncated": truncated})
}

// readmeRank orders README names the way GitHub picks one; any other README.* ranks after these.
var readmeRank = []string{"readme.md", "readme.markdown", "readme.rst", "readme.txt", "readme"}

func handleUIReadme(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	q := r.URL.Query()
	c, p := q.Get("commit"), q.Get("path")
	if !hexRe.MatchString(c) || !uiPath(p) {
		fail(w, 400, "bad commit or path")
		return
	}
	l := lock(net)
	l.RLock()
	defer l.RUnlock()
	if !dirAt(w, rp, c, p) {
		return
	}
	es, err := listDir(rp, c, p)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	sortEntries(es)
	var best *treeEntry
	bestRank := len(readmeRank)
	for i := range es {
		e := &es[i]
		name := strings.ToLower(baseName(e.path))
		// A symlink is a blob too, but its content is its target, not a README.
		if e.typ != "blob" || e.mode == "120000" || (name != "readme" && !strings.HasPrefix(name, "readme.")) {
			continue
		}
		rank := len(readmeRank)
		for k, n := range readmeRank {
			if name == n {
				rank = k
			}
		}
		if best == nil || rank < bestRank {
			best, bestRank = e, rank
		}
	}
	if best == nil {
		fail(w, 404, "no README in %q at %s", p, c)
		return
	}
	if best.size > blobMaxBytes {
		fail(w, 422, "%s is over %d bytes", best.path, blobMaxBytes)
		return
	}
	content, err := git(rp, nil, nil, "cat-file", "blob", best.oid)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	if isBinary(rp, c, best.path, content) {
		fail(w, 422, "%s is binary", best.path)
		return
	}
	jsonOut(w, map[string]any{"name": baseName(best.path), "path": best.path, "sha": best.oid, "size": best.size,
		"text": string(content)})
}

// handleUIBlob is the text of regular file P at commit C, for the Actions trigger's workflow reads.
func handleUIBlob(w http.ResponseWriter, r *http.Request) {
	net, rp, ok := netParam(w, r, true)
	if !ok {
		return
	}
	q := r.URL.Query()
	c, p := q.Get("commit"), q.Get("path")
	if !hexRe.MatchString(c) || p == "" || !uiPath(p) {
		fail(w, 400, "bad commit or path")
		return
	}
	l := lock(net)
	l.RLock()
	defer l.RUnlock()
	if commitOf(rp, c) == "" {
		fail(w, 404, "no commit %s", c)
		return
	}
	e, ok, err := entryAt(rp, c, p)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	// A symlink is a blob too, but its content is its target, not a file's text.
	if !ok || e.typ != "blob" || e.mode == "120000" {
		fail(w, 404, "%s is not a regular file at %s", p, c)
		return
	}
	if e.size > blobMaxBytes {
		fail(w, 422, "%s is over %d bytes", p, blobMaxBytes)
		return
	}
	content, err := git(rp, nil, nil, "cat-file", "blob", e.oid)
	if err != nil {
		fail(w, 500, "%v", err)
		return
	}
	if isBinary(rp, c, p, content) {
		fail(w, 422, "%s is binary", p)
		return
	}
	jsonOut(w, map[string]any{"path": p, "sha": e.oid, "size": e.size, "text": string(content)})
}
