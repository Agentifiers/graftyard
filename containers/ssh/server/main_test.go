package main

import (
	"bufio"
	"bytes"
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"encoding/pem"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/cgi"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
)

// slotPEM makes one host-key slot as deploy/ssh-front/hostkeys.mjs writes it: an Ed25519, an ECDSA P-256, and an RSA
// key (2048 bits here, to keep the test fast; the deploy makes 3072), in OpenSSH PEM, concatenated.
func slotPEM(t *testing.T) string {
	t.Helper()
	_, ed, _ := ed25519.GenerateKey(rand.Reader)
	ec, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	rk, _ := rsa.GenerateKey(rand.Reader, 2048)
	var out []byte
	for _, k := range []any{ed, ec, rk} {
		b, err := ssh.MarshalPrivateKey(k, "")
		if err != nil {
			t.Fatal(err)
		}
		out = append(out, pem.EncodeToMemory(b)...)
	}
	return string(out)
}

// edge stands in for the Graftyard edge: the key route answers from `keys` (fingerprint hex to status), and every
// other path is `git http-backend` over a directory of bare repositories, so the test drives real git on both ends.
type edge struct {
	root    string
	mu      sync.Mutex
	keys    map[string]int
	seen    []string // "METHOD path fp"
	lookups atomic.Int32
}

func (e *edge) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	fp := r.Header.Get("x-graftyard-ssh-key")
	e.mu.Lock()
	e.seen = append(e.seen, r.Method+" "+r.URL.Path+" "+fp)
	e.mu.Unlock()
	if r.URL.Path == "/_graftyard/ssh/key" {
		e.lookups.Add(1)
		e.mu.Lock()
		code, ok := e.keys[r.URL.Query().Get("fp")]
		e.mu.Unlock()
		if !ok {
			code = 404
		}
		w.WriteHeader(code)
		return
	}
	if fp == "" {
		http.Error(w, "no key", 401)
		return
	}
	// The edge streams a request body; Go's CGI handler takes only a sized one, so the stand-in holds it.
	if r.ContentLength < 0 {
		body, _ := io.ReadAll(r.Body)
		r.Body = io.NopCloser(bytes.NewReader(body))
		r.ContentLength = int64(len(body))
		r.TransferEncoding = nil
	}
	h := &cgi.Handler{
		Path: gitBin(),
		Args: []string{"http-backend"},
		Env:  []string{"GIT_PROJECT_ROOT=" + e.root, "GIT_HTTP_EXPORT_ALL=1", "REMOTE_USER=ssh"},
	}
	h.ServeHTTP(w, r)
}

func gitBin() string {
	p, err := exec.LookPath("git")
	if err != nil {
		return "git"
	}
	return p
}

func run(t *testing.T, dir string, env []string, name string, args ...string) string {
	t.Helper()
	c := exec.Command(name, args...)
	c.Dir = dir
	c.Env = append(os.Environ(), env...)
	out, err := c.CombinedOutput()
	if err != nil {
		t.Fatalf("%s %v: %v\n%s", name, args, err, out)
	}
	return string(out)
}

type bed struct {
	t       *testing.T
	edge    *edge
	srv     *server
	addr    string
	dir     string
	sshCmd  string
	keyFP   string
	slots   hostKeys
	tmpHome string
}

