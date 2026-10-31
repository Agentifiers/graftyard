# M0-WP14 (L0-02): the Sandbox image the controller runs the containment harness in. Alpine with the tools an escape
# attempt would reach for: git (clone/push), curl (HTTP(S), redirects, CONNECT proxy, alt ports, IP literals), and
# python3 (raw sockets, DNS). No credential is baked in; the outbound router adds the git credential outside the VM
# (research/agents.md 6.7). Built on deploy, like b0-22/Dockerfile and wp16/Dockerfile.
FROM alpine:3.22
RUN apk add --no-cache git curl ca-certificates python3 bind-tools
RUN mkdir -p /work
WORKDIR /work
CMD ["sleep", "infinity"]
