// graft-sshd, the SSH front's server (design/DESIGN.md, Shape, "SSH front"; build/BUILD-PLAN.md, M6-WP2). It runs in the
// `sshfront` container behind each site's `SshFront` Durable Object, which pipes it the sockets the edge's `connect()`
// handler receives from the install's Spectrum app on port 22. It holds no repository, no git binary, and no secret
// at rest: the host keys come from `keys.graft` at start, a client key is checked by asking the edge, and every git
// request is the smart-HTTP request an HTTPS client makes, sent to the edge through `git.graft` with the verified key's
// fingerprint, so a push over SSH takes the HTTPS write path (staging, validate, place, commit, report-status).
// `keys.graft` and `git.graft` are outbound intercepts the object registers; they reach `SshOutbound` in the site
// script, which reads Secrets Store and calls the edge over a service binding whose props mark the call as the front's.
package main

import (
	"bufio"
	"bytes"
	"compress/zlib"
	"crypto/rand"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"strings"
	"time"

	"golang.org/x/crypto/ssh"
)

const (
	// The version string the edge waits for before it forwards a client byte (any "SSH-" banner counts).
	serverVersion = "SSH-2.0-graftyard"
	// B0-24a's front: a 30 s login grace, and client keepalives every 15 s with 4 misses (tests/m0/wp7/front/sshd_config).
	loginGrace   = 30 * time.Second
	keepalive    = 15 * time.Second
	keepaliveMax = 4
	// A key lookup is tried for up to 20 s, inside the login grace: B0-24a saw one outbound call fail alone and the
	// key's group unreachable for about 23 s (tests/m0/results/m0-wp7.md, B0-24a) [guess for the budget].
	lookupBudget = 20 * time.Second
	lookupTry    = 5 * time.Second
	userAgent    = "git/graft-sshd"
	zeroID       = "0000000000000000000000000000000000000000"
)

type server struct {
	keysURL, gitURL string
	http            *http.Client
	config          *ssh.ServerConfig
	// Every host key of both slots, announced with `hostkeys-00@openssh.com` and proved on request.
	announced []ssh.Signer
	// How long a key lookup is retried; a test shortens it.
	lookupBudget time.Duration
}

// hostKeys is `keys.graft`'s answer: each slot is the PEM text of its OpenSSH private keys (one Ed25519, one ECDSA
// P-256, one RSA 3072, as deploy/ssh-front/hostkeys.mjs writes them), and `active` names the slot key exchange uses.
type hostKeys struct {
	Active string            `json:"active"`
	Slots  map[string]string `json:"slots"`
}

func parseSlot(text string) ([]ssh.Signer, error) {
	var out []ssh.Signer
	rest := []byte(text)
	for {
		var b *pem.Block
		b, rest = pem.Decode(rest)
		if b == nil {
			return out, nil
		}
		k, err := ssh.ParsePrivateKey(pem.EncodeToMemory(b))
		if err != nil {
			return nil, err
		}
		out = append(out, k)
	}
}

// newServer builds the server from `keys.graft`'s answer: the active slot's keys serve key exchange, one per
// algorithm, and both slots' keys are announced, so every replica serves the same fingerprints.
func newServer(keysURL, gitURL string, keys hostKeys) (*server, error) {
	s := &server{keysURL: keysURL, gitURL: strings.TrimSuffix(gitURL, "/"), http: &http.Client{}, lookupBudget: lookupBudget}
	active, err := parseSlot(keys.Slots[keys.Active])
	if err != nil || len(active) == 0 {
		return nil, fmt.Errorf("active host-key slot %q: %v (%d keys)", keys.Active, err, len(active))
	}
	s.config = &ssh.ServerConfig{ServerVersion: serverVersion, PublicKeyCallback: s.authenticate}
	for _, k := range active {
		s.config.AddHostKey(k)
	}
	seen := map[string]bool{}
	for _, slot := range []string{"A", "B"} {
		ks, err := parseSlot(keys.Slots[slot])
		if err != nil {
			return nil, fmt.Errorf("host-key slot %s: %v", slot, err)
		}
		for _, k := range ks {
			if b := string(k.PublicKey().Marshal()); !seen[b] {
				seen[b] = true
				s.announced = append(s.announced, k)
			}
		}
	}
	return s, nil
}