// newBed starts the stand-in edge, graft-sshd on a loopback port, a bare repository `acme/app.git` with one commit,
// and a client key the edge knows; `sshCmd` is the GIT_SSH_COMMAND that reaches it with OpenSSH's client.
func newBed(t *testing.T) *bed {
	if _, err := exec.LookPath("ssh"); err != nil {
		t.Skip("no OpenSSH client")
	}
	dir := t.TempDir()
	root := filepath.Join(dir, "repos")
	run(t, dir, nil, "git", "init", "-q", "--bare", "-b", "main", filepath.Join(root, "acme", "app.git"))
	run(t, filepath.Join(root, "acme", "app.git"), nil, "git", "config", "http.receivepack", "true")
	seed := filepath.Join(dir, "seed")
	run(t, dir, nil, "git", "init", "-q", "-b", "main", seed)
	for i := 0; i < 20; i++ {
		os.WriteFile(filepath.Join(seed, fmt.Sprintf("f%d.txt", i)), bytes.Repeat([]byte(fmt.Sprintf("line %d\n", i)), 300), 0o644)
	}
	id := []string{"GIT_AUTHOR_NAME=s", "GIT_AUTHOR_EMAIL=s@x", "GIT_COMMITTER_NAME=s", "GIT_COMMITTER_EMAIL=s@x"}
	run(t, seed, nil, "git", "add", "-A")
	run(t, seed, id, "git", "commit", "-qm", "seed")
	run(t, seed, nil, "git", "push", "-q", filepath.Join(root, "acme", "app.git"), "main")

	run(t, dir, nil, "ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", filepath.Join(dir, "id"))
	pubText, _ := os.ReadFile(filepath.Join(dir, "id.pub"))
	pub, _, _, _, err := ssh.ParseAuthorizedKey(pubText)
	if err != nil {
		t.Fatal(err)
	}
	sum := sha256.Sum256(pub.Marshal())
	fp := hex.EncodeToString(sum[:])

	e := &edge{root: root, keys: map[string]int{fp: 200}}
	hs := httptest.NewServer(e)
	t.Cleanup(hs.Close)
	slots := hostKeys{Active: "A", Slots: map[string]string{"A": slotPEM(t), "B": slotPEM(t)}}
	srv, err := newServer(hs.URL, hs.URL, slots)
	if err != nil {
		t.Fatal(err)
	}
	srv.lookupBudget = 3 * time.Second
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { l.Close() })
	go func() {
		for {
			c, err := l.Accept()
			if err != nil {
				return
			}
			go srv.serve(c)
		}
	}()
	_, port, _ := net.SplitHostPort(l.Addr().String())
	ssh := fmt.Sprintf("ssh -p %s -i %s -o IdentitiesOnly=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=%s "+
		"-o UpdateHostKeys=no -o BatchMode=yes", port, filepath.Join(dir, "id"), filepath.Join(dir, "known_hosts"))
	return &bed{t: t, edge: e, srv: srv, addr: l.Addr().String(), dir: dir, sshCmd: ssh, keyFP: fp, slots: slots}
}

func (b *bed) git(dir string, args ...string) (string, error) {
	c := exec.Command("git", args...)
	c.Dir = dir
	c.Env = append(os.Environ(), "GIT_SSH_COMMAND="+b.sshCmd, "GIT_AUTHOR_NAME=a", "GIT_AUTHOR_EMAIL=a@x",
		"GIT_COMMITTER_NAME=a", "GIT_COMMITTER_EMAIL=a@x", "GIT_TERMINAL_PROMPT=0")
	out, err := c.CombinedOutput()
	return string(out), err
}

// A stock `git clone git@host:owner/repo.git` and `git push` through graft-sshd: the clone gets the seeded commit, the
// push lands in the edge's repository, and every git request carried the verified key's fingerprint.
func TestCloneAndPushThroughTheEdge(t *testing.T) {
	b := newBed(t)
	work := filepath.Join(b.dir, "work")
	if out, err := b.git(b.dir, "clone", "git@127.0.0.1:acme/app.git", work); err != nil {
		t.Fatalf("clone: %v\n%s", err, out)
	}
	if _, err := os.Stat(filepath.Join(work, "f19.txt")); err != nil {
		t.Fatalf("the clone has no seeded file: %v", err)
	}
	// A push that sends a pack with a delta against a file the server has, and a new branch.
	os.WriteFile(filepath.Join(work, "f3.txt"), append(bytes.Repeat([]byte("line 3\n"), 300), "more\n"...), 0o644)
	os.WriteFile(filepath.Join(work, "new.txt"), []byte("new\n"), 0o644)
	if out, err := b.git(work, "commit", "-qam", "edit"); err != nil {
		t.Fatalf("commit: %v\n%s", err, out)
	}
	b.git(work, "add", "new.txt")
	b.git(work, "commit", "-qm", "add")
	if out, err := b.git(work, "push", "origin", "main", "main:refs/heads/side"); err != nil {
		t.Fatalf("push: %v\n%s", err, out)
	}
	local := strings.TrimSpace(run(t, work, nil, "git", "rev-parse", "HEAD"))
	bare := filepath.Join(b.edge.root, "acme", "app.git")
	for _, ref := range []string{"main", "side"} {
		if got := strings.TrimSpace(run(t, bare, nil, "git", "rev-parse", ref)); got != local {
			t.Fatalf("%s on the edge is %s, want %s", ref, got, local)
		}
	}
	run(t, bare, nil, "git", "fsck", "--strict")
	// A push of a delete only sends no pack.
	if out, err := b.git(work, "push", "origin", ":side"); err != nil {
		t.Fatalf("delete: %v\n%s", err, out)
	}
	if _, err := b.git(bare, "rev-parse", "--verify", "side"); err == nil {
		t.Fatal("side survived its delete")
	}
	// An up-to-date push sends a bare flush and changes nothing.
	if out, err := b.git(work, "push", "origin", "main"); err != nil || !strings.Contains(out, "up-to-date") {
		t.Fatalf("up-to-date push: %v\n%s", err, out)
	}
	b.edge.mu.Lock()
	defer b.edge.mu.Unlock()
	posts := 0
	for _, s := range b.edge.seen {
		if strings.HasPrefix(s, "POST ") {
			posts++
			if !strings.HasSuffix(s, " "+b.keyFP) {
				t.Fatalf("a git request without the key's fingerprint: %s", s)
			}
		}
	}
	if posts < 4 {
		t.Fatalf("expected the clone's and pushes' POSTs, got %v", b.edge.seen)
	}
}

