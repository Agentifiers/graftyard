# The 2.39-era client for tests/m1/wp6/clients.sh: Debian bookworm's git (2.39.5), unmodified.
# docker build -t graft-git239 -f tests/m1/wp6/git239.Dockerfile tests/m1/wp6
FROM debian:bookworm
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends git ca-certificates curl >/dev/null && rm -rf /var/lib/apt/lists/*
