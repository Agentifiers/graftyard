// hostd: M0-WP14's stand-in Graftyard host git backend, run in the host container with enableInternet:false. It seeds
// three bare repos under /data (base, fork, other) with one commit each, then serves git smart HTTP through
// `git http-backend` with the receive-pack service enabled, so a push that the router lets through would actually
// write. The router refuses every push, so the push target's acceptance is what the containment row disproves.
//
//	GET  /health
//	ANY  /<owner>/<repo>.git/...   git smart HTTP via git-http-backend (GET info/refs, POST upload-pack/receive-pack)
package main

import (
	"fmt"
	"net/http"
	"net/http/cgi"
	"os"
	"os/exec"
	"strings"
)

const root = "/data"

func seed() error {
	for _, name := range []string{"acme/app", "acme-agents/app", "acme/secrets"} {
		dir := root + "/" + name + ".git"
		if _, err := os.Stat(dir); err == nil {
			continue
		}
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return err
		}
		if out, err := exec.Command("git", "init", "--bare", "-b", "main", dir).CombinedOutput(); err != nil {
			return fmt.Errorf("init %s: %v %s", name, err, out)
		}
		// Enable push over HTTP for the bare repo.
		exec.Command("git", "-C", dir, "config", "http.receivepack", "true").Run()
		// Seed one commit via a scratch worktree so clone/fetch return real data.
		work := root + "/seed-" + strings.ReplaceAll(name, "/", "-")
		os.RemoveAll(work)
		if out, err := exec.Command("git", "clone", dir, work).CombinedOutput(); err != nil {
			return fmt.Errorf("clone %s: %v %s", name, err, out)
		}
		os.WriteFile(work+"/README.md", []byte("seed of "+name+"\n"), 0o644)
		if name == "acme/secrets" {
			os.WriteFile(work+"/TOKEN", []byte("super-secret-other-repo-token\n"), 0o644)
		}
		run := func(args ...string) error {
			c := exec.Command("git", args...)
			c.Dir = work
			c.Env = append(os.Environ(),
				"GIT_AUTHOR_NAME=seed", "GIT_AUTHOR_EMAIL=seed@wp14",
				"GIT_COMMITTER_NAME=seed", "GIT_COMMITTER_EMAIL=seed@wp14")
			if out, err := c.CombinedOutput(); err != nil {
				return fmt.Errorf("%v: %v %s", args, err, out)
			}
			return nil
		}
		if err := run("add", "-A"); err != nil {
			return err
		}
		if err := run("commit", "-m", "seed"); err != nil {
			return err
		}
		if err := run("push", "origin", "main"); err != nil {
			return err
		}
		os.RemoveAll(work)
	}
	return nil
}

func main() {
	if err := seed(); err != nil {
		fmt.Fprintln(os.Stderr, "seed:", err)
		os.Exit(1)
	}
	mux := http.NewServeMux()
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) { w.Write([]byte("ok")) })
	// git-http-backend serves everything under /data. Export all repos; receive-pack is on.
	backend := &cgi.Handler{
		Path: gitPath(),
		Args: []string{"http-backend"},
		Env: []string{
			"GIT_PROJECT_ROOT=" + root,
			"GIT_HTTP_EXPORT_ALL=1",
		},
	}
	mux.Handle("/", backend)
	addr := os.Getenv("HOSTD_ADDR")
	if addr == "" {
		addr = "0.0.0.0:8080"
	}
	fmt.Println("hostd on", addr)
	http.ListenAndServe(addr, mux)
}

func gitPath() string {
	for _, p := range []string{"/usr/bin/git", "/usr/local/bin/git"} {
		if _, err := os.Stat(p); err == nil {
			return p
		}
	}
	return "git"
}