// A key the edge does not know (404) is refused at once, with one lookup; a lookup that fails (503) is retried and
// a key that then answers 200 gets in; a key removed after a session started is refused on the next connection.
func TestKeyLookup(t *testing.T) {
	b := newBed(t)
	work := filepath.Join(b.dir, "w1")
	b.edge.mu.Lock()
	b.edge.keys[b.keyFP] = 404
	b.edge.mu.Unlock()
	out, err := b.git(b.dir, "clone", "git@127.0.0.1:acme/app.git", work)
	if err == nil || !strings.Contains(out, "Permission denied") {
		t.Fatalf("an unknown key was not refused: %v\n%s", err, out)
	}
	if n := b.edge.lookups.Load(); n != 1 {
		t.Fatalf("a 404 is final: %d lookups", n)
	}
	b.edge.mu.Lock()
	b.edge.keys[b.keyFP] = 503
	b.edge.mu.Unlock()
	go func() {
		time.Sleep(1500 * time.Millisecond)
		b.edge.mu.Lock()
		b.edge.keys[b.keyFP] = 200
		b.edge.mu.Unlock()
	}()
	b.edge.lookups.Store(0)
	if out, err := b.git(b.dir, "clone", "git@127.0.0.1:acme/app.git", work); err != nil {
		t.Fatalf("a key whose lookup recovered was refused: %v\n%s", err, out)
	}
	if n := b.edge.lookups.Load(); n < 2 {
		t.Fatalf("a 503 was not retried: %d lookups", n)
	}
	b.edge.mu.Lock()
	delete(b.edge.keys, b.keyFP)
	b.edge.mu.Unlock()
	if out, err := b.git(work, "fetch"); err == nil || !strings.Contains(out, "Permission denied") {
		t.Fatalf("a removed key fetched: %v\n%s", err, out)
	}
}

// A v0 fetch is refused with what to set; a command that is not upload-pack or receive-pack, or a path outside
// owner/repo, runs nothing.
func TestRefusals(t *testing.T) {
	b := newBed(t)
	c := exec.Command("git", "-c", "protocol.version=0", "clone", "git@127.0.0.1:acme/app.git", filepath.Join(b.dir, "v0"))
	c.Env = append(os.Environ(), "GIT_SSH_COMMAND="+b.sshCmd)
	out, err := c.CombinedOutput()
	if err == nil || !strings.Contains(string(out), "protocol v2 only") {
		t.Fatalf("a v0 clone was not refused with its reason: %v\n%s", err, out)
	}
	for _, cmd := range []string{"ls /", "git-upload-pack '../../etc'", "git-receive-pack 'a/b/c'"} {
		args := append(strings.Fields(b.sshCmd)[1:], "git@127.0.0.1", cmd)
		out, err := exec.Command("ssh", args...).CombinedOutput()
		if err == nil || !strings.Contains(string(out), "runs only git-upload-pack") {
			t.Fatalf("%q: %v\n%s", cmd, err, out)
		}
	}
	for cmd, want := range map[string]string{
		"git-upload-pack 'acme/app.git'":   "git-upload-pack /acme/app.git",
		"git-receive-pack '/acme/app'":     "git-receive-pack /acme/app.git",
		"git upload-pack \"acme/app.git\"": "git-upload-pack /acme/app.git",
	} {
		svc, path, ok := parseCommand(cmd)
		if !ok || svc+" "+path != want {
			t.Fatalf("%q parsed as %q %q %v", cmd, svc, path, ok)
		}
	}
}