// fetchKeys reads `keys.graft` until it answers: the object registers the intercept before it starts the container,
// but its first call can still fail (B0-24d: a cold container's first connection closed with no bytes).
func fetchKeys(keysURL string) (hostKeys, error) {
	var last error
	for end := time.Now().Add(60 * time.Second); time.Now().Before(end); time.Sleep(time.Second) {
		c := &http.Client{Timeout: 5 * time.Second}
		r, err := c.Get(keysURL + "/slots")
		if err != nil {
			last = err
			continue
		}
		var k hostKeys
		err = json.NewDecoder(r.Body).Decode(&k)
		r.Body.Close()
		if r.StatusCode == 200 && err == nil {
			return k, nil
		}
		last = fmt.Errorf("HTTP %d %v", r.StatusCode, err)
	}
	return hostKeys{}, fmt.Errorf("keys.graft did not answer in 60 s: %v", last)
}

var errRefused = errors.New("graft-sshd: key refused")

// authenticate asks the edge for the key's `Tokens` row on its `fingerprint_shard` (read through `fresh`). Only the
// user `git` is served, as on github.com. An unknown, removed, or deploy key is 404, which is final; anything else is
// retried for `lookupBudget`.
func (s *server) authenticate(meta ssh.ConnMetadata, key ssh.PublicKey) (*ssh.Permissions, error) {
	if meta.User() != "git" {
		return nil, errRefused
	}
	sum := sha256.Sum256(key.Marshal())
	fp := hex.EncodeToString(sum[:])
	var last error
	for end := time.Now().Add(s.lookupBudget); ; time.Sleep(time.Second) {
		code, err := s.lookup(fp)
		switch {
		case err == nil && code == 200:
			return &ssh.Permissions{Extensions: map[string]string{"fp": fp}}, nil
		case err == nil && code == 404:
			return nil, errRefused
		case err == nil:
			last = fmt.Errorf("HTTP %d", code)
		default:
			last = err
		}
		if !time.Now().Before(end) {
			log.Printf("key lookup gave up: %v", last)
			return nil, errRefused
		}
	}
}

func (s *server) lookup(fp string) (int, error) {
	req, _ := http.NewRequest("GET", s.gitURL+"/_graftyard/ssh/key?fp="+fp, nil)
	req.Header.Set("user-agent", userAgent)
	c := *s.http
	c.Timeout = lookupTry
	r, err := c.Do(req)
	if err != nil {
		return 0, err
	}
	io.Copy(io.Discard, r.Body)
	r.Body.Close()
	return r.StatusCode, nil
}

func (s *server) serve(nc net.Conn) {
	start := time.Now()
	nc.SetDeadline(start.Add(loginGrace))
	conn, chans, reqs, err := ssh.NewServerConn(nc, s.config)
	if err != nil {
		nc.Close()
		return
	}
	nc.SetDeadline(time.Time{})
	defer conn.Close()
	go s.globals(conn, reqs)
	go s.keepalive(conn)
	s.announce(conn)
	for nch := range chans {
		if nch.ChannelType() != "session" {
			nch.Reject(ssh.UnknownChannelType, "graft-sshd serves git sessions only")
			continue
		}
		ch, creqs, err := nch.Accept()
		if err != nil {
			continue
		}
		go s.session(conn, ch, creqs)
	}
	log.Printf("connection closed after %s", time.Since(start).Round(time.Millisecond))
}

func sshString(b []byte) []byte {
	out := make([]byte, 4, 4+len(b))
	binary.BigEndian.PutUint32(out, uint32(len(b)))
	return append(out, b...)
}

// announce sends both slots' public keys (OpenSSH's PROTOCOL, "hostkeys-00@openssh.com"), so a client with
// `UpdateHostKeys` learns the next slot before the rotation makes it active (BF-51).
func (s *server) announce(conn *ssh.ServerConn) {
	var p []byte
	for _, k := range s.announced {
		p = append(p, sshString(k.PublicKey().Marshal())...)
	}
	conn.SendRequest("hostkeys-00@openssh.com", false, p)
}

