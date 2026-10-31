// sink: the B0-22 container's server.
//   GET /health            answers once the server is up
//   PUT /f?path=P          writes the request body to /data/P
//   GET /f?path=P          serves /data/P with its Content-Length (the object streams it to R2 when seeding)
//   POST /upload-pack?repo=R  protocol v2 `git upload-pack --stateless-rpc /data/R` (B0-23)
// `sink gen MB FILES_PER_COMMIT` writes a git fast-import stream of about MB megabytes of random blobs, 20 to 400 KB
// each, so seeding a synthetic network happens inside the container.
package main

import (
	"bufio"
	"bytes"
	"fmt"
	"io"
	"math/rand"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"
)

func gen(mb, perCommit int) {
	w := bufio.NewWriterSize(os.Stdout, 1<<20)
	defer w.Flush()
	rng := rand.New(rand.NewSource(1))
	buf := make([]byte, 400<<10)
	total, mark, commit := int64(0), 1, 0
	files := map[string]int{}
	for total < int64(mb)<<20 {
		var paths []string
		for i := 0; i < perCommit && total < int64(mb)<<20; i++ {
			n := 20<<10 + rng.Intn(380<<10)
			rng.Read(buf[:n])
			fmt.Fprintf(w, "blob\nmark :%d\ndata %d\n", mark, n)
			w.Write(buf[:n])
			w.WriteString("\n")
			p := fmt.Sprintf("d%03d/f%05d.bin", rng.Intn(200), rng.Intn(20000))
			files[p] = mark
			paths = append(paths, p)
			mark++
			total += int64(n)
		}
		commit++
		fmt.Fprintf(w, "commit refs/heads/main\nmark :%d\ncommitter m0 <m0@example.com> %d +0000\ndata 8\ncommit %d\n", mark, 1700000000+commit, commit%10)
		mark++
		for _, p := range paths {
			fmt.Fprintf(w, "M 100644 :%d %s\n", files[p], p)
		}
		w.WriteString("\n")
	}
}

// mono writes a fast-import stream for B0-23's monorepo: one commit of FILES text files of 1 to 4 KB, then COMMITS
// commits that each rewrite 3 files, so each later commit is one tip the clients clone.
func mono(nfiles, commits int) {
	w := bufio.NewWriterSize(os.Stdout, 1<<20)
	defer w.Flush()
	rng := rand.New(rand.NewSource(2))
	const b64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
	line := make([]byte, 77)
	blob := func(path string) {
		n := 1024 + rng.Intn(3072)
		var sb strings.Builder
		for sb.Len() < n {
			for i := 0; i < 76; i++ {
				line[i] = b64[rng.Intn(64)]
			}
			line[76] = '\n'
			sb.Write(line)
		}
		fmt.Fprintf(w, "M 100644 inline %s\ndata %d\n%s\n", path, sb.Len(), sb.String())
	}
	path := func(i int) string { return fmt.Sprintf("svc%02d/pkg%03d/f%05d.txt", i%40, (i/40)%250, i) }
	for c := 0; c <= commits; c++ {
		fmt.Fprintf(w, "commit refs/heads/main\ncommitter m0 <m0@example.com> %d +0000\ndata 8\ncommit %d\n", 1700000000+c, c%10)
		if c == 0 {
			for i := 0; i < nfiles; i++ {
				blob(path(i))
			}
		} else {
			for k := 0; k < 3; k++ {
				blob(path(rng.Intn(nfiles)))
			}
		}
		w.WriteString("\n")
	}
}

