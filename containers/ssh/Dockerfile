# The SSH front image, `sshfront` (design/DESIGN.md, Stack, Containers; Shape, "SSH front"): `graft-sshd` alone, a
# static Go binary on golang.org/x/crypto/ssh. No OpenSSH, no git, no cloudflared, and no secret: the host keys come
# from `keys.graft` at start and every git request goes to the edge through `git.graft`, both outbound intercepts of
# the site's `SshFront` object. Built by `docker build containers/ssh` and pushed digest-pinned for the site deploy.
FROM golang:1.26-alpine AS server
WORKDIR /src
COPY server/go.mod server/go.sum ./
RUN go mod download
COPY server/*.go ./
RUN rm -f *_test.go && CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /graft-sshd .

FROM scratch
COPY --from=server /graft-sshd /graft-sshd
EXPOSE 22
ENTRYPOINT ["/graft-sshd"]
