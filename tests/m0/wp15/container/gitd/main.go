// gitd: M0-WP10's git cache stand-in, run in the `AgentRepo` container (or locally for tests). Every git fact the
// Worker's `validate` and `decide` use comes from real git here: index-pack --strict into a quarantine, ancestry,
// tree diffs against the merge base and the old tip, the new commits' raw bytes, and `refs/notes/ai` entries.
//
//	GET  /health
//	POST /init        {repo, alternates?}                 bare repository /data/<repo>.git
//	POST /pack?repo=R&id=Q                                body is a pack; index-pack --strict --fix-thin into /data/q/Q
//	POST /inspect     {repo, id, updates, defaultTip}     facts per update, read through the quarantine
//	POST /commit      {repo, id, parent, add, del, message, author}   createCommitOnBranch: a commit in the quarantine
//	POST /finalize    {repo, id, updates}                 move quarantined packs in, update-ref --stdin as one transaction
//	POST /discard     {id}
//	GET  /advertise?repo=R&proto=0|1|2&service=upload-pack   git's own advertisement, refs/graftyard/ hidden
//	POST /upload-pack?repo=R&proto=0|1|2&hide=1|0             stateless-rpc
package main

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
)

var root = envOr("GITD_ROOT", "/data")

func envOr(k, d string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return d
}

var safe = regexp.MustCompile(`^[A-Za-z0-9._-]+$`)

func repoDir(r string) string { return filepath.Join(root, safeName(r)+".git") }
func qDir(id string) string   { return filepath.Join(root, "q", safeName(id)) }
func safeName(s string) string {
	s = strings.ReplaceAll(s, "/", "__")
	if !safe.MatchString(s) {
		panic("bad name " + s)
	}
	return s
}

const zero = "0000000000000000000000000000000000000000"
const emptyTree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"

// git runs git in repo with the quarantine (when id != "") as the object directory and the repository's own objects
// as an alternate, as receive-pack's tmp_objdir does.
func git(repo, id string, stdin []byte, args ...string) ([]byte, []byte, error) {
	cmd := exec.Command("git", append([]string{"--git-dir=" + repoDir(repo)}, args...)...)
	cmd.Env = append(os.Environ(), "GIT_CONFIG_NOSYSTEM=1", "HOME=/tmp")
	if id != "" {
		q := qDir(id)
		cmd.Env = append(cmd.Env, "GIT_OBJECT_DIRECTORY="+q, "GIT_ALTERNATE_OBJECT_DIRECTORIES="+filepath.Join(repoDir(repo), "objects"))
	}
	if stdin != nil {
		cmd.Stdin = bytes.NewReader(stdin)
	}
	var out, errb bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errb
	err := cmd.Run()
	return out.Bytes(), errb.Bytes(), err
}

func trim(b []byte) string { return strings.TrimSpace(string(b)) }

type update struct {
	Name string  `json:"name"`
	Old  *string `json:"old"`
	New  *string `json:"new"`
}

type fact struct {
	Name      string   `json:"name"`
	Missing   bool     `json:"missing"`
	Ff        bool     `json:"ff"`
	Type      string   `json:"type"`
	MergeBase string   `json:"mergeBase"`
	Paths     []string `json:"paths"`
	Notes     []note   `json:"notes,omitempty"`
	// M0-WP13: which of the asked-for heads (open agent PRs into this ref) the new tip contains.
	Contains []string `json:"contains"`
}
type note struct {
	Path  string `json:"path"`
	Mode  string `json:"mode"`
	Type  string `json:"type"`
	Oid   string `json:"oid"`
	Bytes string `json:"bytes,omitempty"` // base64, at most noteMax
}
type commitRaw struct {
	Oid string `json:"oid"`
	Raw string `json:"raw"` // base64
}

const noteMax = 64 << 10
const commitsMax = 2000

func splitZ(b []byte) []string {
	var out []string
	for _, s := range strings.Split(string(b), "\x00") {
		if s != "" {
			out = append(out, s)
		}
	}
	return out
}

