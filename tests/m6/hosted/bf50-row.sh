#!/usr/bin/env bash
# BF-50 on graft-m6a (`build/BUILD-PLAN.md`, M6-WP1; `tests/m6/results/m6-wp1.md`, "BF-50"): one phase of a row per
# call, each writing its raw files under tests/m6/raw/m6a/<tag>/ through `scrub`.
#   bash tests/m6/hosted/bf50-row.sh <tag> <phase>
# Phases, in order:
#   create    test-register chains: 24 Tri homed on A (0), 8 homed on B (1), and 4 Tri on B, C and the spare, the
#             controls that hold no voter on A
#   load      LOAD_S seconds of puts at LOAD_RATE on the register chains, then stop (load.json: the last acked value
#             per chain)
#   before    after the load stops and before the fault: every registry row on sites 0-3 (64 shards each), each
#             chain's committed config, every script's version list, and the Release view's count (the span's start)
#   fault     BM-1's site fault on A for FAULT_MS
#   sitelost  (m6aL) a customer-signed SiteLost{0}
#   restore1  a customer-signed RestorePlacement{0} while A is drained: expect a refusal
#   watch     the Release view now: the entries since `before`, with signers, and the current intent
#   after     once the ReplaceMember is done: every config again, the coordinator and group instances' step names,
#             the version lists, and the judge's line 2, signers, line 3, and campaign counts
#   lift      the site fault off, so A recovers; graft-ops undrains it after OPS_RECOVER_MS
#   restore2  once A is undrained: RestorePlacement{0}, customer-signed
#   home      once the restore is done: every config, the judge's home check, the reads of every acked write
#   teardown  stop every shard the moves warmed, refused while a config names the spare (R67)
#   spend     the install's spend since the row's start, against the $10 row stop
set -uo pipefail
TAG=${1:?tag}; PHASE=${2:?phase}
source "$(dirname "${BASH_SOURCE[0]}")/bf50.sh"
R=$ROOT/tests/m6/raw/m6a/$TAG; mkdir -p "$R"
J="python3 $ROOT/tests/m6/hosted/bf50judge.py $R"
WF=graft-m6a-workflows
log() { echo "{\"at\":\"$(date -u +%FT%TZ)\",\"phase\":\"$PHASE\",$1}" | tee -a "$R/row.log"; }
chains_a() { for i in $(seq 1 24); do printf '"repo:%d",' $((6000 + i)); done; }
chains_b() { for i in $(seq 1 8); do printf '"repo:%d",' $((6100 + i)); done; }
chains_s() { for i in $(seq 1 4); do printf '"repo:%d",' $((6210 + i)); done; }
# Every register chain as {chain, site, do_name} on a live voter (B's member: never the faulted A).
regs() {
  jq -c '[.[] | select(.chain | startswith("repo:")) | select(.site == 1) | {chain, site, do_name}] | unique_by(.chain)' "$R/rows-before.json"
}
# snapshot <name> <sites json> [rows]: registry rows on those sites, then (unless `rows`) each chain's config.
snapshot() {
  local out=$1 sites=$2 all='[]'
  for from in 0 16 32 48; do
    all=$(jq -sc '.[0] + .[1].rows' <(echo "$all") <(bench /snapshot "{\"sites\":$sites,\"from\":$from,\"count\":16}"))
  done
  echo "$all" >"$R/rows-$out.json"
  # Each chain once, through a member on a surviving site (B, else C, else the spare), at its highest generation.
  jq -c '[group_by(.chain)[] | (map(select(.site != 0)) | sort_by(-.generation, .site) | .[0]) // empty
    | {chain, site, do_name}]' "$R/rows-$out.json" >"$R/chains-$out.json"
  [ "${3:-}" = rows ] && return
  local n; n=$(jq length "$R/chains-$out.json"); local cfgs='{}'
  for off in $(seq 0 60 $((n - 1))); do
    cfgs=$(jq -sc '.[0] + .[1]' <(echo "$cfgs") <(bench /configs "{\"chains\":$(jq -c ".[$off:$((off + 60))]" "$R/chains-$out.json")}"))
  done
  # A member at commit 0 holds no config and names no voter to ask next, so an unread chain is asked again through
  # each of its other rows' members, and the highest config index any of them reports is kept.
  echo "$cfgs" >"$R/configs-$out.json"
  local again; again=$(jq -c --slurpfile c "$R/configs-$out.json" \
    '[.[] | select(($c[0][.chain] // {}) | has("error")) | {chain, site, do_name}] | unique' "$R/rows-$out.json")
  if [ "$(echo "$again" | jq length)" != 0 ]; then
    local more='[]'
    for off in $(seq 0 60 $(($(echo "$again" | jq length) - 1))); do
      more=$(jq -sc '.[0] + [.[1] | to_entries[]]' <(echo "$more") \
        <(bench /configs "{\"chains\":$(echo "$again" | jq -c ".[$off:$((off + 60))] | map(. + {chain: (.chain + \"|\" + .do_name)})")}"))
    done
    echo "$more" >"$R/configs-$out-again.json"
    jq -c --slurpfile m "$R/configs-$out-again.json" 'reduce ($m[0][] | select(.value | has("cfg"))) as $x (.;
      ($x.key | split("|")[0]) as $k | if ((.[$k].index // -1) < $x.value.index) then .[$k] = $x.value else . end)' \
      "$R/configs-$out.json" >"$R/configs-$out.tmp" && mv "$R/configs-$out.tmp" "$R/configs-$out.json"
  fi
  log "\"rows\":$(jq length "$R/rows-$out.json"),\"chains\":$n,\"unread\":$(jq '[.[] | select(.error)] | length' "$R/configs-$out.json")"
}
entries() {
  local v; v=$(view "$(cat "$R/view-from")")
  echo "$v" | jq -c '.entries' >"$R/entries.json"
  echo "$v" | jq -c '.keys' >"$R/keys.json"
  echo "$v" | jq -c '{count, current, drained}'
}
instance() { api "/workflows/$1/instances/$2" | jq -c '{result: {status: .result.status, error: .result.error, output: .result.output, steps: [.result.steps[]? | {name, type, start, end, success}]}}'; }
case $PHASE in
  create)
    log "\"a\":$(bench /create "{\"chains\":[$(chains_a | sed 's/,$//')],\"home\":0}" | jq -c '[.[] | select(.warm and .role == "leader")] | length')"
    log "\"b\":$(bench /create "{\"chains\":[$(chains_b | sed 's/,$//')],\"home\":1}" | jq -c '[.[] | select(.warm and .role == "leader")] | length')"
    # The controls: Tri on B, C and the spare, so no voter on A. A Solo group needs an evaluation genesis, which m6a's is not.
    log "\"controls\":$(bench /create "{\"chains\":[$(chains_s | sed 's/,$//')],\"home\":1,\"sites\":[1,2,3]}" | jq -c '[.[] | select(.warm and .role == "leader")] | length')"
    date -u +%FT%TZ >"$R/start" ;;
  load)
    # The register chains' leaders, from their members on B.
    snapshot load "[1]" rows
    body=$(jq -c --argjson s "${LOAD_S:-120}" --argjson r "${LOAD_RATE:-4}" \
      '{chains: [.[] | select(.chain | startswith("repo:"))], seconds: $s, rate: $r, key: "bf50"}' "$R/chains-load.json")
    bench /load "$body" >"$R/load.json"
    log "\"load\":$(jq -c '{ok, bad, chains: (.last | length), errs}' "$R/load.json")" ;;
  before)
    snapshot before "[0,1,2,3]"
    # Every row member of every chain answers `/status` as joined, else the chain is excluded with its reason
    # (m6aL: tokens:953's site 1 member had never joined, so no group could elect without A). A's are read too,
    # since this runs before the fault.
    jq -c '[.[] | {site, do_name}] | unique' "$R/rows-before.json" >"$R/members-before.json"
    n=$(jq length "$R/members-before.json"); j='{}'
    for off in $(seq 0 200 $((n - 1))); do
      j=$(jq -sc '.[0] + .[1]' <(echo "$j") <(bench /joined "{\"members\":$(jq -c ".[$off:$((off + 200))]" "$R/members-before.json")}"))
    done
    echo "$j" >"$R/joined-before.json"
    jq -c --slurpfile j "$R/joined-before.json" '[.[] | select($j[0]["\(.site)|\(.do_name)"] != true)
      | {chain, site, do_name, why: ($j[0]["\(.site)|\(.do_name)"] // "no answer" | if . == false then "not joined" else tostring end)}]
      | group_by(.chain) | map({chain: .[0].chain, why: map("\(.do_name) on site \(.site): \(.why)")})' \
      "$R/rows-before.json" >"$R/unjoined-before.json"
    # And a chain whose rows name one member name on two sites (OPEN-QUESTIONS R65): every row member is joined, but a
    # reader keyed by name reads two groups (m6aL: repo:6201's `#1#f0` was a Tri voter on site 0 and a Solo try on 1).
    jq -c --slurpfile u "$R/unjoined-before.json" '($u[0] | map(.chain)) as $have | $u[0] + [group_by(.chain)[]
      | select(.[0].chain as $c | $have | index($c) | not)
      | [group_by(.do_name)[] | select(map(.site) | unique | length > 1)] | select(length > 0)
      | {chain: .[0][0].chain, why: map("\(.[0].do_name) on sites \(map(.site) | unique | map(tostring) | join(" and "))")}]' \
      "$R/rows-before.json" >"$R/unjoined-before.tmp" && mv "$R/unjoined-before.tmp" "$R/unjoined-before.json"
    log "\"members\":$n,\"unjoined_chains\":$(jq length "$R/unjoined-before.json")"
    versions | scrub >"$R/versions-before.jsonl"
    view 0 | jq -r .count >"$R/view-from"
    log "\"view_from\":$(cat "$R/view-from"),\"held_A\":$(jq '[.[] | select(.cfg) | select(any(.cfg | .. | objects | select(has("do_name")); .site == 0))] | length' "$R/configs-before.json")" ;;
  fault)
    log "\"fault\":$(bench /fault "{\"site\":0,\"for_ms\":${FAULT_MS:-86400000}}" | jq -c '{plan, put}')" ;;
  sitelost)
    log "\"sitelost\":$(customer '{"SiteLost":{"site":0}}' sitelost | jq -Rs .)" ;;
  restore1|restore2)
    log "\"restore\":$(customer '{"Intent":{"ttl_s":86400,"intent":{"RestorePlacement":{"site":0}}}}' "$PHASE" | jq -Rs .)" ;;
  watch)
    log "\"view\":$(entries)"
    jq -c '.[] | {index, kind, sigs, entry: .entry[0:100]}' "$R/entries.json" ;;
  after)
    entries >/dev/null
    id=$(jq -r '[.[] | select(.entry | test("ReplaceMember.* id "))][0].entry | split(" id ")[1]' "$R/entries.json")
    echo "$id" >"$R/replace-id"
    instance "$WF-movecoordinator" "$id" >"$R/coordinator.json"
    : >"$R/groups.jsonl"
    snapshot after "[1,2,3]"
    # One REST read per group, 16 at a time: one after another, 3,118 groups took hours.
    group1() {
      local gid="$1-$(echo "$2" | sed 's/[^a-zA-Z0-9_-]/_/g')"
      api "/workflows/$WF-movegroup/instances/$gid" | jq -c --arg id "$gid" '{id: $id, status: .result.status,
        error: .result.error, output: .result.output, steps: [.result.steps[]? | .name],
        verdicts: [.result.steps[]? | select((.name // "") | startswith("campaign ")) | .output]}'
    }
    export -f group1 api; export WF CFAPI CLOUDFLARE_API_TOKEN
    jq -r '.[].chain' "$R/chains-after.json" | xargs -P 16 -I{} bash -c 'group1 "$0" "$1"' "$id" {} >>"$R/groups.jsonl"
    versions | scrub >"$R/versions-after.jsonl"
    log "\"versions_same\":$(cmp -s "$R/versions-before.jsonl" "$R/versions-after.jsonl" && echo true || echo false)"
    for c in line2 signers line3 campaign; do $J "$c" | tee -a "$R/judge.jsonl"; done ;;
  lift)
    log "\"lift\":$(bench /fault '{"off":true}' | jq -c '.put')" ;;
  home)
    entries >/dev/null
    snapshot home "[0,1,2,3]"
    # Configs read while groups idle come back empty or stale (m6aL: 60 unread, 72 short); reread after the reads
    # below wake each home, and judge on the reread.
    body=$(jq -c '{chains: [.[] | select(.chain | startswith("repo:"))], key: "bf50"}' "$R/chains-home.json")
    bench /read "$body" >"$R/read-home.json"
    snapshot home "[0,1,2,3]"
    versions | scrub >"$R/versions-home.jsonl"
    log "\"versions_same\":$(cmp -s "$R/versions-before.jsonl" "$R/versions-home.jsonl" && echo true || echo false)"
    for c in home signers writes; do $J "$c" | tee -a "$R/judge.jsonl"; done ;;
  teardown)
    # After `home`: stop every shard the moves warmed (OPEN-QUESTIONS R67), and read the container list before and
    # after.
    log "$(bash "$ROOT/tests/m6/hosted/bf50-teardown.sh" "$R" | tail -n 1 | sed 's/^{//; s/}$//')" ;;
  spend)
    log "\"spend\":$(spend "$(cat "$R/start")")" ;;
  *) echo "unknown phase $PHASE" >&2; exit 2 ;;
esac
