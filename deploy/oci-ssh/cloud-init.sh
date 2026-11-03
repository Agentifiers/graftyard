#!/bin/bash
# The git SSH proxy: public port 22 forwarded into a Cloudflare Tunnel `ssh` hostname, one edge stream per connection.
# Runs as the VM's cloud-init user-data, and again by hand to converge an existing VM:
#   ssh -p 2222 ubuntu@<ip> sudo bash -s < deploy/oci-ssh/cloud-init.sh
# Every step rewrites its whole file or checks before it changes state, so a rerun or a crashed run converges.
set -euo pipefail

TUNNEL_HOSTNAME=${TUNNEL_HOSTNAME:-ssh-m0.graftyard.dev}
ADMIN_PORT=${ADMIN_PORT:-2222}
export DEBIAN_FRONTEND=noninteractive

install -d -m 0755 /usr/share/keyrings
curl -fsSL https://pkg.cloudflare.com/cloudflare-public-v2.gpg -o /usr/share/keyrings/cloudflare-public-v2.gpg
echo 'deb [signed-by=/usr/share/keyrings/cloudflare-public-v2.gpg] https://pkg.cloudflare.com/cloudflared any main' \
  > /etc/apt/sources.list.d/cloudflared.list
apt-get update -q
apt-get install -y -q cloudflared unattended-upgrades

cat > /etc/apt/apt.conf.d/20auto-upgrades <<'EOF'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
EOF
# Ubuntu's security pocket is on by default; cloudflared is the only internet-facing package this VM adds.
cat > /etc/apt/apt.conf.d/52graft-ssh-proxy <<'EOF'
Unattended-Upgrade::Origins-Pattern { "origin=cloudflared,codename=any"; };
Unattended-Upgrade::Automatic-Reboot "true";
Unattended-Upgrade::Automatic-Reboot-Time "10:00";
EOF

cat > /etc/ssh/sshd_config.d/10-graft-admin.conf <<EOF
Port $ADMIN_PORT
PasswordAuthentication no
KbdInteractiveAuthentication no
AuthenticationMethods publickey
PermitRootLogin no
EOF
# On first boot sshd has not run yet, and `sshd -t` refuses to test without its privilege separation directory.
install -d -m 0755 /run/sshd
sshd -t
# Ubuntu 24.04 starts sshd from ssh.socket on port 22, which the forwarder needs. Run sshd as a plain service instead.
# ssh.service has KillMode=process, so the socket-started listener can outlive the switch; it is killed by port.
if systemctl is-enabled -q ssh.socket 2>/dev/null || systemctl is-active -q ssh.socket; then
  systemctl stop ssh.service ssh.socket
  systemctl disable ssh.socket
  systemctl daemon-reload
fi
for pid in $(ss -Htlnp 'sport = :22' | grep -o 'sshd",pid=[0-9]*' | cut -d= -f2 | sort -u); do kill "$pid"; done
systemctl enable ssh.service
systemctl restart ssh.service

# Oracle's Ubuntu images ship an iptables INPUT chain that rejects every port but 22.
if iptables -S INPUT | grep -q -- '-j REJECT'; then
  if ! iptables -C INPUT -p tcp -m state --state NEW -m tcp --dport "$ADMIN_PORT" -j ACCEPT 2>/dev/null; then
    n=$(iptables -L INPUT --line-numbers | awk '$2 == "REJECT" { print $1; exit }')
    iptables -I INPUT "$n" -p tcp -m state --state NEW -m tcp --dport "$ADMIN_PORT" -j ACCEPT
  fi
  if command -v netfilter-persistent >/dev/null; then netfilter-persistent save; fi
fi

echo "TUNNEL_HOSTNAME=$TUNNEL_HOSTNAME" > /etc/default/graft-ssh-proxy
cat > /etc/systemd/system/graft-ssh-proxy.service <<'EOF'
[Unit]
Description=git SSH on port 22, forwarded into the Cloudflare Tunnel hostname in /etc/default/graft-ssh-proxy
After=network-online.target ssh.service
Wants=network-online.target
StartLimitIntervalSec=0

[Service]
EnvironmentFile=/etc/default/graft-ssh-proxy
ExecStart=/usr/bin/cloudflared access tcp --hostname ${TUNNEL_HOSTNAME} --url 0.0.0.0:22 --loglevel info
DynamicUser=yes
StateDirectory=graft-ssh-proxy
Environment=HOME=/var/lib/graft-ssh-proxy
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
Restart=always
RestartSec=1

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable graft-ssh-proxy.service
systemctl restart graft-ssh-proxy.service
for _ in $(seq 30); do
  if ss -Htlnp 'sport = :22' | grep -q cloudflared; then exit 0; fi
  sleep 1
done
echo "graft-ssh-proxy is not listening on port 22" >&2
exit 1