func inspect(repo, id string, updates []update, defaultTip string, heads map[string][]string) (map[string]any, error) {
	os.MkdirAll(filepath.Join(qDir(id), "pack"), 0o755)
	facts := []fact{}
	var tips []string
	for _, u := range updates {
		f := fact{Name: u.Name, Paths: []string{}}
		if u.New == nil || *u.New == zero {
			f.Ff = true
			facts = append(facts, f)
			continue
		}
		nw := *u.New
		t, _, err := git(repo, id, nil, "cat-file", "-t", nw)
		if err != nil {
			f.Missing = true
			facts = append(facts, f)
			continue
		}
		f.Type = trim(t)
		tips = append(tips, nw)
		old := ""
		if u.Old != nil && *u.Old != zero {
			old = *u.Old
		}
		if old == "" {
			f.Ff = true
		} else if _, _, err := git(repo, id, nil, "merge-base", "--is-ancestor", old, nw); err == nil {
			f.Ff = true
		}
		c, _, err := git(repo, id, nil, "rev-parse", "--verify", "-q", nw+"^{commit}")
		if err != nil {
			facts = append(facts, f)
			continue
		}
		commit := trim(c)
		f.Contains = []string{}
		for _, h := range heads[u.Name] {
			if _, _, err := git(repo, id, nil, "merge-base", "--is-ancestor", h, commit); err == nil {
				f.Contains = append(f.Contains, h)
			}
		}
		paths := map[string]bool{}
		// Against the merge base with the default branch's tip (the base's, in fork mode).
		mb := emptyTree
		if defaultTip != "" {
			if b, _, err := git(repo, id, nil, "merge-base", defaultTip, commit); err == nil && trim(b) != "" {
				mb = trim(b)
			}
		}
		f.MergeBase = mb
		if d, _, err := git(repo, id, nil, "diff", "--no-renames", "--name-only", "-z", mb, commit); err == nil {
			for _, p := range splitZ(d) {
				paths[p] = true
			}
		}
		// Against the old tip.
		if old != "" {
			if oc, _, err := git(repo, id, nil, "rev-parse", "--verify", "-q", old+"^{commit}"); err == nil {
				if d, _, err := git(repo, id, nil, "diff", "--no-renames", "--name-only", "-z", trim(oc), commit); err == nil {
					for _, p := range splitZ(d) {
						paths[p] = true
					}
				}
			}
		}
		for p := range paths {
			f.Paths = append(f.Paths, p)
		}
		if u.Name == "refs/notes/ai" {
			base := emptyTree
			if old != "" {
				if ot, _, err := git(repo, id, nil, "rev-parse", "--verify", "-q", old+"^{tree}"); err == nil {
					base = trim(ot)
				}
			}
			d, _, _ := git(repo, id, nil, "diff-tree", "-r", "-z", "--no-renames", base, commit+"^{tree}")
			// Raw format: ":<old mode> <new mode> <old oid> <new oid> <status>\0<path>\0"
			parts := strings.Split(string(d), "\x00")
			for i := 0; i+1 < len(parts); i += 2 {
				meta := strings.Fields(strings.TrimPrefix(parts[i], ":"))
				if len(meta) < 5 || meta[4] == "D" {
					continue
				}
				n := note{Path: parts[i+1], Mode: meta[1], Oid: meta[3]}
				tt, _, _ := git(repo, id, nil, "cat-file", "-t", n.Oid)
				n.Type = trim(tt)
				if n.Type == "blob" {
					if b, _, err := git(repo, id, nil, "cat-file", "blob", n.Oid); err == nil {
						if len(b) > noteMax {
							b = b[:noteMax]
						}
						n.Bytes = base64.StdEncoding.EncodeToString(b)
					}
				}
				f.Notes = append(f.Notes, n)
			}
		}
		facts = append(facts, f)
	}
	// The new commits: reachable from the new tips and from no existing ref.
	commits := []commitRaw{}
	if len(tips) > 0 {
		// New commits: reachable from the new tips and not from the updated refs' old tips or the default branch.
		// (git's receive-pack uses --not --all; with thousands of refs that walk dominated each push, and for the
		// trailer parse a commit already on the default branch or the ref's old tip is never new.)
		args := append([]string{"rev-list", fmt.Sprintf("--max-count=%d", commitsMax)}, tips...)
		args = append(args, "--not")
		for _, u := range updates {
			if u.Old != nil && *u.Old != zero {
				args = append(args, *u.Old)
			}
		}
		if defaultTip != "" {
			args = append(args, defaultTip)
		}
		if out, _, err := git(repo, id, nil, args...); err == nil {
			oids := strings.Fields(string(out))
			if len(oids) > 0 {
				raw, _, _ := git(repo, id, []byte(strings.Join(oids, "\n")+"\n"), "cat-file", "--batch")
				commits = parseBatch(raw)
			}
		}
	}
	return map[string]any{"facts": facts, "commits": commits}, nil
}

