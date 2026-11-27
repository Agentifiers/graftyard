# LM-3's kill row, real-harness case (FRAME, "Operator decision, 2026-11-26, a model key for LM-3's real-harness kill
# case"): the stock Claude Code CLI as the Sandbox's harness, against the operator's token proxy, which serves Anthropic's
# Messages API (FRAME, main 70aa964b). Its HTTPS goes through the outbound router like any other request, and the route's
# model arm names the proxy's host (`taskbox.json`, `route_others`). No credential is baked in: `ANTHROPIC_API_KEY` is a dummy,
# and the outbound router drops the Sandbox's `x-api-key`, `anthropic-version` and `Authorization` on the model arm and
# sets the model key itself (`graftyard_git::outbound`, `Cred::ModelKey`). `run.py` is the entrypoint `HARNESS` names.
# The scripted harness's image (`../Dockerfile`) is unchanged.
FROM node:22-alpine
RUN apk add --no-cache git curl ca-certificates python3
RUN npm install -g @anthropic-ai/claude-code@2.1.332 && npm cache clean --force
RUN adduser -D agent && mkdir /work && chown agent /work
COPY run.py /run.py
USER agent
WORKDIR /work
ENV ANTHROPIC_API_KEY=sk-ant-dummy-the-router-overwrites-it \
    DISABLE_AUTOUPDATER=1 DISABLE_TELEMETRY=1 DISABLE_ERROR_REPORTING=1 \
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 NODE_EXTRA_CA_CERTS=/etc/cloudflare/certs/cloudflare-containers-ca.crt \
    ANTHROPIC_BASE_URL=https://token-proxy.workinprogress.dev \
    ANTHROPIC_MODEL=claude-sonnet-5-5 ANTHROPIC_SMALL_FAST_MODEL=claude-sonnet-5-5
CMD ["python3", "/run.py"]
