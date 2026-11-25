# LM-3's kill row (`build/BUILD-PLAN.md`, AM3-WP1, "a Sandbox killed mid-task resumes"): the scripted harness's image.
# Alpine with git, curl and python3, as M0-WP14's Sandbox image (`tests/m0/wp14/Dockerfile`); no credential is baked in,
# the outbound router attaches the task's tokens. `harness.py` is the entrypoint `HARNESS` names.
FROM alpine:3.22
RUN apk add --no-cache git curl ca-certificates python3
COPY harness.py /harness.py
WORKDIR /work
CMD ["python3", "/harness.py"]