// prove answers "hostkeys-prove-00@openssh.com": one signature per requested key over the request name, the session
// identifier, and the key, by that key. An RSA key signs with the RSA algorithm key exchange negotiated, else
// rsa-sha2-512, as OpenSSH's server does. A key the server did not announce refuses the request.
func (s *server) prove(conn *ssh.ServerConn, payload []byte) ([]byte, bool) {
	alg := ""
	if a, ok := ssh.ConnMetadata(conn).(ssh.AlgorithmsConnMetadata); ok {
		alg = a.Algorithms().HostKey
	}
	var out []byte
	for len(payload) > 0 {
		if len(payload) < 4 {
			return nil, false
		}
		n := binary.BigEndian.Uint32(payload)
		if uint64(len(payload)-4) < uint64(n) {
			return nil, false
		}
		blob := payload[4 : 4+n]
		payload = payload[4+n:]
		var key ssh.Signer
		for _, k := range s.announced {
			if bytes.Equal(k.PublicKey().Marshal(), blob) {
				key = k
			}
		}
		if key == nil {
			return nil, false
		}
		data := append(append(sshString([]byte("hostkeys-prove-00@openssh.com")), sshString(conn.SessionID())...),
			sshString(blob)...)
		var sig *ssh.Signature
		var err error
		if as, ok := key.(ssh.AlgorithmSigner); ok && key.PublicKey().Type() == ssh.KeyAlgoRSA {
			a := ssh.KeyAlgoRSASHA512
			if alg == ssh.KeyAlgoRSASHA256 || alg == ssh.KeyAlgoRSASHA512 {
				a = alg
			}
			sig, err = as.SignWithAlgorithm(rand.Reader, data, a)
		} else {
			sig, err = key.Sign(rand.Reader, data)
		}
		if err != nil {
			return nil, false
		}
		out = append(out, sshString(ssh.Marshal(sig))...)
	}
	return out, true
}

func (s *server) globals(conn *ssh.ServerConn, reqs <-chan *ssh.Request) {
	for r := range reqs {
		if r.Type == "hostkeys-prove-00@openssh.com" {
			p, ok := s.prove(conn, r.Payload)
			r.Reply(ok, p)
		} else if r.WantReply {
			r.Reply(false, nil)
		}
	}
}

// keepalive closes a connection whose client missed `keepaliveMax` keepalives in a row. A refusal is an answer.
func (s *server) keepalive(conn *ssh.ServerConn) {
	misses := 0
	t := time.NewTicker(keepalive)
	defer t.Stop()
	for range t.C {
		done := make(chan error, 1)
		go func() { _, _, err := conn.SendRequest("keepalive@openssh.com", true, nil); done <- err }()
		select {
		case err := <-done:
			if err != nil {
				return
			}
			misses = 0
		case <-time.After(keepalive):
			if misses++; misses >= keepaliveMax {
				log.Printf("closing a connection after %d missed keepalives", misses)
				conn.Close()
				return
			}
		}
	}
}

func (s *server) session(conn *ssh.ServerConn, ch ssh.Channel, reqs <-chan *ssh.Request) {
	defer ch.Close()
	proto := ""
	exit := func(code uint32) { ch.SendRequest("exit-status", false, ssh.Marshal(struct{ C uint32 }{code})) }
	for r := range reqs {
		switch r.Type {
		case "env":
			var kv struct{ Name, Value string }
			if ssh.Unmarshal(r.Payload, &kv) == nil && kv.Name == "GIT_PROTOCOL" {
				proto = kv.Value
			}
			r.Reply(true, nil)
		case "exec":
			var c struct{ Command string }
			if ssh.Unmarshal(r.Payload, &c) != nil {
				r.Reply(false, nil)
				return
			}
			r.Reply(true, nil)
			go ssh.DiscardRequests(reqs)
			exit(s.exec(conn.Permissions.Extensions["fp"], c.Command, proto, ch))
			return
		case "shell":
			r.Reply(true, nil)
			go ssh.DiscardRequests(reqs)
			fmt.Fprint(ch.Stderr(), "Hi! You've successfully authenticated, but Graftyard does not provide shell access.\r\n")
			exit(1)
			return
		default:
			if r.WantReply {
				r.Reply(false, nil)
			}
		}
	}
}

