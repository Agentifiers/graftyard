// M0-WP15, L0-08: the official Go SDK client (github.com/modelcontextprotocol/go-sdk v1.8.0) against graft-mcp,
// unmodified, at protocol 2026-07-28. auth.AuthorizationCodeHandler does RFC 9728 discovery, RFC 8414, CIMD,
// PKCE S256, RFC 8707 resource, and the RFC 9207 iss check. The fetcher hands the authorization URL to the scripted
// browser (run/wp15-browser.ts). Build out of the repository: see results/m0-wp15.md. Prints one JSON line.
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net/url"
	"os"
	"os/exec"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/modelcontextprotocol/go-sdk/auth"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var (
	mcpURL = "https://graft-wp15-mcp." + os.Getenv("CF_WORKERS_SUBDOMAIN") + ".workers.dev/"
	cimd   = "https://graft-wp15-aux." + os.Getenv("CF_WORKERS_SUBDOMAIN") + ".workers.dev/cimd/go-v18.json"
)

const (
	redirect = "http://127.0.0.1:4712/callback"
	root     = "/home/user/workspace/repo/tests/m0"
)

func main() {
	calls := 30
	if len(os.Args) > 1 {
		calls, _ = strconv.Atoi(os.Args[1])
	}
	out := map[string]any{"client": "go-v18", "sdk": "github.com/modelcontextprotocol/go-sdk v1.8.0"}
	var iss string
	h, err := auth.NewAuthorizationCodeHandler(&auth.AuthorizationCodeHandlerConfig{
		ClientIDMetadataDocumentConfig: &auth.ClientIDMetadataDocumentConfig{URL: cimd},
		RedirectURL:                    redirect,
		RequestRefreshToken:            true,
		ScopeFilter:                    func([]string) []string { return []string{"write:write"} },
		AuthorizationCodeFetcher: func(ctx context.Context, a *auth.AuthorizationArgs) (*auth.AuthorizationResult, error) {
			cmd := exec.CommandContext(ctx, "node", "run/wp15-browser.ts", a.URL)
			cmd.Dir = root
			b, err := cmd.Output()
			if err != nil {
				return nil, fmt.Errorf("browser: %v", err)
			}
			u, err := url.Parse(strings.TrimSpace(string(b)))
			if err != nil {
				return nil, err
			}
			q := u.Query()
			iss = q.Get("iss")
			return &auth.AuthorizationResult{Code: q.Get("code"), State: q.Get("state"), Iss: q.Get("iss")}, nil
		},
	})
	if err != nil {
		panic(err)
	}
	ctx := context.Background()
	t0 := time.Now()
	c := mcp.NewClient(&mcp.Implementation{Name: "go-v18", Version: "1.0.0"}, nil)
	cs, err := c.Connect(ctx, &mcp.StreamableClientTransport{Endpoint: mcpURL, OAuthHandler: h}, &mcp.ClientSessionOptions{ProtocolVersion: "2026-07-28"})
	if err != nil {
		out["connectError"] = err.Error()
		b, _ := json.Marshal(out)
		fmt.Println(string(b))
		os.Exit(1)
	}
	out["authMs"] = time.Since(t0).Milliseconds()
	out["protocol"] = cs.InitializeResult().ProtocolVersion
	out["iss"] = iss
	tl, err := cs.ListTools(ctx, nil)
	if err == nil {
		names := []string{}
		for _, t := range tl.Tools {
			names = append(names, t.Name)
		}
		out["tools"] = names
	}
	ok, lat, errs := 0, []int64{}, []string{}
	for i := 0; i < calls; i++ {
		var p mcp.CallToolParams
		switch i % 3 {
		case 0:
			p = mcp.CallToolParams{Name: "get_me", Arguments: map[string]any{}}
		case 1:
			p = mcp.CallToolParams{Name: "get_ref", Arguments: map[string]any{"owner": "acme", "repo": "mono", "ref": "heads/main"}}
		default:
			p = mcp.CallToolParams{Name: "get_issue", Arguments: map[string]any{"owner": "acme", "repo": "mono", "number": 1}}
		}
		s := time.Now()
		r, err := cs.CallTool(ctx, &p)
		lat = append(lat, time.Since(s).Milliseconds())
		if err == nil && !r.IsError {
			ok++
		} else if err != nil {
			errs = append(errs, err.Error())
		} else {
			b, _ := json.Marshal(r.Content)
			errs = append(errs, string(b))
		}
	}
	sort.Slice(lat, func(i, j int) bool { return lat[i] < lat[j] })
	out["calls"], out["ok"], out["clientP50"] = calls, ok, lat[len(lat)/2]
	if len(errs) > 3 {
		errs = errs[:3]
	}
	out["errors"] = errs
	cs.Close()
	b, _ := json.Marshal(out)
	fmt.Println(string(b))
}
