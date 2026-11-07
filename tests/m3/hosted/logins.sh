#!/usr/bin/env bash
# Every login the edge renders on the seeded repositories' issue, pull, comment, and review lists, by account id, so a
# run with `LOGINS` set and one with it empty can be compared (F50's hosted check). Reads as the owner, whose token
# reaches curl on stdin. Prints one line per (repo, surface, id, login) and a summary.
#
#   tests/m3/hosted/logins.sh <edge url> <owner token file> <repo> ...
set -euo pipefail
edge=${1:?edge}; tok=${2:?token file}; shift 2
get() { curl -s -m 60 -H @- "$edge/api/v3$1" <<<"authorization: token $(cat "$tok")"; }
for r in "$@"; do
  for s in "issues?state=all&per_page=100" "pulls?state=all&per_page=100" "issues/comments?per_page=100"; do
    get "/repos/wp6/$r/$s" | jq -r --arg r "$r" --arg s "${s%%\?*}" '
      if type == "array" then .[] | [.user, .assignee, (.assignees // [])[], (.requested_reviewers // [])[], .merged_by]
        | .[] | select(. != null) | "\($r) \($s) \(.id) \(.login)" else "\($r) \($s) error \(.message // .)" end'
  done
done | sort | uniq -c
# GraphQL: pull request, review, and comment authors, which REST does not list here.
for r in "$@"; do
  q=$(jq -cn --arg r "$r" '{query: "query($r:String!){repository(owner:\"wp6\",name:$r){pullRequests(first:50){nodes{author{login} reviews(first:20){nodes{author{login}}} comments(first:20){nodes{author{login}}}}} issues(first:50){nodes{author{login} comments(first:20){nodes{author{login}}}}}}}", variables: {r: $r}}')
  curl -s -m 60 -H @- -d "$q" "$edge/api/graphql" <<<"authorization: token $(cat "$tok")" |
    jq -r --arg r "$r" 'if .errors then "\($r) graphql error \(.errors[0].message)" else [.. | objects | select(has("login")) | .login] | .[] | "\($r) graphql - \(.)" end'
done | sort | uniq -c
