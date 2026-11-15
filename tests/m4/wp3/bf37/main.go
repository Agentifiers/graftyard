// BF-37's go-playground/webhooks check: every delivery the sink kept (`sink/`, `/dump`) is sent through
// go-playground/webhooks v6.4.0's unmodified `github.Parse`, with the hook's secret, as the request the edge made.
//
//   - `pull_request` deliveries parse into `PullRequestPayload`; `enqueued` and `dequeued` must carry the number.
//   - v6.4.0 has no `merge_group` type: `Parse` checks the HMAC first, then answers "unknown event merge_group". That
//     answer counts as the library's signature check passing (a bad signature answers ErrHMACVerificationFailed), and
//     the body is then decoded into MergeGroupPayload, GitHub's documented shape, which must name a queue ref.
//
// Prints one JSON line per delivery and a summary; exits 1 if any delivery fails.
//
//	go run . <sink /dump JSON file> <secret>
package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"strings"

	"github.com/go-playground/webhooks/v6/github"
)

type delivery struct {
	At    int64  `json:"at"`
	Hook  string `json:"hook"`
	Event string `json:"event"`
	GUID  string `json:"guid"`
	Sig   string `json:"sig"`
	Body  string `json:"body"`
}

// MergeGroupPayload is GitHub's `merge_group` webhook payload (docs.github.com, "Webhook events and payloads",
// merge_group), the fields BF-37 reads.
type MergeGroupPayload struct {
	Action     string `json:"action"`
	Reason     string `json:"reason"`
	MergeGroup struct {
		HeadSHA string `json:"head_sha"`
		HeadRef string `json:"head_ref"`
		BaseSHA string `json:"base_sha"`
		BaseRef string `json:"base_ref"`
	} `json:"merge_group"`
	Repository struct {
		FullName string `json:"full_name"`
	} `json:"repository"`
}

type result struct {
	GUID    string `json:"guid"`
	Event   string `json:"event"`
	Action  string `json:"action"`
	Detail  string `json:"detail"`
	OK      bool   `json:"ok"`
	Problem string `json:"problem,omitempty"`
}

func main() {
	raw, err := os.ReadFile(os.Args[1])
	if err != nil {
		panic(err)
	}
	var ds []delivery
	if err := json.Unmarshal(raw, &ds); err != nil {
		panic(err)
	}
	hook, _ := github.New(github.Options.Secret(os.Args[2]))
	failed := 0
	counts := map[string]int{}
	for _, d := range ds {
		r := check(hook, d)
		if !r.OK {
			failed++
		}
		counts[r.Event+"."+r.Action]++
		line, _ := json.Marshal(r)
		fmt.Println(string(line))
	}
	sum, _ := json.Marshal(map[string]any{"deliveries": len(ds), "failed": failed, "byEventAction": counts})
	fmt.Println(string(sum))
	if failed > 0 {
		os.Exit(1)
	}
}

func check(hook *github.Webhook, d delivery) result {
	req, _ := http.NewRequest(http.MethodPost, "/", bytes.NewReader([]byte(d.Body)))
	req.Header.Set("X-GitHub-Event", d.Event)
	req.Header.Set("X-GitHub-Delivery", d.GUID)
	req.Header.Set("X-Hub-Signature-256", d.Sig)
	r := result{GUID: d.GUID, Event: d.Event}
	fail := func(f string, a ...any) result { r.Problem = fmt.Sprintf(f, a...); return r }
	p, err := hook.Parse(req, github.PullRequestEvent, github.Event("merge_group"), github.PingEvent)
	switch d.Event {
	case "pull_request":
		if err != nil {
			return fail("Parse: %v", err)
		}
		pr, ok := p.(github.PullRequestPayload)
		if !ok {
			return fail("Parse gave %T", p)
		}
		r.Action, r.Detail = pr.Action, fmt.Sprintf("#%d", pr.Number)
		if pr.Number == 0 || pr.PullRequest.Number != pr.Number {
			return fail("no pull request number")
		}
	case "merge_group":
		if err == nil || err.Error() != "unknown event merge_group" {
			return fail("Parse: %v (want the library's signature check, then unknown event)", err)
		}
		var m MergeGroupPayload
		dec := json.NewDecoder(strings.NewReader(d.Body))
		if err := dec.Decode(&m); err != nil {
			return fail("decode: %v", err)
		}
		r.Action, r.Detail = m.Action, m.MergeGroup.HeadRef
		if !strings.HasPrefix(m.MergeGroup.HeadRef, "refs/heads/gh-readonly-queue/") || len(m.MergeGroup.HeadSHA) != 40 {
			return fail("merge_group without a queue ref and head: %+v", m.MergeGroup)
		}
		if m.Action == "destroyed" && m.Reason == "" {
			return fail("destroyed without a reason")
		}
	case "ping":
		if err != nil {
			return fail("Parse: %v", err)
		}
		r.Action = "ping"
	default:
		return fail("unexpected event")
	}
	r.OK = true
	return r
}