// parseCommand reads `git-upload-pack 'owner/repo.git'` (or `git upload-pack`, with or without the leading slash, the
// quotes, or `.git`) into the service and the repository's smart-HTTP path `/owner/repo.git`.
func parseCommand(cmd string) (service, path string, ok bool) {
	cmd = strings.TrimSpace(cmd)
	for _, svc := range []string{"git-upload-pack", "git-receive-pack"} {
		for _, prefix := range []string{svc + " ", "git " + strings.TrimPrefix(svc, "git-") + " "} {
			if arg, found := strings.CutPrefix(cmd, prefix); found {
				arg = strings.TrimSpace(arg)
				if len(arg) >= 2 && (arg[0] == '\'' && arg[len(arg)-1] == '\'' || arg[0] == '"' && arg[len(arg)-1] == '"') {
					arg = arg[1 : len(arg)-1]
				}
				arg = strings.TrimPrefix(arg, "/")
				owner, repo, found := strings.Cut(arg, "/")
				repo = strings.TrimSuffix(repo, ".git")
				valid := func(s string) bool {
					return s != "" && s != "." && s != ".." && !strings.ContainsAny(s, "/\\?#%' \"")
				}
				if !found || !valid(owner) || !valid(repo) {
					return "", "", false
				}
				return svc, "/" + owner + "/" + repo + ".git", true
			}
		}
	}
	return "", "", false
}

func (s *server) exec(fp, command, proto string, ch ssh.Channel) uint32 {
	svc, path, ok := parseCommand(command)
	if !ok {
		fmt.Fprintf(ch.Stderr(), "ERROR: graft-sshd runs only git-upload-pack and git-receive-pack on owner/repo\r\n")
		return 1
	}
	start := time.Now()
	var code uint32
	if svc == "git-upload-pack" {
		code = s.uploadPack(fp, path, proto, ch)
	} else {
		code = s.receivePack(fp, path, ch)
	}
	log.Printf("%s %s key %s: %d in %s", svc, path, fp[:12], code, time.Since(start).Round(time.Millisecond))
	return code
}

func (s *server) request(method, path, fp, proto, contentType string, body io.Reader) (*http.Response, error) {
	req, err := http.NewRequest(method, s.gitURL+path, body)
	if err != nil {
		return nil, err
	}
	req.Header.Set("user-agent", userAgent)
	req.Header.Set("x-graftyard-ssh-key", fp)
	if proto != "" {
		req.Header.Set("git-protocol", proto)
	}
	if contentType != "" {
		req.Header.Set("content-type", contentType)
	}
	return s.http.Do(req)
}

// refuse shows the edge's answer as the remote's error, which git prints before "Could not read from remote
// repository", and is the exit code git sees for a failed service.
func refuse(ch ssh.Channel, r *http.Response, err error) uint32 {
	msg := ""
	if err != nil {
		msg = "the Graftyard edge did not answer; retry"
	} else {
		b, _ := io.ReadAll(io.LimitReader(r.Body, 4096))
		r.Body.Close()
		msg = strings.TrimSpace(string(b))
		if msg == "" {
			msg = fmt.Sprintf("HTTP %d", r.StatusCode)
		}
	}
	fmt.Fprintf(ch.Stderr(), "ERROR: %s\r\n", msg)
	return 128
}

func wantsV2(proto string) bool {
	for _, kv := range strings.Split(proto, ":") {
		if strings.TrimSpace(kv) == "version=2" {
			return true
		}
	}
	return false
}

// readPkt reads one pkt-line, appending its raw bytes to `raw`, and answers its length field (0 flush, 1 delim, 2
// response-end) or the payload length plus 4.
func readPkt(r *bufio.Reader, raw *bytes.Buffer) (int, error) {
	var h [4]byte
	if _, err := io.ReadFull(r, h[:]); err != nil {
		return 0, err
	}
	var n [2]byte
	if _, err := hex.Decode(n[:], h[:]); err != nil {
		return 0, fmt.Errorf("bad pkt-line length %q", h[:])
	}
	size := int(n[0])<<8 | int(n[1])
	raw.Write(h[:])
	if size <= 2 {
		return size, nil
	}
	if size < 4 {
		return 0, fmt.Errorf("bad pkt-line length %d", size)
	}
	if _, err := io.CopyN(raw, r, int64(size-4)); err != nil {
		return 0, err
	}
	return size, nil
}