// BF-51's server half: key exchange uses the active slot's keys, and every connection announces both slots' keys
// with `hostkeys-00@openssh.com`; `hostkeys-prove-00@openssh.com` gets one valid signature per key, bound to the
// session, and a key the server did not announce refuses the proof. Two replicas built from the same secrets serve
// the same host keys.
func TestHostKeysAnnounceAndProve(t *testing.T) {
	b := newBed(t)
	active, _ := parseSlot(b.slots.Slots["A"])
	next, _ := parseSlot(b.slots.Slots["B"])
	if len(active) != 3 || len(next) != 3 {
		t.Fatalf("slots have %d and %d keys", len(active), len(next))
	}
	signer, _ := ssh.ParsePrivateKey(must(os.ReadFile(filepath.Join(b.dir, "id"))))
	for _, k := range active {
		var gotHost ssh.PublicKey
		cfg := &ssh.ClientConfig{
			User:              "git",
			Auth:              []ssh.AuthMethod{ssh.PublicKeys(signer)},
			HostKeyAlgorithms: []string{hostAlgo(k.PublicKey())},
			HostKeyCallback: func(_ string, _ net.Addr, key ssh.PublicKey) error {
				gotHost = key
				return nil
			},
		}
		nc, err := net.Dial("tcp", b.addr)
		if err != nil {
			t.Fatal(err)
		}
		conn, _, reqs, err := ssh.NewClientConn(nc, b.addr, cfg)
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(gotHost.Marshal(), k.PublicKey().Marshal()) {
			t.Fatalf("key exchange served %s, not the active slot's %s key", ssh.FingerprintSHA256(gotHost), k.PublicKey().Type())
		}
		var announced [][]byte
		select {
		case r := <-reqs:
			if r.Type != "hostkeys-00@openssh.com" {
				t.Fatalf("first global request is %s", r.Type)
			}
			for p := r.Payload; len(p) > 0; {
				n := binary.BigEndian.Uint32(p)
				announced = append(announced, p[4:4+n])
				p = p[4+n:]
			}
		case <-time.After(5 * time.Second):
			t.Fatal("no hostkeys-00 announcement")
		}
		go ssh.DiscardRequests(reqs)
		if len(announced) != 6 {
			t.Fatalf("announced %d keys, want both slots' 6", len(announced))
		}
		for _, want := range append(active, next...) {
			if !containsBlob(announced, want.PublicKey().Marshal()) {
				t.Fatalf("slot key %s not announced", ssh.FingerprintSHA256(want.PublicKey()))
			}
		}
		var payload []byte
		for _, blob := range announced {
			payload = append(payload, sshString(blob)...)
		}
		ok, reply, err := conn.SendRequest("hostkeys-prove-00@openssh.com", true, payload)
		if err != nil || !ok {
			t.Fatalf("prove refused: %v", err)
		}
		for i, blob := range announced {
			n := binary.BigEndian.Uint32(reply)
			var sig ssh.Signature
			if err := ssh.Unmarshal(reply[4:4+n], &sig); err != nil {
				t.Fatal(err)
			}
			reply = reply[4+n:]
			pub, _ := ssh.ParsePublicKey(blob)
			data := append(append(sshString([]byte("hostkeys-prove-00@openssh.com")), sshString(conn.SessionID())...), sshString(blob)...)
			if err := pub.Verify(data, &sig); err != nil {
				t.Fatalf("proof %d for %s does not verify: %v", i, pub.Type(), err)
			}
			if pub.Type() == ssh.KeyAlgoRSA && sig.Format == ssh.KeyAlgoRSA {
				t.Fatal("an RSA proof signed with SHA-1")
			}
			// The same proof under another session's id fails, so a proof cannot be replayed.
			other := append(append(sshString([]byte("hostkeys-prove-00@openssh.com")), sshString([]byte("other"))...), sshString(blob)...)
			if pub.Verify(other, &sig) == nil {
				t.Fatal("a proof verified for another session")
			}
		}
		stranger, _ := ssh.NewSignerFromKey(must(ecdsa.GenerateKey(elliptic.P256(), rand.Reader)))
		if ok, _, _ := conn.SendRequest("hostkeys-prove-00@openssh.com", true, sshString(stranger.PublicKey().Marshal())); ok {
			t.Fatal("proved a key the server never announced")
		}
		conn.Close()
	}
	again, err := newServer("", "", b.slots)
	if err != nil {
		t.Fatal(err)
	}
	for i, k := range again.keys.Load().announced {
		if !bytes.Equal(k.PublicKey().Marshal(), b.srv.keys.Load().announced[i].PublicKey().Marshal()) {
			t.Fatal("two replicas from the same secrets announce different keys")
		}
	}
	// After a rotation (slot B active), the keys a client learned from the announcement serve key exchange.
	rotated, err := newServer("", "", hostKeys{Active: "B", Slots: b.slots.Slots})
	if err != nil || len(rotated.keys.Load().announced) != 6 {
		t.Fatalf("rotated server: %v", err)
	}
}

