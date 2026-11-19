# The hosted runner image (design/SHARED.md, "Actions", Hosted runners; build/BUILD-PLAN.md, M4-WP4): the Sandbox SDK's
# base image, whose `/container-server/sandbox` serves the SDK's calls, plus an unmodified `actions/runner` at the
# version the install pins (tests/m4/wp3/runner-protocol.md), checked against the release's published SHA-256.
# `graft-runners` starts `run.sh --jitconfig` in it, one job per container (workers/runners/src/worker.rs): on the legacy
# Sandbox classes through the base image's server, and on `RunnerBox` (workers/runners/box.mjs) as the container's main
# process, whose entrypoint override bypasses that server [unverified until release M1's check]. Built and pushed by
# wrangler from workers/runners/deploy.mjs's config.
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
