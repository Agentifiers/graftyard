#!/usr/bin/env bash
# Lists every git shard container instance of each site script in sites.json, through the Containers API: site script,
# instance name (git#<site>#net:<repo>), state, the last 12 characters of its image digest, and when it started.
# Usage: instances.sh <sites.json>
set -u
API=https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID/containers/applications
# The API token reaches curl through `-H @<(printf ...)`, never argv, which `ps` shows.
h() { printf 'Authorization: Bearer %s\n' "$CLOUDFLARE_API_TOKEN"; }
apps=$(curl -sf -H @<(h) "$API")
for s in $(python3 -c 'import json,sys; print(" ".join(x["script"] for x in json.load(open(sys.argv[1]))))' "$1"); do
  id=$(echo "$apps" | python3 -c "import json,sys; print(next((a['id'] for a in json.load(sys.stdin)['result'] if a['name'] == '$s-gitshard'), ''))")
  [ -n "$id" ] || continue
  curl -sf -H @<(h) "$API/$id/instances?per_page=1000" | python3 -c "
import json, sys
for i in sorted(json.load(sys.stdin)['result']['instances'], key=lambda i: i['name']):
    print('$s', i['name'], i['status']['state'], i['image'][-12:], i['started_at'])"
done