// stripPreamble drops smart HTTP's `# service=...` line and its flush, which the SSH transport does not send.
func stripPreamble(b []byte) []byte {
	if len(b) >= 4+len("# service=") && string(b[4:4+len("# service=")]) == "# service=" {
		var n [2]byte
		if _, err := hex.Decode(n[:], b[:4]); err == nil {
			size := int(n[0])<<8 | int(n[1])
			if size <= len(b) && bytes.HasPrefix(b[size:], []byte("0000")) {
				return b[size+4:]
			}
		}
	}
	return b
}

// uploadPack serves protocol v2 only: the capability advertisement from the edge's `info/refs`, then each command
// request (up to its flush) as one stateless `git-upload-pack` POST, until the client sends a bare flush or closes.
// v0 fetch over SSH holds a negotiation the stateless HTTP requests do not, so it is refused with what to set.
func (s *server) uploadPack(fp, path, proto string, ch ssh.Channel) uint32 {
	if !wantsV2(proto) {
		fmt.Fprint(ch.Stderr(), "ERROR: Graftyard serves fetch over SSH in protocol v2 only; use git 2.26 or later, or set "+
			"protocol.version=2 (git sends GIT_PROTOCOL=version=2)\r\n")
		return 128
	}
	r, err := s.request("GET", path+"/info/refs?service=git-upload-pack", fp, proto, "", nil)
	if err != nil || r.StatusCode != 200 {
		return refuse(ch, r, err)
	}
	adv, err := io.ReadAll(r.Body)
	r.Body.Close()
	if err != nil {
		return refuse(ch, nil, err)
	}
	if _, err := ch.Write(stripPreamble(adv)); err != nil {
		return 128
	}
	in := bufio.NewReaderSize(ch, 64<<10)
	for {
		var req bytes.Buffer
		lines := 0
		for {
			n, err := readPkt(in, &req)
			if err == io.EOF && req.Len() == 0 {
				return 0
			}
			if err != nil {
				return 128
			}
			if n == 0 {
				break
			}
			lines++
		}
		if lines == 0 {
			return 0
		}
		r, err := s.request("POST", path+"/git-upload-pack", fp, proto, "application/x-git-upload-pack-request", &req)
		if err != nil || r.StatusCode != 200 {
			return refuse(ch, r, err)
		}
		_, err = io.Copy(ch, r.Body)
		r.Body.Close()
		if err != nil {
			return 128
		}
	}
}

// receivePack serves v0 receive-pack, as git speaks it over every transport: the edge's advertisement, then one
// `git-receive-pack` POST of the client's commands, push options, and pack, then the edge's report back. The pack's
// end is found by reading it (`copyPack`), so the request ends whether or not the client closes its side.
func (s *server) receivePack(fp, path string, ch ssh.Channel) uint32 {
	r, err := s.request("GET", path+"/info/refs?service=git-receive-pack", fp, "", "", nil)
	if err != nil || r.StatusCode != 200 {
		return refuse(ch, r, err)
	}
	adv, err := io.ReadAll(r.Body)
	r.Body.Close()
	if err != nil {
		return refuse(ch, nil, err)
	}
	if _, err := ch.Write(stripPreamble(adv)); err != nil {
		return 128
	}
	in := bufio.NewReaderSize(ch, 64<<10)
	var head bytes.Buffer
	pack, options, commands := false, false, 0
	for {
		at := head.Len()
		n, err := readPkt(in, &head)
		if err == io.EOF && head.Len() == 0 {
			return 0
		}
		if err != nil {
			return 128
		}
		if n == 0 {
			break
		}
		line := head.Bytes()[at+4:]
		if commands == 0 {
			if i := bytes.IndexByte(line, 0); i >= 0 {
				options = bytes.Contains(append(line[i:], ' '), []byte(" push-options "))
				line = line[:i]
			}
		}
		commands++
		if f := strings.Fields(string(line)); len(f) >= 2 && f[1] != zeroID {
			pack = true
		}
	}
	// A bare flush: nothing to update.
	if commands == 0 {
		return 0
	}
	if options {
		for {
			n, err := readPkt(in, &head)
			if err != nil {
				return 128
			}
			if n == 0 {
				break
			}
		}
	}
	body := io.Reader(&head)
	var packErr chan error
	if pack {
		pr, pw := io.Pipe()
		packErr = make(chan error, 1)
		go func() {
			err := copyPack(pw, in)
			pw.CloseWithError(err)
			packErr <- err
		}()
		body = io.MultiReader(&head, pr)
	}
	r, err = s.request("POST", path+"/git-receive-pack", fp, "", "application/x-git-receive-pack-request", body)
	if packErr != nil {
		if perr := <-packErr; perr != nil && err == nil {
			log.Printf("pack: %v", perr)
		}
	}
	if err != nil || r.StatusCode != 200 {
		return refuse(ch, r, err)
	}
	_, err = io.Copy(ch, r.Body)
	r.Body.Close()
	if err != nil {
		return 128
	}
	return 0
}

