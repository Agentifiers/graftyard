#!/bin/bash
# Deletes what provision.sh made, found by the same display names: the instance and its boot volume, the subnet, the
# internet gateway, the VCN (with its default route table and security list), and the ssh.graftyard.dev record if
# provision.sh's comment is on it. Safe to rerun: anything already gone is skipped.
set -euo pipefail
export PYTHONWARNINGS=ignore SUPPRESS_LABEL_WARNING=True
NAME=graft-ssh-proxy
ZONE=${CF_ZONE_ID:?}
RECORD=ssh.graftyard.dev
T=$(awk -F= '$1 == "tenancy" { print $2 }' ~/.oci/config)
C=${COMPARTMENT:-$T}
oci() { command oci "$@" 2>/dev/null; }

for id in $(oci compute instance list -c "$C" --display-name "$NAME" --all |
    jq -r '.data[] | select(."lifecycle-state" != "TERMINATED") | .id'); do
  oci compute instance terminate --instance-id "$id" --preserve-boot-volume false --force \
    --wait-for-state TERMINATED --max-wait-seconds 900 >/dev/null
  echo "terminated $id"
done
VCN=$(oci network vcn list -c "$C" --display-name "$NAME-vcn" | jq -r '.data[0].id // empty')
if [ -n "$VCN" ]; then
  for id in $(oci network subnet list -c "$C" --vcn-id "$VCN" | jq -r '.data[].id'); do
    oci network subnet delete --subnet-id "$id" --force --wait-for-state TERMINATED >/dev/null; echo "deleted $id"
  done
  RT=$(oci network vcn get --vcn-id "$VCN" | jq -r '.data."default-route-table-id"')
  oci network route-table update --rt-id "$RT" --route-rules '[]' --force >/dev/null
  for id in $(oci network internet-gateway list -c "$C" --vcn-id "$VCN" | jq -r '.data[].id'); do
    oci network internet-gateway delete --ig-id "$id" --force --wait-for-state TERMINATED >/dev/null; echo "deleted $id"
  done
  oci network vcn delete --vcn-id "$VCN" --force --wait-for-state TERMINATED >/dev/null; echo "deleted $VCN"
fi
CF=https://api.cloudflare.com/client/v4/zones/$ZONE/dns_records
AUTH="authorization: Bearer $CLOUDFLARE_API_TOKEN"
for id in $(curl -fsS -H "$AUTH" "$CF?name=$RECORD" | jq -r '.result[] | select(.comment == "graft-ssh-proxy (deploy/oci-ssh)") | .id'); do
  curl -fsS -X DELETE -H "$AUTH" "$CF/$id" >/dev/null; echo "deleted DNS record $id"
done
