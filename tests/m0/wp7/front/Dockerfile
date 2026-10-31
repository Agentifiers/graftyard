# M0-WP7 (B0-24): the SSH front image. OpenSSH stands in for `graft-sshd`, git-shell serves upload-pack and
# receive-pack for one seeded repository, and cloudflared runs one Tunnel replica. The host key comes from `keys.graft`,
# authorized keys from `tokens.graft`, and every push is committed through `group.graft`; all three are virtual hosts
# of the container's outbound handler (src/wp7/ssh.ts), so no write leaves through the public zone.
FROM alpine:3.22
ARG CLOUDFLARED=2026.10.7
RUN apk add --no-cache openssh-server git git-daemon curl ca-certificates \
 && wget -qO /usr/local/bin/cloudflared https://github.com/cloudflare/cloudflared/releases/download/${CLOUDFLARED}/cloudflared-linux-amd64 \
 && chmod 755 /usr/local/bin/cloudflared \
 && adduser -D -h /srv/git -s /bin/sh git && sed -i 's/^git:!/git:*/' /etc/shadow
COPY sshd_config /etc/ssh/sshd_config
COPY akc gitwrap entry /usr/local/bin/
COPY pre-receive /srv/pre-receive
# The seed commit has fixed dates, so every replica serves the same object ids.
RUN chmod 755 /usr/local/bin/akc /usr/local/bin/gitwrap /usr/local/bin/entry /srv/pre-receive \
 && mkdir -p /srv/git/acme && git init -q --bare -b main /srv/git/acme/app.git \
 && git clone -q /srv/git/acme/app.git /tmp/seed && cd /tmp/seed \
 && for i in $(seq 1 40); do seq 1 300 | sed "s/^/line $i /" > f$i.txt; done \
 && git add -A && GIT_AUTHOR_DATE='2026-10-30T00:00:00Z' GIT_COMMITTER_DATE='2026-10-30T00:00:00Z' \
    git -c user.name=seed -c user.email=seed@m0 commit -qm seed && git push -q origin main \
 && cd / && rm -rf /tmp/seed && cp /srv/pre-receive /srv/git/acme/app.git/hooks/pre-receive \
 && chown -R git:git /srv/git
CMD ["/usr/local/bin/entry"]