// hostKeyOf dials the server as a new connection, offering only `algo`, and answers the host key key exchange served
// and the keys the server announced.
func hostKeyOf(t *testing.T, b *bed, algo string) (ssh.PublicKey, [][]byte) {
	t.Helper()
	signer, _ := ssh.ParsePrivateKey(must(os.ReadFile(filepath.Join(b.dir, "id"))))
	var got ssh.PublicKey
	cfg := &ssh.ClientConfig{
		User:              "git",
		Auth:              []ssh.AuthMethod{ssh.PublicKeys(signer)},
		HostKeyAlgorithms: []string{algo},
		HostKeyCallback: func(_ string, _ net.Addr, key ssh.PublicKey) error {
			got = key
			return nil
		},
	}
	nc, err := net.Dial("tcp", b.addr)
	if err != nil {
		t.Fatal(err)
	}
	conn, _, reqs, err := ssh.NewClientConn(nc, b.addr, cfg)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	var announced [][]byte
	select {
	case r := <-reqs:
		for p := r.Payload; len(p) > 0; {
			n := binary.BigEndian.Uint32(p)
			announced = append(announced, p[4:4+n])
			p = p[4+n:]
		}
	case <-time.After(5 * time.Second):
		t.Fatal("no hostkeys-00 announcement")
	}
	go ssh.DiscardRequests(reqs)
	return got, announced
}

// A `Rotate { SshHostKeys }` switch with no restart: once `keys.graft` names slot B and a refresh reads it, a new
// connection's key exchange serves slot B's keys, and both slots stay announced. A failed read keeps the set in use,
// and a slot written to Secrets Store later is announced after the next refresh.
func TestRefreshServesTheSwitchedSlot(t *testing.T) {
	b := newBed(t)
	var mu sync.Mutex
	answer, status := hostKeys{Active: "A", Slots: map[string]string{"A": b.slots.Slots["A"], "B": ""}}, 200
	keys := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		defer mu.Unlock()
		if r.URL.Path != "/slots" || status != 200 {
			http.Error(w, "no Release view yet", status)
			return
		}
		json.NewEncoder(w).Encode(answer)
	}))
	t.Cleanup(keys.Close)
	b.srv.keysURL = keys.URL
	set := func(a hostKeys, code int) {
		mu.Lock()
		answer, status = a, code
		mu.Unlock()
	}
	slotA, _ := parseSlot(b.slots.Slots["A"])
	slotB, _ := parseSlot(b.slots.Slots["B"])

	// Slot B not yet written: only A's 3 keys are announced after a refresh.
	if err := b.srv.refresh(); err != nil {
		t.Fatal(err)
	}
	if _, ann := hostKeyOf(t, b, ssh.KeyAlgoED25519); len(ann) != 3 {
		t.Fatalf("announced %d keys with slot B empty, want 3", len(ann))
	}
	// Slot B written: announced within a refresh, still served from A.
	set(hostKeys{Active: "A", Slots: b.slots.Slots}, 200)
	if err := b.srv.refresh(); err != nil {
		t.Fatal(err)
	}
	if got, ann := hostKeyOf(t, b, ssh.KeyAlgoED25519); !bytes.Equal(got.Marshal(), slotA[0].PublicKey().Marshal()) || len(ann) != 6 {
		t.Fatalf("before the switch: served %s, announced %d", ssh.FingerprintSHA256(got), len(ann))
	}
	// The switch: slot B serves key exchange for each algorithm, and both slots are still announced.
	set(hostKeys{Active: "B", Slots: b.slots.Slots}, 200)
	if err := b.srv.refresh(); err != nil {
		t.Fatal(err)
	}
	for _, k := range slotB {
		got, ann := hostKeyOf(t, b, hostAlgo(k.PublicKey()))
		if !bytes.Equal(got.Marshal(), k.PublicKey().Marshal()) {
			t.Fatalf("after the switch key exchange served %s, not slot B's %s", ssh.FingerprintSHA256(got), k.PublicKey().Type())
		}
		if len(ann) != 6 {
			t.Fatalf("after the switch announced %d keys, want 6", len(ann))
		}
	}
	// keys.graft fails (503) or names a slot with no keys: the refresh errs and slot B keeps serving.
	set(hostKeys{}, 503)
	if err := b.srv.refresh(); err == nil {
		t.Fatal("a 503 refreshed")
	}
	set(hostKeys{Active: "A", Slots: map[string]string{"B": b.slots.Slots["B"]}}, 200)
	if err := b.srv.refresh(); err == nil {
		t.Fatal("an empty active slot refreshed")
	}
	if got, _ := hostKeyOf(t, b, ssh.KeyAlgoED25519); !bytes.Equal(got.Marshal(), slotB[0].PublicKey().Marshal()) {
		t.Fatal("a failed refresh changed the served keys")
	}
}

