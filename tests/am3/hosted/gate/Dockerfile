# LM-3 row A's L0-02 Sandbox image: M0-WP14's tools (`tests/m0/wp14/Dockerfile`: git, curl, python3) and the adapted
# harness. No credential is baked in; `/gate.json` is the row's config (`gate-row.sh image`), with no secret in it.
FROM alpine:3.22
RUN apk add --no-cache git curl ca-certificates python3 bind-tools
COPY attempts.py /attempts.py
COPY gate.json /gate.json
WORKDIR /work
CMD ["python3", "/attempts.py", "/gate.json"]