// byteTee is a flate.Reader over `r` that writes every byte it consumes to `w`, so zlib reads exactly one object's
// stream and no further (compress/flate reads ahead only from a reader that is not an io.ByteReader).
type byteTee struct {
	r *bufio.Reader
	w *bufio.Writer
}

func (t byteTee) Read(p []byte) (int, error) {
	n, err := t.r.Read(p)
	t.w.Write(p[:n])
	return n, err
}

func (t byteTee) ReadByte() (byte, error) {
	b, err := t.r.ReadByte()
	if err == nil {
		t.w.WriteByte(b)
	}
	return b, err
}

// copyPack copies one SHA-1 pack from `r` to `w` and stops at its trailer: the header and object count, each object's
// type and size header, an offset delta's offset or a ref delta's base id, its zlib stream, then the 20-byte trailer.
// The edge checks the pack itself (index-pack on the shard); this only finds where it ends.
func copyPack(w io.Writer, r *bufio.Reader) error {
	bw := bufio.NewWriterSize(w, 64<<10)
	t := byteTee{r, bw}
	var hdr [12]byte
	if _, err := io.ReadFull(t, hdr[:]); err != nil {
		return fmt.Errorf("pack header: %w", err)
	}
	if string(hdr[:4]) != "PACK" {
		return fmt.Errorf("not a pack")
	}
	count := binary.BigEndian.Uint32(hdr[8:])
	for i := uint32(0); i < count; i++ {
		b, err := t.ReadByte()
		if err != nil {
			return err
		}
		kind := (b >> 4) & 7
		for b&0x80 != 0 {
			if b, err = t.ReadByte(); err != nil {
				return err
			}
		}
		switch kind {
		case 6:
			for {
				if b, err = t.ReadByte(); err != nil {
					return err
				}
				if b&0x80 == 0 {
					break
				}
			}
		case 7:
			if _, err := io.CopyN(io.Discard, t, 20); err != nil {
				return err
			}
		}
		z, err := zlib.NewReader(t)
		if err != nil {
			return fmt.Errorf("object %d: %w", i, err)
		}
		if _, err := io.Copy(io.Discard, z); err != nil {
			return fmt.Errorf("object %d: %w", i, err)
		}
		z.Close()
	}
	if _, err := io.CopyN(io.Discard, t, 20); err != nil {
		return fmt.Errorf("pack trailer: %w", err)
	}
	return bw.Flush()
}

func main() {
	keysURL := env("GRAFT_KEYS_URL", "http://keys.graft")
	gitURL := env("GRAFT_GIT_URL", "http://git.graft")
	listen := env("GRAFT_LISTEN", ":22")
	keys, err := fetchKeys(keysURL)
	if err != nil {
		log.Fatal(err)
	}
	s, err := newServer(keysURL, gitURL, keys)
	if err != nil {
		log.Fatal(err)
	}
	l, err := net.Listen("tcp", listen)
	if err != nil {
		log.Fatal(err)
	}
	log.Printf("graft-sshd on %s: slot %s active, %d host keys announced", listen, keys.Active, len(s.announced))
	for {
		c, err := l.Accept()
		if err != nil {
			log.Fatal(err)
		}
		go s.serve(c)
	}
}

func env(k, def string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return def
}
