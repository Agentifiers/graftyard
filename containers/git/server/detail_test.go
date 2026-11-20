package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// /validate's detail for the agent layer: an update's new commits, raw, with their trailers; the union of its paths
// against the merge base with HEAD and against the old tip; fast-forward or not; refs/notes/ai's entries; and the
// detail-only form a REST ref write sends with no pack.

type detailOut struct {
	Checksum string      `json:"checksum"`
	Detail   []refDetail `json:"detail"`
}

func validateDetail(t *testing.T, url string, tips []string, updates []update, pack []byte) (int, detailOut, string) {
	t.Helper()
	head := ""
	if len(tips) > 0 {
		head = strings.Join(tips, "\n") + "\n"
	}
	var d strings.Builder
	for _, u := range updates {
		fmt.Fprintf(&d, "%s %s %s\n", u.New, u.Old, u.Ref)
	}
	body := append([]byte(head+d.String()), pack...)
	q := fmt.Sprintf("/validate?net=7&tipsn=%d&detailn=%d", len(head), d.Len())
	r, err := http.Post(url+q, "", bytes.NewReader(body))
	if err != nil {
		t.Fatal(err)
	}
	defer r.Body.Close()
	var b bytes.Buffer
	b.ReadFrom(r.Body)
	var o detailOut
	json.Unmarshal(b.Bytes(), &o)
	return r.StatusCode, o, b.String()
}

