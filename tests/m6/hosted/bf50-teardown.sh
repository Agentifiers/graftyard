#!/usr/bin/env bash
# BF-50's teardown (`bf50-row.sh <tag> teardown`): once `home` has committed, stop every git shard container still
# running on graft-m6a's sites and spare. A move warms the destination's network shards with a client catch-up, which
# keeps each warm 24 h (OPEN-QUESTIONS R67): after m6aL 25 ran on the spare and 25 on the restored site a with nothing
# reading them, about $5.4 an hour together. Each goes through the shard's own `/teardown`, from a scratch Worker bound
# to every site script's `GitShard` class (the product edge has no `/_bed/kill`; tests/fanout/shardkill), deployed with
# a one-time token in a 0600 secrets file and deleted at the end. A shard starts again on the next client request.
# Refuses to run while any committed config still names the spare (configs-home.json, from `home`).
set -uo pipefail
R=${1:?raw folder}
source "$(dirname "${BASH_SOURCE[0]}")/bf50.sh"
if jq -e '[.[] | .cfg // {} | .. | objects | select(has("do_name")) | .site] | index(3) != null' "$R/configs-home.json" >/dev/null; then
  echo '{"teardown":"refused: a config still names the spare"}'; exit 1
fi
W=$(mktemp -d); trap 'rm -rf "$W"; "$WRANGLER" delete graft-m6a-bf50-shardkill --force >/dev/null 2>&1' EXIT
jq -n --arg main "$ROOT/tests/fanout/shardkill/index.js" '{name: "graft-m6a-bf50-shardkill", main: $main,
  compatibility_date: "2025-09-01", workers_dev: true, durable_objects: {bindings: [
  {name: "SHARD_0", class_name: "GitShard", script_name: "graft-m6a-site-a"},
  {name: "SHARD_1", class_name: "GitShard", script_name: "graft-m6a-site-b"},
  {name: "SHARD_2", class_name: "GitShard", script_name: "graft-m6a-site-c"},
  {name: "SHARD_3", class_name: "GitShard", script_name: "graft-m6a-spare-wnam"}]}}' >"$W/wrangler.json"
python3 -c "import secrets,os,json,sys;t=secrets.token_hex(24);d=sys.argv[1]
for n,b in (('secrets.json',json.dumps({'TOKEN':t})),('tok',t)):
  fd=os.open(f'{d}/{n}',os.O_WRONLY|os.O_CREAT,0o600);os.write(fd,b.encode());os.close(fd)" "$W"
"$WRANGLER" deploy -c "$W/wrangler.json" --secrets-file "$W/secrets.json" >"$W/deploy.log" 2>&1 || { echo '{"teardown":"deploy failed"}'; exit 1; }
BASE="https://graft-m6a-bf50-shardkill.$CF_WORKERS_SUBDOMAIN.workers.dev"
before=$(api /containers/applications | jq -c '[.result[]? | select(.name|test("^graft-m6a-(site-[abc]|spare-wnam)-gitshard$")) | {(.name): .health.instances.active}] | add')
for app in $(api /containers/applications | jq -r '.result[]? | select(.name|test("^graft-m6a-(site-[abc]|spare-wnam)-gitshard$")) | .id'); do
  for n in $(api "/containers/applications/$app/instances?per_page=1000" | jq -r '.result.instances[] | select(.status.state=="running") | .name'); do
    site=${n#git#}; site=${site%%#*}
    for t in 1 2 3 4 5 6; do
      c=$(curl -s -m 60 -X POST -H @- -o /dev/null -w '%{http_code}' "$BASE/?site=$site&name=$(jq -rn --arg n "$n" '$n|@uri')" <<<"x-t: $(cat "$W/tok")")
      [ "$c" = 200 ] && break; sleep 5
    done
    echo "{\"shard\":\"$n\",\"code\":\"$c\"}" >>"$R/teardown.jsonl"
  done
done
sleep 30
after=$(api /containers/applications | jq -c '[.result[]? | select(.name|test("^graft-m6a-(site-[abc]|spare-wnam)-gitshard$")) | {(.name): .health.instances.active}] | add')
echo "{\"teardown\":{\"before\":$before,\"after\":$after,\"codes\":$(jq -s 'group_by(.code) | map({(.[0].code): length}) | add // {}' "$R/teardown.jsonl")}}"
