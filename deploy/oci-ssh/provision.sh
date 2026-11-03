#!/bin/bash
# Creates or finds the Always Free git SSH proxy in the tenancy's home region, then its DNS-only record.
#   deploy/oci-ssh/provision.sh <admin public key file>
# Needs: the OCI CLI configured (~/.oci/config), jq, CLOUDFLARE_API_TOKEN. Every resource is looked up by display name
# first and created only if absent, so a rerun, or a run after a crash, converges on one copy of each.
# Prints the resource ids as JSON on stdout.
set -euo pipefail
export PYTHONWARNINGS=ignore SUPPRESS_LABEL_WARNING=True

PUBKEY=${1:?admin public key file}
HERE=$(cd "$(dirname "$0")" && pwd)
NAME=graft-ssh-proxy
SHAPE=VM.Standard.E2.1.Micro
BOOT_GB=50
ADMIN_PORT=2222
ZONE=<cf-zone-id>
RECORD=ssh.graftyard.dev
TAGS='{"project":"graftyard","purpose":"ssh-proxy"}'

T=$(awk -F= '$1 == "tenancy" { print $2 }' ~/.oci/config)
C=${COMPARTMENT:-$T}
oci() { command oci "$@" 2>/dev/null; }
first() { jq -r '.data[0].id // empty'; }
log() { echo "$*" >&2; }

# Always Free compute exists only in the home region, and the Micro shape only in one availability domain.
HOME_REGION=$(oci iam region-subscription list | jq -r '.data[] | select(."is-home-region") | ."region-name"')
[ "$HOME_REGION" = "$(awk -F= '$1 == "region" { print $2 }' ~/.oci/config)" ] || { log "config region is not the home region $HOME_REGION"; exit 1; }
AD=$(for ad in $(oci iam availability-domain list --compartment-id "$T" | jq -r '.data[].name'); do
  oci compute shape list --compartment-id "$C" --availability-domain "$ad" --all |
    jq -r --arg s "$SHAPE" --arg ad "$ad" '.data[] | select(.shape == $s and ."billing-type" == "ALWAYS_FREE") | $ad'
done | head -1)
[ -n "$AD" ] || { log "no availability domain offers $SHAPE as ALWAYS_FREE"; exit 1; }

VCN=$(oci network vcn list -c "$C" --display-name "$NAME-vcn" --lifecycle-state AVAILABLE | first)
if [ -z "$VCN" ]; then
  log "creating VCN"
  VCN=$(oci network vcn create -c "$C" --display-name "$NAME-vcn" --cidr-blocks '["10.0.0.0/16"]' --dns-label graftssh \
    --freeform-tags "$TAGS" --wait-for-state AVAILABLE | jq -r .data.id)
fi
VCN_JSON=$(oci network vcn get --vcn-id "$VCN")
RT=$(jq -r '.data."default-route-table-id"' <<<"$VCN_JSON")
SL=$(jq -r '.data."default-security-list-id"' <<<"$VCN_JSON")

IGW=$(oci network internet-gateway list -c "$C" --vcn-id "$VCN" --display-name "$NAME-igw" --lifecycle-state AVAILABLE | first)
if [ -z "$IGW" ]; then
  log "creating internet gateway"
  IGW=$(oci network internet-gateway create -c "$C" --vcn-id "$VCN" --display-name "$NAME-igw" --is-enabled true \
    --freeform-tags "$TAGS" --wait-for-state AVAILABLE | jq -r .data.id)
fi
oci network route-table update --rt-id "$RT" --force --wait-for-state AVAILABLE \
  --route-rules "[{\"destination\":\"0.0.0.0/0\",\"destinationType\":\"CIDR_BLOCK\",\"networkEntityId\":\"$IGW\"}]" >/dev/null

# The VCN's default security list, rewritten whole: git on 22 and admin SSH from anywhere, path-MTU ICMP, all egress.
tcp() { echo "{\"protocol\":\"6\",\"source\":\"0.0.0.0/0\",\"isStateless\":false,\"tcpOptions\":{\"destinationPortRange\":{\"min\":$1,\"max\":$1}}}"; }
oci network security-list update --security-list-id "$SL" --force --wait-for-state AVAILABLE \
  --ingress-security-rules "[$(tcp 22),$(tcp $ADMIN_PORT),{\"protocol\":\"1\",\"source\":\"0.0.0.0/0\",\"isStateless\":false,\"icmpOptions\":{\"type\":3,\"code\":4}}]" \
  --egress-security-rules '[{"protocol":"all","destination":"0.0.0.0/0","isStateless":false}]' >/dev/null

