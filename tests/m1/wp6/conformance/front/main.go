// The conformance harness's web server: what git's t/lib-httpd.sh starts in Apache's place (see ../httpd). It listens
// on 127.0.0.1 where the test expects Apache and forwards smart HTTP to a hosted Graftyard edge.
//
// git's HTTP tests keep the "server" repositories as local bare repositories under $HTTPD_DOCUMENT_ROOT_PATH and edit
// them directly (create, commit, update-ref). A hosted service has no such back door, so before forwarding a request
// for www/<repo> the front mirrors that local repository into one of a few slot repositories on the host
// (`git push --mirror`, through the edge itself), and after a receive-pack it mirrors the slot back into the local
// repository (`git fetch --prune +refs/*:refs/*`), so the test's own checks of the server repository see what the
// host holds. The traffic under test, the client's requests, goes to the host unchanged except for the path and the
// credential mapping below.
//
// Mapped prefixes, each as Apache's apache.conf serves it:
//
//	/smart/       anonymous; receive-pack carries the writer token only when the local repository sets
//	              http.receivepack, as git-http-backend allows anonymous pushes only then
//	/smart_v0/    the same with the Git-Protocol header removed (Apache: SetEnv GIT_PROTOCOL)
//	/auth-push/smart/  the client's own credential: user@host:pass@host becomes the writer token and
//	              forbidden-user@host:pass@host the read-only token; no credential stays none
//
// Every other prefix is an Apache fixture (rewrites, CGI scripts, realms, namespaces) and answers 501, logged as
// "unmapped" so the report classifies the test as not applicable. Every request is logged in Apache's common format to
// $root/access.log, which the tests read, and to the events file with the number of the test that was running.
package main

import (
	"bufio"
	"bytes"
	"crypto/tls"
	"encoding/base64"
	"flag"
	"fmt"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"
)

type slot struct {
	name  string // owner/repo on the host
	local string // the local repository it mirrors, "" when unused
	state string // that repository's refs when last mirrored
	used  time.Time
}

type front struct {
	host     *url.URL
	token    string
	roToken  string
	gitBin   string
	syncHome string
	root     string
	www      string
	script   string
	version  string
	out      string // the test's --verbose-log output, read for the running test's number
	events   *os.File
	access   *os.File

	mu      sync.Mutex
	slots   []*slot
	outPos  int64
	test    string
	settled map[string]bool // (test, setting) pairs already logged
}

var testStart = regexp.MustCompile(`expecting (?:success|failure) of [^ ]*\.([0-9]+) `)

// current is the number of the test whose body is running, from the last "expecting success of N.M" line.
func (f *front) current() string {
	fh, err := os.Open(f.out)
	if err != nil {
		return f.test
	}
	defer fh.Close()
	if _, err := fh.Seek(f.outPos, 0); err != nil {
		return f.test
	}
	r := bufio.NewReaderSize(fh, 1<<16)
	for {
		line, err := r.ReadString('\n')
		if err != nil {
			break // a partial last line is read again next time
		}
		f.outPos += int64(len(line))
		if m := testStart.FindStringSubmatch(line); m != nil {
			f.test = m[1]
		}
	}
	return f.test
}

func (f *front) event(kind, method, uri string, status int, detail string) {
	detail = strings.NewReplacer("\t", " ", "\n", " | ").Replace(f.redact(detail))
	fmt.Fprintf(f.events, "%s\t%s\t%s\t%s\t%s\t%s\t%d\t%s\n", f.version, f.script, f.current(), kind, method, uri, status,
		detail)
}

func (f *front) redact(s string) string {
	for _, t := range []string{f.token, f.roToken} {
		if t != "" {
			s = strings.ReplaceAll(s, t, "<token>")
		}
	}
	return s
}

// git runs the harness's own git (not the one under test) with a clean environment.
func (f *front) git(dir string, args ...string) (string, error) {
	cmd := exec.Command(f.gitBin, append([]string{"-C", dir, "-c", "credential.helper=", "-c", "protocol.version=2"},
		args...)...)
	env := []string{"HOME=" + f.syncHome, "GIT_CONFIG_NOSYSTEM=1", "GIT_TERMINAL_PROMPT=0", "PATH=/usr/bin:/bin"}
	for _, k := range []string{"HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY", "https_proxy", "http_proxy", "no_proxy"} {
		if v, ok := os.LookupEnv(k); ok {
			env = append(env, k+"="+v)
		}
	}
	cmd.Env = env
	out, err := cmd.CombinedOutput()
	return string(out), err
}

func (f *front) refs(local string) (string, error) {
	out, err := f.git(local, "for-each-ref", "--format=%(objectname) %(refname)")
	if err != nil {
		return "", err
	}
	lines := strings.Split(strings.TrimSpace(out), "\n")
	sort.Strings(lines)
	return strings.Join(lines, "\n"), nil
}