// commitMsg writes files over the work tree's and commits them with msg, returning the commit.
func commitMsg(t *testing.T, wt string, files map[string]string, msg string) string {
	t.Helper()
	for p, c := range files {
		os.MkdirAll(filepath.Dir(filepath.Join(wt, p)), 0o755)
		if err := os.WriteFile(filepath.Join(wt, p), []byte(c), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	run(t, wt, "", "add", "-A")
	run(t, wt, "", "commit", "-qm", msg)
	return run(t, wt, "", "rev-parse", "HEAD")
}

func TestValidateDetail(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commitMsg(t, wt, map[string]string{"pkg/a.go": "a\n", "README": "r\n"}, "base")
	run(t, wt, "", "push", "-q", repo("7"), "main")
	run(t, repo("7"), "", "symbolic-ref", "HEAD", "refs/heads/main")

	// Two agent commits on claude/x: one changes a workflow, the last carries a session trailer.
	run(t, wt, "", "checkout", "-qb", "claude/x")
	commitMsg(t, wt, map[string]string{".github/workflows/ci.yml": "on: push\n"}, "ci")
	tip := commitMsg(t, wt, map[string]string{"pkg/b.go": "b\n"},
		"work\n\nGraftyard-Session: https://h/o/r/_graftyard/sessions/s1\nCo-authored-by: A <a@a>")
	pack := packOfWt(t, wt, tip, base)
	z := strings.Repeat("0", 40)
	code, o, body := validateDetail(t, s.URL, []string{tip}, []update{{tip, z, "refs/heads/claude/x"}}, pack)
	if code != 200 || o.Checksum == "" || len(o.Detail) != 1 {
		t.Fatalf("validate: %d %s", code, body)
	}
	d := o.Detail[0]
	if len(d.Commits) != 2 || !bytes.Contains(d.Commits[0].Raw, []byte("Graftyard-Session: https://h/o/r/_graftyard/sessions/s1")) {
		t.Fatalf("commits: %q", d.Commits)
	}
	if !bytes.HasPrefix(d.Commits[0].Raw, []byte("tree ")) {
		t.Fatalf("not a raw commit: %q", d.Commits[0])
	}
	if d.Commits[0].Oid != tip {
		t.Fatalf("newest first: %s, tip %s", d.Commits[0].Oid, tip)
	}
	if strings.Join(d.Paths, ",") != ".github/workflows/ci.yml,pkg/b.go" || d.FF || d.MergeBase != base {
		t.Fatalf("paths %v ff %v base %s", d.Paths, d.FF, d.MergeBase)
	}

	// The same tip over an old one it descends from: a fast-forward, and the old tip's diff adds nothing new.
	code, o, body = validateDetail(t, s.URL, []string{tip}, []update{{tip, base, "refs/heads/claude/x"}}, pack)
	if code != 200 || len(o.Detail) != 1 || !o.Detail[0].FF || len(o.Detail[0].Paths) != 2 {
		t.Fatalf("ff: %d %s", code, body)
	}

	// A REST ref write: no pack, detail from the repository's own objects. base as a new ref has no new commits and
	// no paths against HEAD.
	code, o, body = validateDetail(t, s.URL, nil, []update{{base, z, "refs/heads/claude/y"}}, nil)
	if code != 200 || len(o.Detail) != 1 || len(o.Detail[0].Commits) != 0 || len(o.Detail[0].Paths) != 0 {
		t.Fatalf("detail only: %d %s", code, body)
	}

	// A non-fast-forward: the old tip is not an ancestor of the new one, and both diffs count.
	run(t, wt, "", "checkout", "-q", "main")
	run(t, wt, "", "checkout", "-qb", "other")
	other := commitMsg(t, wt, map[string]string{"docs/x.md": "x\n"}, "other")
	run(t, wt, "", "push", "-q", repo("7"), "claude/x")
	code, o, body = validateDetail(t, s.URL, []string{other}, []update{{other, tip, "refs/heads/claude/x"}},
		packOfWt(t, wt, other, base))
	if code != 200 || len(o.Detail) != 1 || o.Detail[0].FF {
		t.Fatalf("non-ff: %d %s", code, body)
	}
	if got := strings.Join(o.Detail[0].Paths, ","); got != ".github/workflows/ci.yml,docs/x.md,pkg/b.go" {
		t.Fatalf("non-ff paths %s", got)
	}

	// refs/notes/ai: each changed note blob, by its fan-out path.
	run(t, wt, "", "notes", "--ref=ai", "add", "-m", "session log", other)
	notes := run(t, wt, "", "rev-parse", "refs/notes/ai")
	code, o, body = validateDetail(t, s.URL, []string{notes}, []update{{notes, z, "refs/notes/ai"}},
		packOfWt(t, wt, notes, base))
	if code != 200 || len(o.Detail) != 1 || len(o.Detail[0].Notes) != 1 || o.Detail[0].Notes[0].Path != other ||
		!o.Detail[0].Notes[0].Blob {
		t.Fatalf("notes: %d %s", code, body)
	}

	// A bad update line is refused before anything is indexed: a bad oid, or a name outside refs/.
	if code, _, _ := validateDetail(t, s.URL, nil, []update{{"x", base, "refs/heads/a"}}, nil); code != 400 {
		t.Fatalf("a bad update line answered %d", code)
	}
	if code, _, _ := validateDetail(t, s.URL, nil, []update{{base, z, "heads/a"}}, nil); code != 400 {
		t.Fatalf("an update outside refs/ answered %d", code)
	}

	// A delete has no new tip, so it gets no detail; the create beside it does.
	code, o, body = validateDetail(t, s.URL, nil, []update{{z, base, "refs/heads/claude/gone"}, {base, z, "refs/heads/claude/y2"}}, nil)
	if code != 200 || len(o.Detail) != 1 || o.Detail[0].Ref != "refs/heads/claude/y2" {
		t.Fatalf("delete beside a create: %d %s", code, body)
	}
}

// packOfWt packs what tip reaches and base does not, from the work tree, as a client's push would.
func packOfWt(t *testing.T, wt, tip, base string) []byte {
	t.Helper()
	out, err := gitOut(wt, tip+"\n^"+base+"\n", "pack-objects", "--stdout", "--revs", "--thin")
	if err != nil {
		t.Fatal(err)
	}
	return out
}

func gitOut(dir, stdin string, args ...string) ([]byte, error) {
	return git(dir, nil, strings.NewReader(stdin), args...)
}

// A body whose detail or pack changed between the edge and the container is not the pack's answer: with headsum and
// packsum in the URL, the container answers 502 (the shard's 503, which moves the push on), never the 400 "bad update
// line" that became `unpacker error` on B0-32 m3fpc1 and m3fr13 (decisions/OPEN-QUESTIONS.md, G35). The same body
// unchanged still validates.
func TestValidateBodyChangedInTransit(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	base := commitMsg(t, wt, map[string]string{"README": "r\n"}, "base")
	run(t, wt, "", "push", "-q", repo("7"), "main")
	tip := commitMsg(t, wt, map[string]string{"a.txt": "a\n"}, "work")
	pack := packOfWt(t, wt, tip, base)
	z := strings.Repeat("0", 40)
	head := tip + "\n"
	detail := tip + " " + z + " refs/heads/claude/x\n"
	sum := func(b []byte) string { h := sha256.Sum256(b); return hex.EncodeToString(h[:]) }
	post := func(body []byte, headsum, packsum string) (int, string) {
		q := fmt.Sprintf("/validate?net=7&tipsn=%d&detailn=%d&headsum=%s&packsum=%s", len(head), len(detail), headsum, packsum)
		r, err := http.Post(s.URL+q, "", bytes.NewReader(body))
		if err != nil {
			t.Fatal(err)
		}
		defer r.Body.Close()
		var b bytes.Buffer
		b.ReadFrom(r.Body)
		return r.StatusCode, b.String()
	}
	hs, ps := sum([]byte(head+detail)), sum(pack)
	// The m3fpc1 shape: pack bytes where the detail lines were framed, same lengths.
	garbled := append([]byte(head), pack[:len(detail)]...)
	garbled = append(garbled, pack...)
	if code, body := post(garbled, hs, ps); code != 502 || !strings.Contains(body, "changed in transit") {
		t.Fatalf("garbled detail answered %d %s", code, body)
	}
	// A pack byte changed after an intact head.
	bad := append([]byte(head+detail), pack...)
	bad[len(head)+len(detail)+20] ^= 0xff
	if code, body := post(bad, hs, ps); code != 502 {
		t.Fatalf("changed pack answered %d %s", code, body)
	}
	if code, body := post(append([]byte(head+detail), pack...), hs, ps); code != 200 {
		t.Fatalf("intact body answered %d %s", code, body)
	}
}