SUBNET=$(oci network subnet list -c "$C" --vcn-id "$VCN" --display-name "$NAME-subnet" --lifecycle-state AVAILABLE | first)
if [ -z "$SUBNET" ]; then
  log "creating subnet"
  SUBNET=$(oci network subnet create -c "$C" --vcn-id "$VCN" --display-name "$NAME-subnet" --cidr-block 10.0.0.0/24 \
    --dns-label proxy --prohibit-public-ip-on-vnic false --freeform-tags "$TAGS" --wait-for-state AVAILABLE | jq -r .data.id)
fi

INSTANCE=$(oci compute instance list -c "$C" --display-name "$NAME" --all |
  jq -r '[.data[] | select(."lifecycle-state" != "TERMINATED" and ."lifecycle-state" != "TERMINATING")][0].id // empty')
if [ -z "$INSTANCE" ]; then
  IMAGE=$(oci compute image list -c "$C" --operating-system "Canonical Ubuntu" --operating-system-version 24.04 \
    --shape "$SHAPE" --sort-by TIMECREATED --sort-order DESC --all |
    jq -r '[.data[] | select(."display-name" | test("Minimal") | not)][0].id')
  log "launching $SHAPE in $AD"
  INSTANCE=$(oci compute instance launch -c "$C" --availability-domain "$AD" --display-name "$NAME" --shape "$SHAPE" \
    --image-id "$IMAGE" --subnet-id "$SUBNET" --assign-public-ip true --boot-volume-size-in-gbs "$BOOT_GB" \
    --ssh-authorized-keys-file "$PUBKEY" --user-data-file "$HERE/cloud-init.sh" --freeform-tags "$TAGS" \
    --wait-for-state RUNNING --max-wait-seconds 900 | jq -r .data.id)
fi
VNIC=$(oci compute vnic-attachment list -c "$C" --instance-id "$INSTANCE" | jq -r '.data[0]."vnic-id"')
IP=$(oci network vnic get --vnic-id "$VNIC" | jq -r '.data."public-ip"')
BOOT=$(oci compute boot-volume-attachment list -c "$C" --availability-domain "$AD" --instance-id "$INSTANCE" | jq -r '.data[0]."boot-volume-id"')

# DNS only (grey cloud): Cloudflare's proxy does not carry SSH on a Pro zone.
CF=https://api.cloudflare.com/client/v4/zones/$ZONE/dns_records
AUTH="authorization: Bearer $CLOUDFLARE_API_TOKEN"
REC=$(curl -fsS -H "$AUTH" "$CF?name=$RECORD")
REC_ID=$(jq -r '.result[0].id // empty' <<<"$REC")
BODY=$(jq -nc --arg n "$RECORD" --arg ip "$IP" '{type:"A",name:$n,content:$ip,proxied:false,ttl:300,comment:"graft-ssh-proxy (deploy/oci-ssh)"}')
if [ -z "$REC_ID" ]; then
  REC_ID=$(curl -fsS -X POST -H "$AUTH" -H 'content-type: application/json' "$CF" -d "$BODY" | jq -r .result.id)
elif [ "$(jq -r '.result[0].comment' <<<"$REC")" = "graft-ssh-proxy (deploy/oci-ssh)" ]; then
  curl -fsS -X PUT -H "$AUTH" -H 'content-type: application/json' "$CF/$REC_ID" -d "$BODY" >/dev/null
else
  log "$RECORD exists and was not made by this script; left unchanged"; exit 1
fi

jq -n --arg region "$HOME_REGION" --arg ad "$AD" --arg vcn "$VCN" --arg igw "$IGW" --arg rt "$RT" --arg sl "$SL" \
  --arg subnet "$SUBNET" --arg instance "$INSTANCE" --arg boot "$BOOT" --arg vnic "$VNIC" --arg ip "$IP" \
  --arg port "$ADMIN_PORT" --arg dns "$REC_ID" --arg record "$RECORD" \
  '{region:$region, ad:$ad, vcn:$vcn, internet_gateway:$igw, route_table:$rt, security_list:$sl, subnet:$subnet,
    instance:$instance, boot_volume:$boot, vnic:$vnic, public_ip:$ip, admin_port:($port|tonumber),
    dns_record:{name:$record, id:$dns}}'