func (f *front) remote(s *slot) string {
	u := *f.host
	u.User = url.UserPassword("x", f.token)
	u.Path = "/" + s.name + ".git"
	return u.String()
}

// hostDefault is how the host behaves for each transfer setting a test may put on its server repository: a test that
// sets one of these to this value asks for nothing the host does not already do. `http.receivepack` is mapped to the
// writer's credential instead (ServeHTTP).
var hostDefault = map[string]string{
	"uploadpack.allowfilter":              "true",
	"uploadpack.allowtipsha1inwant":       "true",
	"uploadpack.allowreachablesha1inwant": "true",
	"http.receivepack":                    "true",
}

// settings logs, when they first appear or change, the server-side settings a hosted repository cannot carry: hooks,
// and transfer configuration other than the host's own behavior.
func (f *front) settings(local string) {
	found := map[string]string{}
	for _, d := range []string{filepath.Join(local, "hooks"), filepath.Join(local, ".git", "hooks")} {
		if hooks, err := os.ReadDir(d); err == nil {
			for _, h := range hooks {
				if !strings.HasSuffix(h.Name(), ".sample") {
					found["hook:"+h.Name()] = "present"
				}
			}
		}
	}
	if out, err := f.git(local, "config", "--local", "--list"); err == nil {
		for _, l := range strings.Split(out, "\n") {
			k, v, _ := strings.Cut(l, "=")
			for _, p := range []string{"uploadpack.", "receive.", "transfer.", "http.", "uploadarchive.", "core.bigfilethreshold", "pack."} {
				if strings.HasPrefix(k, p) && hostDefault[k] != v {
					found["config:"+k] = v
				}
			}
		}
	}
	for k, v := range found {
		key := f.current() + " " + local + " " + k + "=" + v
		if !f.settled[key] {
			f.settled[key] = true
			f.event("server-setting", "", local[len(f.www):], 0, k+"="+v)
		}
	}
}

// slotFor mirrors local into a slot and returns it: the slot already holding local, else the least recently used.
func (f *front) slotFor(local string) (*slot, error) {
	state, err := f.refs(local)
	if err != nil {
		return nil, err
	}
	f.settings(local)
	var s *slot
	for _, c := range f.slots {
		if c.local == local {
			s = c
		}
	}
	if s == nil {
		s = f.slots[0]
		for _, c := range f.slots {
			if c.used.Before(s.used) {
				s = c
			}
		}
		s.local, s.state = "", "\x00"
	}
	s.used = time.Now()
	if s.local == local && s.state == state {
		return s, nil
	}
	if _, err := os.Stat(filepath.Join(gitDir(local), "shallow")); err == nil {
		// A shallow server repository (t5539) cannot be pushed from, and a hosted repository always holds whole
		// history, so the test is classified as not applicable; the push below fails and says why.
		f.event("server-setting", "", local[len(f.www):], 0, "shallow server repository")
	}
	out, err := f.git(local, "push", "--mirror", "--porcelain", f.remote(s))
	f.event("mirror-push", "", local[len(f.www):]+" -> "+s.name, 0, lastLines(out, 6))
	if err != nil && state == "" && strings.Contains(out, "No refs in common") {
		err = nil
	}
	if err != nil {
		s.local = ""
		return s, fmt.Errorf("mirroring %s into %s: %v: %s", local, s.name, err, f.redact(lastLines(out, 6)))
	}
	s.local, s.state = local, state
	return s, nil
}

func (f *front) mirrorBack(s *slot, local string) {
	out, err := f.git(local, "fetch", "--prune", "--update-head-ok", "--no-tags", "--no-recurse-submodules",
		"--no-write-fetch-head",
		f.remote(s), "+refs/*:refs/*")
	f.event("mirror-back", "", s.name+" -> "+local[len(f.www):], 0, lastLines(out, 4))
	if err != nil {
		s.local = ""
		return
	}
	if state, err := f.refs(local); err == nil {
		s.state = state
	}
}

func gitDir(local string) string {
	if _, err := os.Stat(filepath.Join(local, ".git")); err == nil {
		return filepath.Join(local, ".git")
	}
	return local
}

func lastLines(s string, n int) string {
	l := strings.Split(strings.TrimSpace(s), "\n")
	if len(l) > n {
		l = l[len(l)-n:]
	}
	return strings.Join(l, "\n")
}

type recorder struct {
	http.ResponseWriter
	status int
	bytes  int64
}

func (r *recorder) WriteHeader(c int) {
	if r.status == 0 {
		r.status = c
	}
	r.ResponseWriter.WriteHeader(c)
}

func (r *recorder) Write(b []byte) (int, error) {
	if r.status == 0 {
		r.status = 200
	}
	n, err := r.ResponseWriter.Write(b)
	r.bytes += int64(n)
	return n, err
}

