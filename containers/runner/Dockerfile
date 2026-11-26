# The hosted runner image (design/SHARED.md, "Actions", Hosted runners; build/BUILD-PLAN.md, M4-WP4): an unmodified
# `actions/runner` at the version the install pins (tests/m4/wp3/runner-protocol.md), checked against the release's
# published SHA-256, on the Sandbox SDK's base image. `RunnerBox` (workers/runners/box.mjs) starts it with an
# entrypoint that execs `run.sh --jitconfig` as the container's main process, one job per container, so the base
# image's `/container-server/sandbox` never runs [measured: R23's M1 check, PID 1 is run.sh]. Release M3 (`r23-m3`)
# deleted the Sandbox SDK classes that used that server; the base stays only because it is the image every hosted run
# so far measured, and moving off it is a new runner image with its own canary. Built and pushed by
# tests/m4/wp4/image.sh.
ARG SANDBOX_VERSION=0.12.14
FROM docker.io/cloudflare/sandbox:${SANDBOX_VERSION}

ARG RUNNER_VERSION=2.338.2
ARG RUNNER_SHA256=baebf204307bac6193480852732f0aba2ae97b6c5597c244f4d5558390147c40
WORKDIR /home/runner
RUN curl -fsSLo runner.tar.gz \
      "https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/actions-runner-linux-x64-${RUNNER_VERSION}.tar.gz" \
 && echo "${RUNNER_SHA256}  runner.tar.gz" | sha256sum -c - \
 && tar -xzf runner.tar.gz && rm runner.tar.gz \
 && ./bin/installdependencies.sh \
 && rm -rf /var/lib/apt/lists/*
# The Sandbox's processes run as root; the runner refuses root without this.
ENV RUNNER_ALLOW_RUNASROOT=1