func main() {
	if len(os.Args) > 1 && os.Args[1] == "mono" {
		n, _ := strconv.Atoi(os.Args[2])
		c, _ := strconv.Atoi(os.Args[3])
		mono(n, c)
		return
	}
	if len(os.Args) > 1 && os.Args[1] == "gen" {
		mb, _ := strconv.Atoi(os.Args[2])
		per, _ := strconv.Atoi(os.Args[3])
		gen(mb, per)
		return
	}
	// B0-23 serves dozens of concurrent streams; children (git) inherit the raised limit.
	lim := syscall.Rlimit{Cur: 1 << 16, Max: 1 << 16}
	if err := syscall.Setrlimit(syscall.RLIMIT_NOFILE, &lim); err != nil {
		fmt.Fprintln(os.Stderr, "setrlimit:", err)
	}
	http.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) { fmt.Fprint(w, "ok") })
	http.HandleFunc("/f", func(w http.ResponseWriter, r *http.Request) {
		p := filepath.Clean("/data/" + r.URL.Query().Get("path"))
		if !strings.HasPrefix(p, "/data/") {
			http.Error(w, "bad path", 400)
			return
		}
		if r.Method == http.MethodGet {
			http.ServeFile(w, r, p)
			return
		}
		t := time.Now()
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			http.Error(w, err.Error(), 500)
			return
		}
		f, err := os.Create(p + ".part")
		if err != nil {
			http.Error(w, err.Error(), 500)
			return
		}
		n, err := io.CopyBuffer(f, r.Body, make([]byte, 1<<20))
		if cerr := f.Close(); err == nil {
			err = cerr
		}
		if err == nil {
			err = os.Rename(p+".part", p)
		}
		if err != nil {
			http.Error(w, err.Error(), 500)
			return
		}
		fmt.Fprintf(w, `{"bytes":%d,"ms":%d}`, n, time.Since(t).Milliseconds())
	})
	// B0-23: protocol v2 `fetch` against /data/REPO, the request body in and the response streamed out.
	http.HandleFunc("/upload-pack", func(w http.ResponseWriter, r *http.Request) {
		cmd := exec.Command("git", "upload-pack", "--stateless-rpc", filepath.Join("/data", filepath.Base(r.URL.Query().Get("repo"))))
		cmd.Env = append(os.Environ(), "GIT_PROTOCOL=version=2")
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
	http.HandleFunc("/clone", cloneHandler)
	http.HandleFunc("/wait", func(w http.ResponseWriter, r *http.Request) {
		p := "/data/cc/" + filepath.Base(r.URL.Query().Get("key"))
		for i := 0; i < 6000; i++ {
			if b, err := os.ReadFile(p + ".done"); err == nil {
				w.Write(b)
				return
			}
			time.Sleep(10 * time.Millisecond)
		}
		http.Error(w, "not done", 504)
	})
	http.ListenAndServe(":8080", nil)
}

// B0-23 clone cache, the shard side. POST /clone?repo=R&key=K with a protocol v2 fetch request as the body.
// The first request for K runs one `git upload-pack` on that body into /data/cc/K; every request for K, the first included, streams
// that file as it grows (x-cc: gen for the one that started it, follow for the others, done once it is complete), so
// concurrent misses on one tip cost one upload-pack. /data/cc/K.done holds the final size.
var (
	ccMu       sync.Mutex
	ccInflight = map[string]bool{}
)

func cloneHandler(w http.ResponseWriter, r *http.Request) {
	q := r.URL.Query()
	key := filepath.Base(q.Get("key"))
	body, _ := io.ReadAll(r.Body)
	repo := filepath.Join("/data", filepath.Base(q.Get("repo")))
	p := "/data/cc/" + key
	os.MkdirAll("/data/cc", 0o755)
	ccMu.Lock()
	state := "follow"
	if _, err := os.Stat(p + ".done"); err == nil {
		state = "done"
	} else if !ccInflight[key] {
		ccInflight[key] = true
		state = "gen"
		f, err := os.Create(p)
		if err != nil {
			ccMu.Unlock()
			http.Error(w, err.Error(), 500)
			return
		}
		go func() {
			defer f.Close()
			cmd := exec.Command("git", "upload-pack", "--stateless-rpc", repo)
			cmd.Env = append(os.Environ(), "GIT_PROTOCOL=version=2")
			cmd.Stdin = bytes.NewReader(body)
			cmd.Stdout = f
			err := cmd.Run()
			st, _ := f.Stat()
			msg := fmt.Sprintf(`{"size":%d}`, st.Size())
			if err != nil {
				msg = fmt.Sprintf(`{"size":%d,"error":%q}`, st.Size(), err.Error())
			}
			os.WriteFile(p+".done", []byte(msg), 0o644)
			ccMu.Lock()
			delete(ccInflight, key)
			ccMu.Unlock()
		}()
	}
	ccMu.Unlock()
	f, err := os.Open(p)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}
	defer f.Close()
	w.Header().Set("content-type", "application/x-git-upload-pack-result")
	w.Header().Set("x-cc", state)
	fl, _ := w.(http.Flusher)
	buf := make([]byte, 1<<16)
	var off int64
	for {
		n, _ := f.Read(buf)
		if n > 0 {
			if _, err := w.Write(buf[:n]); err != nil {
				return
			}
			off += int64(n)
			if fl != nil {
				fl.Flush()
			}
			continue
		}
		if b, err := os.ReadFile(p + ".done"); err == nil {
			var size int64
			fmt.Sscanf(string(b), `{"size":%d`, &size)
			if off >= size {
				return
			}
			continue
		}
		time.Sleep(5 * time.Millisecond)
	}
}
