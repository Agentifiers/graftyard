// L0-08 hosted and A43 with the official Go SDK client (github.com/modelcontextprotocol/go-sdk v1.8.0), unmodified,
// at protocol 2026-07-28, against graft-mcp. auth.AuthorizationCodeHandler does RFC 9728 discovery, RFC 8414, CIMD,
// PKCE S256, RFC 8707 resource, and the RFC 9207 iss check. The fetcher hands the authorization URL to the scripted
// browser (tests/am2/hosted/browser.mjs). `-drop-token` (A43): the first code redemption's answer is dropped by the
// HTTP client after graft-mcp answered it. Prints one JSON line.
//
//	cd tests/am2/hosted/goclient && go run . [-calls 30] [-drop-token]
package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/modelcontextprotocol/go-sdk/auth"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

type call struct {
	Grant  string `json:"grant"`
	Status int    `json:"status"`
}

// dropper records every /token call and, once, drops a code redemption's answer after reading it.
type dropper struct {
	mu      sync.Mutex
	drop    bool
	dropped bool
	calls   []call
	token   string
}

func (d *dropper) RoundTrip(r *http.Request) (*http.Response, error) {
	grant := ""
	if r.URL.String() == d.token && r.Body != nil {
		b, _ := io.ReadAll(r.Body)
		r.Body = io.NopCloser(strings.NewReader(string(b)))
		q, _ := url.ParseQuery(string(b))
		grant = q.Get("grant_type")
	}
	resp, err := http.DefaultTransport.RoundTrip(r)
	if err != nil || r.URL.String() != d.token {
		return resp, err
	}
	d.mu.Lock()
	defer d.mu.Unlock()
	d.calls = append(d.calls, call{grant, resp.StatusCode})
	if d.drop && !d.dropped && grant == "authorization_code" {
		d.dropped = true
		io.Copy(io.Discard, resp.Body)
		resp.Body.Close()
		return nil, errors.New("A43: the /token answer dropped after graft-mcp answered")
	}
	return resp, nil
}

func main() {
	calls := flag.Int("calls", 30, "tools/list calls")
	drop := flag.Bool("drop-token", false, "A43: drop the first code redemption's answer")
	flag.Parse()
	sub := os.Getenv("CF_WORKERS_SUBDOMAIN")
	mcpOrigin := os.Getenv("GRAFT_MCP")
	if mcpOrigin == "" {
		mcpOrigin = "https://graft-m3f-mcp." + sub + ".workers.dev"
	}
	aux := os.Getenv("GRAFT_AUX")
	if aux == "" {
		aux = "https://graft-m3f-mcp-aux." + sub + ".workers.dev"
	}
	root, _ := os.Getwd()
	browser := root + "/../browser.mjs"
	d := &dropper{drop: *drop, token: mcpOrigin + "/token"}
	out := map[string]any{"client": "go-v18", "sdk": "github.com/modelcontextprotocol/go-sdk v1.8.0", "drop": *drop}
	var iss string
	consents := 0
	h, err := auth.NewAuthorizationCodeHandler(&auth.AuthorizationCodeHandlerConfig{
		ClientIDMetadataDocumentConfig: &auth.ClientIDMetadataDocumentConfig{URL: aux + "/cimd/go-v18.json"},
		RedirectURL:                    "http://127.0.0.1:4712/callback",
		RequestRefreshToken:            true,
		ScopeFilter:                    func([]string) []string { return []string{"write:write"} },
		Client:                         &http.Client{Transport: d, Timeout: 60 * time.Second},
		AuthorizationCodeFetcher: func(ctx context.Context, a *auth.AuthorizationArgs) (*auth.AuthorizationResult, error) {
			consents++
			cmd := exec.CommandContext(ctx, "node", browser, a.URL)
			cmd.Stderr = os.Stderr
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
	var cs *mcp.ClientSession
	// A client that does not retry the code itself gets an error here; a second connect starts again from the 401.
	for attempt := 0; attempt < 3; attempt++ {
		cs, err = c.Connect(ctx, &mcp.StreamableClientTransport{Endpoint: mcpOrigin + "/", OAuthHandler: h}, &mcp.ClientSessionOptions{ProtocolVersion: "2026-07-28"})
		if err == nil {
			break
		}
		out[fmt.Sprintf("connectError%d", attempt)] = err.Error()
	}
	if err == nil {
		out["authMs"] = time.Since(t0).Milliseconds()
		out["protocol"] = cs.InitializeResult().ProtocolVersion
		out["iss"] = iss
		ok, lat, errs := 0, []int64{}, []string{}
		for i := 0; i < *calls; i++ {
			s := time.Now()
			_, err := cs.ListTools(ctx, nil)
			lat = append(lat, time.Since(s).Milliseconds())
			if err == nil {
				ok++
			} else if len(errs) < 3 {
				errs = append(errs, err.Error())
			}
		}
		sort.Slice(lat, func(i, j int) bool { return lat[i] < lat[j] })
		out["calls"], out["ok"], out["errors"] = *calls, ok, errs
		if len(lat) > 0 {
			out["clientP50"] = lat[len(lat)/2]
		}
		cs.Close()
	}
	redemptions := 0
	for _, x := range d.calls {
		if x.Grant == "authorization_code" {
			redemptions++
		}
	}
	out["consents"], out["tokenCalls"], out["codeRedemptions"] = consents, d.calls, redemptions
	b, _ := json.Marshal(out)
	fmt.Println(string(b))
}
