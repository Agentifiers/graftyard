#!/bin/bash
# The hosted vault row (tests/m5/results/m5-wp5-vault.md, "Hosted vault row"): `graftyard vault init` into a vault
# account, passes against graft-m3f's real feed under the real 35-day bucket locks, the lock's 409 on a create-only
# put and on a delete, a pass rerun from an older state/vault, `vault upgrade`, then the operator's teardown order.
# The RUNNER account stands in for the vault account on this row (labelled so in every log): its token is not one of
# the install's tokens, which is the property under test.
#
#   bash tests/m5/vaulthosted/row.sh <tag> <phase>
#
# Phases, in order: init; check (repeat until caught up; the first saves state/vault for the rerun); locks; rerun;
# check; upgrade; teardown; lists. The export token `vault init` mints lives 1 day and exists only as the vault
# script's secret, so deleting the script at teardown removes the only copy.
#
# Needs: CLOUDFLARE_RUNNER_ACCOUNT_ID and CLOUDFLARE_RUNNER_API_TOKEN (the stand-in vault account), the graft-m3f env
# (tests/am1/lf13/env-m3f.sh, for GRAFT_EDGE), REL (the signed release folder) and G (the graftyard binary). Every
# log goes through scrub, which replaces the account ids, the zone id and the workers.dev subdomains; tokens are never
# printed.
set -uo pipefail
cd "$(dirname "$0")/../../.."
TAG=${1:?tag}; PH=${2:?phase}
O=tests/m5/results/raw/vault/$TAG; mkdir -p "$O"
S=graft-m3f-vault-standin                   # script and bucket, in the runner account (stand-in vault)
W=$HOME/.graft-m3f/vault; mkdir -p "$W"; chmod 700 "$W"
G=${G:?the graftyard binary}; REL=${REL:?the release folder}
export CLOUDFLARE_VAULT_API_TOKEN=$CLOUDFLARE_RUNNER_API_TOKEN
VA=$CLOUDFLARE_RUNNER_ACCOUNT_ID
r2() { R2_ACCOUNT=$VA R2_TOKEN_VAR=CLOUDFLARE_RUNNER_API_TOKEN python3 tests/m5/vaulthosted/r2.py $S "$@"; }
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_RUNNER_API_TOKEN" "https://api.cloudflare.com/client/v4/accounts/$VA$1" "${@:2}"; }
scrub() {
  sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CLOUDFLARE_RUNNER_ACCOUNT_ID/<runner-account>/g" \
      -e "s/$CF_WORKERS_SUBDOMAIN/<install-subdomain>/g" -e "s/$CF_RUNNER_WORKERS_SUBDOMAIN/<runner-subdomain>/g" \
      -e "s/$CF_ZONE_ID/<zone-id>/g" -e "s#$PWD/#<checkout>/#g" -e "s#$HOME/#~/#g"
}
N=$(ls "$O" | grep -c "^$PH" || true)
L="$O/$PH$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; echo "vault account: the runner account, standing in for a vault account"; cat; } | scrub >"$L"; cat "$L"; }

case $PH in
init)  # The release verifies under graft-m3f's Keys.release, then bucket, lock rules, script, secret, cron.
  { git rev-parse --short HEAD; echo "release $(sha256sum "$REL/release.json" | cut -c1-64)"
    "$G" --config "$HOME/.graft-m3f/install.toml" vault init --account "$VA" --key "$HOME/.graft-m3f/customer.key" \
      --release "$REL" --release-key "$HOME/.graft-m3f/release.pub" --bucket $S --script $S --edge "$GRAFT_EDGE" --days 1
    echo "init exit $?"
    r2 lock-get
    api "/workers/scripts/$S/schedules" | jq -c '{schedules: [.result.schedules[]?.cron]}'
    api "/workers/scripts/$S/secrets" | jq -c '{secrets: [.result[]? | "\(.name) \(.type)"]}'
  } 2>&1 | log ;;
check)  # What the vault holds against the install's export bucket.
  { r2 snap "$O/vault-keys.json"
    r2 get state/vault "$O/state.json"
    [ -s "$W/state-old" ] || { cp "$O/state.json" "$W/state-old"; echo "saved this state/vault for the rerun"; }
    R2_ACCOUNT=$CLOUDFLARE_ACCOUNT_ID R2_TOKEN_VAR=CLOUDFLARE_API_TOKEN python3 tests/m5/vaulthosted/r2.py \
      graft-m3f-export snap "$O/export-keys.json"
    python3 tests/m5/vaulthosted/compare.py "$O"; echo "compare exit $?"
  } 2>&1 | log ;;
