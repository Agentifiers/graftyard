package main

import (
	"encoding/json"
	"fmt"
	"strings"
	"testing"
)

// disk=1: a validate checked against the disk as it stands, before the shard catches up. A base the disk lacks, or
// refs older than the push's detail needs, answers 409 missingBase and keeps the request; held=<id> checks it again
// after the catch-up. Every other failure is still the pack's 422.

type heldOut struct {
	MissingBase bool   `json:"missingBase"`
	Held        string `json:"held"`
}

func validateDisk(t *testing.T, url, q string, tips []string, updates []update, pack []byte) (int, detailOut, heldOut, string) {
	t.Helper()
	head := strings.Join(tips, "\n") + "\n"
	var d strings.Builder
	for _, u := range updates {
		fmt.Fprintf(&d, "%s %s %s\n", u.New, u.Old, u.Ref)
	}
	body := append([]byte(head+d.String()), pack...)
	code, b := postBody(t, fmt.Sprintf("%s/validate?net=7&tipsn=%d&detailn=%d%s", url, len(head), d.Len(), q), body)
	var o detailOut
	var h heldOut
	json.Unmarshal(b, &o)
	json.Unmarshal(b, &h)
	return code, o, h, string(b)
}

func TestValidateOnTheDiskAsItStands(t *testing.T) {
	s := serve(t)
	call(t, s, "POST", "/init?net=7")
	wt := t.TempDir()
	run(t, wt, "", "init", "-q", "-b", "main")
	commitMsg(t, wt, map[string]string{"pkg/a.go": "a\n"}, "base")
	run(t, wt, "", "push", "-q", repo("7"), "main")
	run(t, repo("7"), "", "symbolic-ref", "HEAD", "refs/heads/main")
	z := strings.Repeat("0", 40)

	// Another push moved main to c1, which this disk has not caught up to; the agent's push builds on c1.
	c1 := commitMsg(t, wt, map[string]string{"pkg/a.go": "a1\n"}, "c1")
	run(t, wt, "", "checkout", "-qb", "claude/x")
	tip := commitMsg(t, wt, map[string]string{"pkg/b.go": "b\n"}, "work")
	pack := packOfWt(t, wt, tip, c1)
	up := []update{{tip, z, "refs/heads/claude/x"}}

	// Without disk=1 a missing base is the pack's refusal, as before.
	if code, _, _, body := validateDisk(t, s.URL, "", []string{tip}, up, pack); code != 422 {
		t.Fatalf("a missing base without disk=1 answered %d %s", code, body)
	}
	code, _, h, body := validateDisk(t, s.URL, "&disk=1", []string{tip}, up, pack)
	if code != 409 || !h.MissingBase || !heldRe.MatchString(h.Held) {
		t.Fatalf("a missing base with disk=1 answered %d %s", code, body)
	}

	// A truncated pack is never a missing base: the disk catching up would not change it.
	if code, _, _, body := validateDisk(t, s.URL, "&disk=1", []string{tip}, up, pack[:len(pack)/2]); code != 422 {
		t.Fatalf("a truncated pack with disk=1 answered %d %s", code, body)
	}

	// A held request asked again before any catch-up brought its base is the pack's refusal: the retry runs as on a
	// caught-up disk, so it never holds again.
	if code, _, h0, body := validateDisk(t, s.URL, "&disk=1", []string{tip}, up, pack); code != 409 {
		t.Fatalf("hold for the early retry: %d %s", code, body)
	} else if code, _, _, body := validateDisk(t, s.URL, "&held="+h0.Held, nil, nil, nil); code != 422 {
		t.Fatalf("a held retry still missing its base answered %d %s", code, body)
	}

	// The window inside a catch-up page: c1's objects ingested, refs not yet applied. The pack now indexes and
	// connects, but its detail would read main at base, so it is held too.
	run(t, wt, "", "push", "-q", repo("7"), c1+":refs/tmp/c1")
	run(t, repo("7"), "", "update-ref", "-d", "refs/tmp/c1")
	if code, _, h2, body := validateDisk(t, s.URL, "&disk=1", []string{tip}, up, pack); code != 409 || !h2.MissingBase {
		t.Fatalf("refs behind the push answered %d %s", code, body)
	}

	// The catch-up applies main = c1; the held request is checked again with no body and answers as a caught-up
	// validate does, with the merge base the leader's refs give.
	run(t, wt, "", "push", "-q", repo("7"), "main")
	code, o, _, body := validateDisk(t, s.URL, "&held="+h.Held, nil, nil, nil)
	if code != 200 || o.Checksum == "" || len(o.Detail) != 1 || o.Detail[0].MergeBase != c1 ||
		strings.Join(o.Detail[0].Paths, ",") != "pkg/b.go" {
		t.Fatalf("the held validate after the catch-up: %d %s", code, body)
	}
	// A held request is answered once.
	if code, _, _, body := validateDisk(t, s.URL, "&held="+h.Held, nil, nil, nil); code != 503 {
		t.Fatalf("a held request asked twice answered %d %s", code, body)
	}

	// A REST ref write's detail against an old tip the disk does not show yet is held, not refused as a
	// non-fast-forward; once the ref is there it answers ff.
	run(t, wt, "", "checkout", "-q", "main")
	c2 := commitMsg(t, wt, map[string]string{"pkg/a.go": "a2\n"}, "c2")
	run(t, wt, "", "push", "-q", repo("7"), c2+":refs/tmp/c2")
	run(t, repo("7"), "", "update-ref", "-d", "refs/tmp/c2")
	rest := []update{{c2, c1, "refs/heads/claude/y"}}
	if code, _, h3, body := validateDisk(t, s.URL, "&disk=1", nil, rest, nil); code != 409 || !h3.MissingBase {
		t.Fatalf("a REST write on an older ref answered %d %s", code, body)
	}
	// With the ref current but c2 reachable from no ref the disk holds, the disk may still lack the ref that names
	// it, so it is held as well; once main names c2 the disk is current for this write, which answers ff.
	run(t, repo("7"), "", "update-ref", "refs/heads/claude/y", c1)
	if code, _, h4, body := validateDisk(t, s.URL, "&disk=1", nil, rest, nil); code != 409 || !h4.MissingBase {
		t.Fatalf("a REST write whose tip no ref names answered %d %s", code, body)
	}
	run(t, repo("7"), "", "update-ref", "refs/heads/main", c2)
	if code, o, _, body := validateDisk(t, s.URL, "&disk=1", nil, rest, nil); code != 200 || len(o.Detail) != 1 || !o.Detail[0].FF {
		t.Fatalf("a REST write on a current disk answered %d %s", code, body)
	}
	// Every object present, but the disk's claude/y is not the old tip the write names: its refs are behind.
	run(t, repo("7"), "", "update-ref", "refs/heads/claude/y", c2)
	if code, _, h5, body := validateDisk(t, s.URL, "&disk=1", nil, rest, nil); code != 409 || !h5.MissingBase {
		t.Fatalf("a stale old tip answered %d %s", code, body)
	}
}

// git 2.56's wording for an object the disk lacks is a missing base; a truncated or malformed pack is not.
func TestMissingBaseWording(t *testing.T) {
	for msg, want := range map[string]bool{
		"git index-pack: exit status 128: fatal: pack has 1 unresolved delta":                    true,
		"git index-pack: exit status 128: fatal: did not receive expected object 6649c11221b4c0": true,
		"git rev-list: exit status 128: fatal: bad object 8e194f40412cacfa70277441c34bcb2fd4d3":  true,
		"git rev-list: exit status 128: fatal: missing blob object '78981922613b2afb6025042ff6'": true,
		"git rev-list: exit status 128: fatal: bad tree object aaff74984cccd156a469afa7d9ab10e":  true,
		"git index-pack: exit status 128: fatal: early EOF":                                      false,
		"git index-pack: exit status 128: error: object 0f8b: missingEmail: invalid author":      false,
		"git index-pack: exit status 128: fatal: pack signature mismatch":                        false,
	} {
		if got := missingBase(fmt.Errorf("%s", msg)); got != want {
			t.Errorf("missingBase(%q) = %v, want %v", msg, got, want)
		}
	}
}