func hostAlgo(k ssh.PublicKey) string {
	if k.Type() == ssh.KeyAlgoRSA {
		return ssh.KeyAlgoRSASHA512
	}
	return k.Type()
}

func containsBlob(list [][]byte, b []byte) bool {
	for _, x := range list {
		if bytes.Equal(x, b) {
			return true
		}
	}
	return false
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

// copyPack stops at the pack's trailer, whatever follows it, for a real pack with offset deltas, ref deltas, and
// objects larger than one read; a pack cut short is an error, not a short copy.
func TestCopyPackStopsAtTheTrailer(t *testing.T) {
	dir := t.TempDir()
	run(t, dir, nil, "git", "init", "-q", "-b", "main", dir)
	id := []string{"GIT_AUTHOR_NAME=s", "GIT_AUTHOR_EMAIL=s@x", "GIT_COMMITTER_NAME=s", "GIT_COMMITTER_EMAIL=s@x"}
	big := make([]byte, 300<<10)
	rand.Read(big)
	for i := 0; i < 5; i++ {
		os.WriteFile(filepath.Join(dir, "big.bin"), append(big, byte(i)), 0o644)
		os.WriteFile(filepath.Join(dir, "t.txt"), bytes.Repeat([]byte(fmt.Sprintf("row %d\n", i)), 2000), 0o644)
		run(t, dir, nil, "git", "add", "-A")
		run(t, dir, id, "git", "commit", "-qm", fmt.Sprint(i))
	}
	for _, flags := range [][]string{{"--delta-base-offset"}, {}} {
		c := exec.Command("git", append([]string{"pack-objects", "--stdout", "--revs"}, flags...)...)
		c.Dir = dir
		c.Stdin = strings.NewReader("main\n")
		pack, err := c.Output()
		if err != nil {
			t.Fatal(err)
		}
		var out bytes.Buffer
		r := bufio.NewReaderSize(bytes.NewReader(append(append([]byte{}, pack...), "0000trailing"...)), 4096)
		if err := copyPack(&out, r); err != nil {
			t.Fatalf("%v: %v", flags, err)
		}
		if !bytes.Equal(out.Bytes(), pack) {
			t.Fatalf("%v: copied %d bytes of a %d-byte pack", flags, out.Len(), len(pack))
		}
		rest := make([]byte, 64)
		n, _ := r.Read(rest)
		if string(rest[:n]) != "0000trailing" {
			t.Fatalf("%v: read past the trailer, left %q", flags, rest[:n])
		}
		if err := copyPack(&bytes.Buffer{}, bufio.NewReader(bytes.NewReader(pack[:len(pack)-30]))); err == nil {
			t.Fatalf("%v: a cut pack copied without error", flags)
		}
	}
}