locks)  # The 409 half under the real lock: a create-only put and a plain put over a held feed key, a delete of it and
  # of a held pack, then the unlocked state/ prefix taking a put and a delete.
  { k=$(jq -r 'keys[] | select(startswith("feed/"))' "$O/vault-keys.json" | head -1)
    p=$(jq -r 'keys[] | select(startswith("pack/"))' "$O/vault-keys.json" | head -1)
    echo "feed key $k; pack key ${p:-none}"
    r2 get "$k" "$W/before"
    printf 'not the object' >"$W/other"
    r2 put "$k" "$W/other" --create-only
    r2 put "$k" "$W/other"
    r2 delete "$k"
    [ -n "$p" ] && r2 delete "$p"
    r2 get "$k" "$W/after"
    cmp -s "$W/before" "$W/after" && echo "held: the feed object is unchanged" || echo "CHANGED: the feed object"
    printf 'x' >"$W/x"; r2 put state/lockprobe "$W/x"; r2 delete state/lockprobe
    rm -f "$W/before" "$W/after" "$W/other" "$W/x"
  } 2>&1 | log ;;
rerun)  # A pass from an older state/vault: put back the saved one, wait for two cron invocations, then compare.
  { [ -s "$W/state-old" ] || { echo "no saved state/vault"; exit 1; }
    python3 -c "import json,sys; s=json.load(open(sys.argv[1])); p=s['feed']['last']; print('older state: position', p and f\"{p['minute']:012}/{p['seq']:010}\", 'chains', len(s['feed']['cursors']))" "$W/state-old"
    r2 snap "$O/before-rerun.json"
    r2 put state/vault "$W/state-old"
    echo "state/vault set back to the older one; waiting 130 s for two cron invocations"
    sleep 130
    r2 snap "$O/after-rerun.json"
    r2 get state/vault "$O/state-after-rerun.json"
    python3 - "$O/before-rerun.json" "$O/after-rerun.json" "$O/state-after-rerun.json" <<'PY'
import json, sys
a, b, st = (json.load(open(p)) for p in sys.argv[1:4])
locked = lambda d: {k: v for k, v in d.items() if not k.startswith("state/")}
la, lb = locked(a), locked(b)
changed = [k for k in la if k in lb and la[k] != lb[k]]
gone = [k for k in la if k not in lb]
new = sorted(set(lb) - set(la))
p = st["feed"]["last"]
print(json.dumps({"locked_before": len(la), "locked_after": len(lb), "changed": changed, "gone": gone, "new": len(new),
                  "new_prefixes": sorted({k.split("/")[0] for k in new}), "position_after": p and f"{p['minute']:012}/{p['seq']:010}",
                  "missed": st["missed"], "undecoded": st["undecoded"]}))
PY
  } 2>&1 | log ;;
upgrade)  # The same verified release again: left alone; a tampered copy: refused before any call.
  { "$G" vault upgrade --account "$VA" --release "$REL" --release-key "$HOME/.graft-m3f/release.pub" --script $S
    echo "upgrade exit $?"
    rm -rf "$W/bad"; cp -r "$REL" "$W/bad"; printf 'x' >>"$W/bad/vault/entry.js"
    "$G" vault upgrade --account "$VA" --release "$W/bad" --release-key "$HOME/.graft-m3f/release.pub" --script $S
    echo "tampered upgrade exit $?"; rm -rf "$W/bad"
  } 2>&1 | log ;;
teardown)  # The operator's order: lock rules to none, GET none, empty, delete bucket, delete the secret, the script.
  { r2 lock-clear; r2 lock-get; r2 empty
    r2 bucket-delete
    api "/workers/scripts/$S/secrets/EXPORT_TOKEN" -X DELETE | jq -c '{secret_delete: .success, errors: [.errors[]?.code]}'
    api "/workers/scripts/$S" -X DELETE | jq -c '{script_delete: .success, errors: [.errors[]?.code]}'
    rm -f "$W/state-old"
  } 2>&1 | log ;;
lists)  # Every one is gone.
  { api "/r2/buckets" | jq -c '{buckets: [.result.buckets[]?.name]}'
    api "/workers/scripts" | jq -c '{scripts: [.result[]?.id]}'
    r2 lock-get
    api "/workers/scripts/$S/secrets" | jq -c '{secrets_of_script: .success, errors: [.errors[]?.code]}'
  } 2>&1 | log ;;
*) echo "phase: init check locks rerun upgrade teardown lists"; exit 2 ;;
esac