func (r *recorder) Flush() {
	if fl, ok := r.ResponseWriter.(http.Flusher); ok {
		fl.Flush()
	}
}

// split returns the mapped prefix, the repository path under www, and the rest of the URL path.
func split(path string) (prefix, repo, rest string) {
	for _, p := range []string{"/auth-push/smart/", "/smart_v0/", "/smart/"} {
		if strings.HasPrefix(path, p) {
			prefix, path = p, path[len(p)-1:]
			break
		}
	}
	if prefix == "" {
		return "", "", path
	}
	for _, m := range []string{"/info/refs", "/git-upload-pack", "/git-receive-pack", "/HEAD", "/objects/"} {
		if i := strings.Index(path, m); i >= 0 {
			return prefix, path[:i], path[i:]
		}
	}
	return prefix, path, ""
}

func basicUser(h string) (string, string, bool) {
	if !strings.HasPrefix(h, "Basic ") {
		return "", "", false
	}
	b, err := base64.StdEncoding.DecodeString(strings.TrimPrefix(h, "Basic "))
	if err != nil {
		return "", "", false
	}
	u, p, ok := strings.Cut(string(b), ":")
	return u, p, ok
}

func basic(token string) string {
	return "Basic " + base64.StdEncoding.EncodeToString([]byte("x:"+token))
}

func (f *front) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	f.mu.Lock()
	defer f.mu.Unlock()
	rec := &recorder{ResponseWriter: w}
	start := time.Now()
	uri := r.URL.RequestURI()
	defer func() {
		size := "-"
		if rec.bytes > 0 {
			size = fmt.Sprint(rec.bytes)
		}
		fmt.Fprintf(f.access, "127.0.0.1 - - [%s] \"%s %s %s\" %d %s\n", start.Format("02/Jan/2006:15:04:05 -0700"),
			r.Method, uri, r.Proto, rec.status, size)
	}()
	prefix, repo, rest := split(r.URL.EscapedPath())
	// apache.conf's ScriptAliases inside /smart/: CGI scripts that answer as a broken server would.
	for _, fx := range []string{"/smart/no_report/", "/smart/incomplete_length/", "/smart/incomplete_body/"} {
		if strings.HasPrefix(r.URL.Path, fx) {
			prefix = ""
		}
	}
	if prefix == "" {
		seg := strings.SplitN(strings.TrimPrefix(r.URL.Path, "/"), "/", 2)[0]
		if seg == "smart" {
			seg = "smart/" + strings.SplitN(strings.TrimPrefix(r.URL.Path, "/smart/"), "/", 2)[0]
		}
		f.event("unmapped", r.Method, uri, 501, seg)
		http.Error(rec, "graftyard conformance front: /"+seg+"/ is an Apache fixture with no hosted equivalent", 501)
		return
	}
	plain, _ := url.PathUnescape(repo)
	local := filepath.Join(f.www, filepath.FromSlash(strings.TrimPrefix(plain, "/")))
	// As git-http-backend (and github.com) serve /o/r for /o/r.git.
	if fi, err := os.Stat(local); (err != nil || !fi.IsDir()) && !strings.HasSuffix(local, ".git") {
		local += ".git"
	}
	if fi, err := os.Stat(local); err != nil || !fi.IsDir() {
		f.event("no-repo", r.Method, uri, 404, "")
		http.Error(rec, "Repository not found.", 404)
		return
	}
	s, err := f.slotFor(local)
	if err != nil {
		f.event("mirror-error", r.Method, uri, 502, err.Error())
		http.Error(rec, "graftyard conformance front: "+f.redact(err.Error()), 502)
		return
	}
	receive := strings.HasSuffix(rest, "/git-receive-pack") || r.URL.Query().Get("service") == "git-receive-pack"
	auth := r.Header.Get("Authorization")
	r.Header.Del("Authorization")
	switch prefix {
	case "/smart/", "/smart_v0/":
		if receive {
			if v, _ := f.git(local, "config", "--bool", "http.receivepack"); strings.TrimSpace(v) == "true" {
				r.Header.Set("Authorization", basic(f.token))
			}
		}
		if prefix == "/smart_v0/" {
			r.Header.Del("Git-Protocol")
		}
	case "/auth-push/smart/":
		if u, p, ok := basicUser(auth); ok && p == "pass@host" && u == "user@host" {
			r.Header.Set("Authorization", basic(f.token))
		} else if ok && p == "pass@host" && u == "forbidden-user@host" {
			r.Header.Set("Authorization", basic(f.roToken))
		} else if auth != "" {
			r.Header.Set("Authorization", auth)
		}
	}
	target := *f.host
	target.Path = "/" + s.name + ".git" + rest
	target.RawPath = ""
	target.RawQuery = r.URL.RawQuery
	// A receive-pack's answer is held until the slot is mirrored back, so the test's next look at its server
	// repository sees the push: git reads the response to its end before it checks anything.
	var held *bufferedWriter
	out := http.ResponseWriter(rec)
	if receive && r.Method == http.MethodPost {
		held = &bufferedWriter{header: http.Header{}}
		out = held
	}
	p := &httputil.ReverseProxy{
		Director: func(q *http.Request) {
			q.URL = &target
			q.Host = target.Host
			q.Header.Del("X-Forwarded-For")
		},
		FlushInterval: -1,
		ErrorHandler: func(w http.ResponseWriter, _ *http.Request, err error) {
			f.event("upstream-error", r.Method, uri, 502, err.Error())
			http.Error(w, "graftyard conformance front: "+err.Error(), 502)
		},
	}
	p.ServeHTTP(out, r)
	if held != nil {
		f.mirrorBack(s, local)
		for k, v := range held.header {
			rec.Header()[k] = v
		}
		rec.WriteHeader(held.status)
		rec.Write(held.body.Bytes())
	}
	f.event("forward", r.Method, uri, rec.status, s.name+fmt.Sprintf(" %d ms", time.Since(start).Milliseconds()))
}