func parseBatch(b []byte) []commitRaw {
	var out []commitRaw
	for len(b) > 0 {
		nl := bytes.IndexByte(b, '\n')
		if nl < 0 {
			break
		}
		hdr := strings.Fields(string(b[:nl]))
		b = b[nl+1:]
		if len(hdr) < 3 {
			continue
		}
		var n int
		fmt.Sscanf(hdr[2], "%d", &n)
		if n > len(b) {
			break
		}
		if hdr[1] == "commit" {
			out = append(out, commitRaw{Oid: hdr[0], Raw: base64.StdEncoding.EncodeToString(b[:n])})
		}
		b = b[n:]
		if len(b) > 0 && b[0] == '\n' {
			b = b[1:]
		}
	}
	return out
}

func readJSON(r *http.Request, v any) error { return json.NewDecoder(r.Body).Decode(v) }
func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("content-type", "application/json")
	json.NewEncoder(w).Encode(v)
}

func main() {
	if len(os.Args) > 1 && os.Args[1] == "-root" {
		root = os.Args[2]
	}
	os.MkdirAll(filepath.Join(root, "q"), 0o755)
	http.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) { w.Write([]byte("ok")) })
	http.HandleFunc("/init", func(w http.ResponseWriter, r *http.Request) {
		var b struct {
			Repo       string `json:"repo"`
			Alternates string `json:"alternates"`
		}
		if err := readJSON(r, &b); err != nil {
			http.Error(w, err.Error(), 400)
			return
		}
		d := repoDir(b.Repo)
		os.RemoveAll(d)
		out, err := exec.Command("git", "init", "-q", "--bare", "-b", "main", d).CombinedOutput()
		if err != nil {
			http.Error(w, string(out), 500)
			return
		}
		// Upload-pack config as GHES serves it; receive is the Worker's own.
		exec.Command("git", "--git-dir="+d, "config", "uploadpack.allowFilter", "true").Run()
		exec.Command("git", "--git-dir="+d, "config", "core.logAllRefUpdates", "false").Run()
		if b.Alternates != "" {
			os.WriteFile(filepath.Join(d, "objects/info/alternates"), []byte(filepath.Join(repoDir(b.Alternates), "objects")+"\n"), 0o644)
		}
		writeJSON(w, map[string]any{"ok": true})
	})
	http.HandleFunc("/pack", func(w http.ResponseWriter, r *http.Request) {
		repo, id := r.URL.Query().Get("repo"), r.URL.Query().Get("id")
		body, _ := io.ReadAll(r.Body)
		os.MkdirAll(filepath.Join(qDir(id), "pack"), 0o755)
		out, errb, err := git(repo, id, body, "index-pack", "--strict", "--fix-thin", "--stdin")
		if err != nil {
			writeJSON(w, map[string]any{"ok": false, "error": trim(errb)})
			return
		}
		writeJSON(w, map[string]any{"ok": true, "pack": trim(out)})
	})
	http.HandleFunc("/inspect", func(w http.ResponseWriter, r *http.Request) {
		var b struct {
			Repo       string              `json:"repo"`
			ID         string              `json:"id"`
			Updates    []update            `json:"updates"`
			DefaultTip string              `json:"defaultTip"`
			Heads      map[string][]string `json:"heads"`
		}
		if err := readJSON(r, &b); err != nil {
			http.Error(w, err.Error(), 400)
			return
		}
		res, err := inspect(b.Repo, b.ID, b.Updates, b.DefaultTip, b.Heads)
		if err != nil {
			http.Error(w, err.Error(), 500)
			return
		}
		writeJSON(w, res)
	})
	http.HandleFunc("/commit", func(w http.ResponseWriter, r *http.Request) {
		var b struct {
			Repo    string            `json:"repo"`
			ID      string            `json:"id"`
			Parent  string            `json:"parent"`
			Add     map[string]string `json:"add"` // path -> base64 contents
			Del     []string          `json:"del"`
			Message string            `json:"message"`
			Author  string            `json:"author"`
		}
		if err := readJSON(r, &b); err != nil {
			http.Error(w, err.Error(), 400)
			return
		}
		os.MkdirAll(filepath.Join(qDir(b.ID), "pack"), 0o755)
		idx := filepath.Join(qDir(b.ID), "index")
		run := func(stdin []byte, args ...string) (string, error) {
			cmd := exec.Command("git", append([]string{"--git-dir=" + repoDir(b.Repo)}, args...)...)
			cmd.Env = append(os.Environ(), "HOME=/tmp", "GIT_INDEX_FILE="+idx, "GIT_OBJECT_DIRECTORY="+qDir(b.ID),
				"GIT_ALTERNATE_OBJECT_DIRECTORIES="+filepath.Join(repoDir(b.Repo), "objects"),
				"GIT_AUTHOR_NAME="+b.Author, "GIT_AUTHOR_EMAIL=noreply@graftyard.invalid",
				"GIT_COMMITTER_NAME=Graftyard", "GIT_COMMITTER_EMAIL=noreply@graftyard.invalid")
			if stdin != nil {
				cmd.Stdin = bytes.NewReader(stdin)
			}
			out, err := cmd.CombinedOutput()
			if err != nil {
				return "", fmt.Errorf("%s: %s", strings.Join(args, " "), out)
			}
			return trim(out), nil
		}
		fail := func(err error) { writeJSON(w, map[string]any{"ok": false, "error": err.Error()}) }
		if _, err := run(nil, "read-tree", b.Parent+"^{tree}"); err != nil {
			fail(err)
			return
		}
		for p, c := range b.Add {
			data, _ := base64.StdEncoding.DecodeString(c)
			oid, err := run(data, "hash-object", "-w", "--stdin")
			if err != nil {
				fail(err)
				return
			}
			if _, err := run(nil, "update-index", "--add", "--cacheinfo", "100644,"+oid+","+p); err != nil {
				fail(err)
				return
			}
		}
		for _, p := range b.Del {
			run(nil, "update-index", "--force-remove", p)
		}
		tree, err := run(nil, "write-tree")
		if err != nil {
			fail(err)
			return
		}
		c, err := run([]byte(b.Message), "commit-tree", tree, "-p", b.Parent)
		if err != nil {
			fail(err)
			return
		}
		writeJSON(w, map[string]any{"ok": true, "oid": c})
	})
	http.HandleFunc("/finalize", func(w http.ResponseWriter, r *http.Request) {
		var b struct {
			Repo    string   `json:"repo"`
			ID      string   `json:"id"`
			Updates []update `json:"updates"`
		}
		if err := readJSON(r, &b); err != nil {
			http.Error(w, err.Error(), 400)
			return
		}
		// Migrate quarantined objects (packs from index-pack, loose objects from /commit) into the repository.
		q := qDir(b.ID)
		packs, _ := filepath.Glob(filepath.Join(q, "pack", "pack-*"))
		for _, p := range packs {
			os.Rename(p, filepath.Join(repoDir(b.Repo), "objects", "pack", filepath.Base(p)))
		}
		filepath.Walk(q, func(p string, info os.FileInfo, err error) error {
			if err != nil || info.IsDir() {
				return nil
			}
			rel, _ := filepath.Rel(q, p)
			if len(rel) == 41 && rel[2] == '/' {
				dst := filepath.Join(repoDir(b.Repo), "objects", rel)
				os.MkdirAll(filepath.Dir(dst), 0o755)
				os.Rename(p, dst)
			}
			return nil
		})
		var in strings.Builder
		in.WriteString("start\n")
		for _, u := range b.Updates {
			old := zero
			if u.Old != nil {
				old = *u.Old
			}
			if u.New == nil || *u.New == zero {
				fmt.Fprintf(&in, "delete %s %s\n", u.Name, old)
			} else {
				fmt.Fprintf(&in, "update %s %s %s\n", u.Name, *u.New, old)
			}
		}
		in.WriteString("prepare\ncommit\n")
		_, errb, err := git(b.Repo, "", []byte(in.String()), "update-ref", "--stdin")
		os.RemoveAll(q)
		// Compaction stand-in: one pack per push would make every git command scan thousands of packs.
		if all, _ := filepath.Glob(filepath.Join(repoDir(b.Repo), "objects", "pack", "*.pack")); len(all) >= 32 {
			git(b.Repo, "", nil, "repack", "--geometric=2", "-d", "-q")
		}
		if err != nil {
			writeJSON(w, map[string]any{"ok": false, "error": trim(errb)})
			return
		}
		writeJSON(w, map[string]any{"ok": true})
	})
	// M0-WP13: one raw object, for GET /commits/{sha}.
	http.HandleFunc("/cat", func(w http.ResponseWriter, r *http.Request) {
		q := r.URL.Query()
		t, _, err := git(q.Get("repo"), "", nil, "cat-file", "-t", q.Get("oid"))
		if err != nil {
			http.Error(w, "not found", 404)
			return
		}
		out, _, err := git(q.Get("repo"), "", nil, "cat-file", trim(t), q.Get("oid"))
		if err != nil {
			http.Error(w, "not found", 404)
			return
		}
		w.Header().Set("x-object-type", trim(t))
		w.Write(out)
	})
	// M0-WP13: POST /merges. A merge commit of head into base in the quarantine, from git merge-tree.
	http.HandleFunc("/merge", func(w http.ResponseWriter, r *http.Request) {
		var b struct {
			Repo    string `json:"repo"`
			ID      string `json:"id"`
			Base    string `json:"base"`
			Head    string `json:"head"`
			Message string `json:"message"`
		}
		if err := readJSON(r, &b); err != nil {
			http.Error(w, err.Error(), 400)
			return
		}
		os.MkdirAll(filepath.Join(qDir(b.ID), "pack"), 0o755)
		env := []string{"GIT_AUTHOR_NAME=Graftyard", "GIT_AUTHOR_EMAIL=noreply@graftyard.invalid", "GIT_COMMITTER_NAME=Graftyard", "GIT_COMMITTER_EMAIL=noreply@graftyard.invalid"}
		run := func(stdin []byte, args ...string) (string, error) {
			cmd := exec.Command("git", append([]string{"--git-dir=" + repoDir(b.Repo)}, args...)...)
			cmd.Env = append(append(os.Environ(), "HOME=/tmp", "GIT_OBJECT_DIRECTORY="+qDir(b.ID),
				"GIT_ALTERNATE_OBJECT_DIRECTORIES="+filepath.Join(repoDir(b.Repo), "objects")), env...)
			if stdin != nil {
				cmd.Stdin = bytes.NewReader(stdin)
			}
			out, err := cmd.Output()
			return trim(out), err
		}
		if _, err := run(nil, "merge-base", "--is-ancestor", b.Head, b.Base); err == nil {
			writeJSON(w, map[string]any{"ok": true, "noop": true})
			return
		}
		tree, err := run(nil, "merge-tree", "--write-tree", b.Base, b.Head)
		if err != nil {
			writeJSON(w, map[string]any{"ok": false, "conflict": true})
			return
		}
		c, err := run([]byte(b.Message), "commit-tree", strings.Fields(tree)[0], "-p", b.Base, "-p", b.Head)
		if err != nil {
			writeJSON(w, map[string]any{"ok": false, "error": err.Error()})
			return
		}
		writeJSON(w, map[string]any{"ok": true, "oid": c})
	})
	http.HandleFunc("/discard", func(w http.ResponseWriter, r *http.Request) {
		var b struct {
			ID string `json:"id"`
		}
		readJSON(r, &b)
		os.RemoveAll(qDir(b.ID))
		writeJSON(w, map[string]any{"ok": true})
	})
	// What git itself holds: every ref with its object id, as "<oid> <name>" lines.
	http.HandleFunc("/refs", func(w http.ResponseWriter, r *http.Request) {
		out, errb, err := git(r.URL.Query().Get("repo"), "", nil, "for-each-ref", "--format=%(objectname) %(refname)")
		if err != nil {
			http.Error(w, trim(errb), 404)
			return
		}
		w.Write(out)
	})
	http.HandleFunc("/advertise", func(w http.ResponseWriter, r *http.Request) {
		q := r.URL.Query()
		cmd := exec.Command("git", "-c", "uploadpack.hideRefs=refs/graftyard/", "upload-pack", "--stateless-rpc", "--http-backend-info-refs", repoDir(q.Get("repo")))
		cmd.Env = append(os.Environ(), "HOME=/tmp", "GIT_PROTOCOL=version="+q.Get("proto"))
		out, err := cmd.Output()
		if err != nil {
			http.Error(w, err.Error(), 500)
			return
		}
		w.Write(out)
	})
	http.HandleFunc("/upload-pack", func(w http.ResponseWriter, r *http.Request) {
		q := r.URL.Query()
		args := []string{}
		if q.Get("hide") != "0" {
			args = append(args, "-c", "uploadpack.hideRefs=refs/graftyard/")
		}
		args = append(args, "upload-pack", "--stateless-rpc", repoDir(q.Get("repo")))
		cmd := exec.Command("git", args...)
		cmd.Env = append(os.Environ(), "HOME=/tmp", "GIT_PROTOCOL=version="+q.Get("proto"))
		cmd.Stdin = r.Body
		out, err := cmd.StdoutPipe()
		if err == nil {
			err = cmd.Start()
		}
		if err != nil {
			http.Error(w, err.Error(), 500)
			return
		}
		w.Header().Set("content-type", "application/x-git-upload-pack-result")
		io.CopyBuffer(w, out, make([]byte, 1<<16))
		cmd.Wait()
	})
	addr := envOr("GITD_ADDR", ":8080")
	fmt.Println("gitd on", addr, "root", root)
	http.ListenAndServe(addr, nil)
}