type bufferedWriter struct {
	header http.Header
	status int
	body   bytes.Buffer
}

func (b *bufferedWriter) Header() http.Header { return b.header }
func (b *bufferedWriter) WriteHeader(c int) {
	if b.status == 0 {
		b.status = c
	}
}
func (b *bufferedWriter) Write(p []byte) (int, error) {
	if b.status == 0 {
		b.status = 200
	}
	return b.body.Write(p)
}

func main() {
	listen := flag.String("listen", "127.0.0.1:5551", "address the test expects Apache on")
	root := flag.String("root", "", "$HTTPD_ROOT_PATH")
	paras := flag.String("para", "", "Apache -D definitions, space-separated")
	flag.Parse()
	host, err := url.Parse(os.Getenv("CONFORMANCE_HOST"))
	if err != nil || host.Host == "" {
		fmt.Fprintln(os.Stderr, "CONFORMANCE_HOST must be https://<edge host>")
		os.Exit(2)
	}
	f := &front{
		host:     host,
		token:    os.Getenv("CONFORMANCE_TOKEN"),
		roToken:  os.Getenv("CONFORMANCE_RO_TOKEN"),
		gitBin:   envOr("CONFORMANCE_SYNC_GIT", "/usr/bin/git"),
		syncHome: filepath.Join(*root, "front-home"),
		root:     *root,
		www:      filepath.Join(*root, "www"),
		version:  os.Getenv("CONFORMANCE_GIT_VERSION"),
		settled:  map[string]bool{},
	}
	os.MkdirAll(f.syncHome, 0o755)
	// $root is <t>/trash directory.<script>/httpd; --verbose-log writes <t>/test-results/<script>.out.
	trash := filepath.Dir(*root)
	f.script = strings.TrimPrefix(filepath.Base(trash), "trash directory.")
	f.out = filepath.Join(filepath.Dir(trash), "test-results", f.script+".out")
	for _, n := range strings.Split(os.Getenv("CONFORMANCE_SLOTS"), ",") {
		if n = strings.TrimSpace(n); n != "" {
			f.slots = append(f.slots, &slot{name: n})
		}
	}
	if len(f.slots) == 0 || f.token == "" {
		fmt.Fprintln(os.Stderr, "CONFORMANCE_SLOTS and CONFORMANCE_TOKEN are required")
		os.Exit(2)
	}
	if f.events, err = os.OpenFile(envOr("CONFORMANCE_EVENTS", filepath.Join(*root, "events.tsv")),
		os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644); err != nil {
		panic(err)
	}
	if f.access, err = os.OpenFile(filepath.Join(*root, "access.log"), os.O_CREATE|os.O_APPEND|os.O_WRONLY,
		0o644); err != nil {
		panic(err)
	}
	// The test removes its trash directory when it ends; the front goes with it.
	go func() {
		for range time.Tick(2 * time.Second) {
			if _, err := os.Stat(*root); err != nil {
				os.Exit(0)
			}
		}
	}()
	srv := &http.Server{Addr: *listen, Handler: f}
	if strings.Contains(" "+*paras+" ", " SSL ") {
		pem := filepath.Join(*root, "httpd.pem")
		cert, err := tls.LoadX509KeyPair(pem, pem)
		if err != nil {
			panic(err)
		}
		srv.TLSConfig = &tls.Config{Certificates: []tls.Certificate{cert}, NextProtos: []string{"h2", "http/1.1"}}
		err = srv.ListenAndServeTLS("", "")
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	fmt.Fprintln(os.Stderr, srv.ListenAndServe())
	os.Exit(1)
}

func envOr(k, d string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return d
}
