#!/bin/bash
# BM-1's git conformance suite, part two: git's own HTTP transport tests (t/), unmodified, run against a hosted
# Graftyard edge. Part one is SHARED's oracle rows, tests/m1/wp6/clients.sh.
#
#   tests/m1/wp6/conformance/run.sh https://<edge host> [options]
#
# git's tests start Apache through t/lib-httpd.sh. Nothing in t/ is patched: LIB_HTTPD_PATH points at ./httpd, which
# starts ./front (front/main.go) where Apache would listen. The front mirrors each test's server repository into a
# hosted slot repository and forwards the client's smart-HTTP requests to the edge (see front/main.go for the mapping
# and what it cannot map). report.py turns git's TAP output and the front's event log into per-test results.
#
# Options:
#   --git 2.51.0,2.56.0      upstream versions; each is fetched from kernel.org, checked against its published
#                            SHA-256, and built once under $CONFORMANCE_SRC (default ~/.graft-wp6/src)
#   --tests "t5551 t5541"    test scripts (default: $DEFAULT_TESTS below)
#   --slots bed/main,bed/bf16  hosted repositories the front may overwrite (every ref in them is replaced)
#   --out DIR                results directory (default tests/m1/results/raw/conformance-<UTC time>)
#   --jobs N                 test scripts run at once per version (default 1; each needs its own slots)
#   --fault-cmd CMD          run CMD once, --fault-after seconds into the run, for example the command that fails
#   --fault-after SECONDS    one site's member class; --restore-cmd CMD runs when the tests end. The commands get
#   --restore-cmd CMD        CONFORMANCE_HOST in their environment. M1-WP8 wires these to its fault tool
# Environment: CONFORMANCE_TOKEN (write) and CONFORMANCE_RO_TOKEN (read-only) for the slots' owner, or
#   --tokens FILE with "<token> <hash>" lines as tests/m1/wp6/mktok.py writes them (line 2 writer, line 3 read-only).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=${GRAFT_REPO:-$(cd "$HERE/../../../.." && pwd)}
DEFAULT_TESTS="t5551-http-fetch-smart t5541-http-push-smart t5702-protocol-v2 t5616-partial-clone
  t5539-fetch-http-shallow t5542-push-http-shallow t5549-fetch-push-http t5557-http-get t5581-http-curl-verbose
  t5700-protocol-v1 t5703-upload-pack-ref-in-want t5548-push-porcelain t5601-clone t5550-http-fetch-dumb
  t5812-proto-disable-http t5545-push-options"
declare -A SHA=(
  [2.39.5]=c58da92c378df4a986ca33266897a7397e86c22ee266a284d8c2432c39066b59
  [2.51.0]=60a7c2251cc2e588d5cd87bae567260617c6de0c22dca9cdbfc4c7d2b8990b62
  [2.56.0]=26c56c296b38c0695b26fa95f475f1d01704d2d38e73465ca30b0b2f5dc789d3
)

# git before 2.4x defines its own `unreachable`, which C23 (GCC 15 and later's default) makes a standard macro.
declare -A CSTD=([2.39.5]=-std=gnu17)

HOST=${1:?usage: run.sh https://<edge host> [options]}
shift
VERSIONS=2.51.0,2.56.0 TESTS=$DEFAULT_TESTS SLOTS=bed/main,bed/bf16 JOBS=1
OUT=$ROOT/tests/m1/results/raw/conformance-$(date -u +%Y%m%dT%H%M%SZ)
FAULT_CMD= FAULT_AFTER=0 RESTORE_CMD= TOKENS=
while test $# -gt 0; do
  case "$1" in
  --git) VERSIONS=$2 ;;
  --tests) TESTS=$2 ;;
  --slots) SLOTS=$2 ;;
  --out) OUT=$2 ;;
  --jobs) JOBS=$2 ;;
  --fault-cmd) FAULT_CMD=$2 ;;
  --fault-after) FAULT_AFTER=$2 ;;
  --restore-cmd) RESTORE_CMD=$2 ;;
  --tokens) TOKENS=$2 ;;
  *) echo "unknown option $1" >&2; exit 2 ;;
  esac
  shift 2
done
if test -n "$TOKENS"; then
  CONFORMANCE_TOKEN=$(sed -n 2p "$TOKENS" | cut -d' ' -f1)
  CONFORMANCE_RO_TOKEN=$(sed -n 3p "$TOKENS" | cut -d' ' -f1)
fi
: "${CONFORMANCE_TOKEN:?set CONFORMANCE_TOKEN or --tokens}" "${CONFORMANCE_RO_TOKEN:=}"
SRC=${CONFORMANCE_SRC:-$HOME/.graft-wp6/src}
mkdir -p "$OUT" "$SRC"
OUT=$(cd "$OUT" && pwd)

build_git() {
  local v=$1 d=$SRC/git-$1
  test -x "$d/git" && test -x "$d/t/helper/test-tool" && return 0
  (
    cd "$SRC" &&
      { test -f "git-$v.tar.xz" || curl -fsSLo "git-$v.tar.xz" "https://mirrors.edge.kernel.org/pub/software/scm/git/git-$v.tar.xz"; } &&
      echo "${SHA[$v]}  git-$v.tar.xz" | sha256sum -c - &&
      { test -d "git-$v" || tar -xJf "git-$v.tar.xz"; } &&
      make -C "git-$v" -j"$(nproc)" CFLAGS="-O2 ${CSTD[$v]:-}" NO_TCLTK=1 NO_GETTEXT=1 NO_PYTHON=1 all >"$OUT/build-$v.log" 2>&1
  )
}

(cd "$HERE/front" && CGO_ENABLED=0 go build -o "$OUT/front" .) || exit 1
mkdir -p "$OUT/empty-modules"
for v in ${VERSIONS//,/ }; do
  test -n "${SHA[$v]:-}" || { echo "no checksum for git $v" >&2; exit 2; }
  build_git "$v" || { echo "building git $v failed: $OUT/build-$v.log" >&2; exit 1; }
done

fault_pid=
if test -n "$FAULT_CMD"; then
  (sleep "$FAULT_AFTER" && echo "fault at $(date -u +%FT%TZ)" >>"$OUT/fault.log" &&
    CONFORMANCE_HOST=$HOST bash -c "$FAULT_CMD" >>"$OUT/fault.log" 2>&1) &
  fault_pid=$!
fi

# One test script, with git $1, against slots $3. Each script's trash directory and output live in its own copy of
# t/ results, so two versions can run at once.
run_one() {
  local v=$1 t=$2 slots=$3 d=$SRC/git-$1/t port
  port=$((5500 + RANDOM % 400))
  (
    cd "$d" &&
      rm -rf "trash directory.$t" "test-results/$t.out" &&
      env GIT_TEST_HTTPD=true LIB_HTTPD_PATH="$HERE/httpd" LIB_HTTPD_MODULE_PATH="$OUT/empty-modules" \
        LIB_HTTPD_PORT=$port CONFORMANCE_FRONT="$OUT/front" CONFORMANCE_HOST="$HOST" \
        CONFORMANCE_TOKEN="$CONFORMANCE_TOKEN" CONFORMANCE_RO_TOKEN="$CONFORMANCE_RO_TOKEN" \
        CONFORMANCE_SLOTS="$slots" CONFORMANCE_EVENTS="$OUT/events.tsv" CONFORMANCE_GIT_VERSION="$v" \
        timeout 3600 sh "./$t.sh" --verbose-log >/dev/null 2>&1
    cp "test-results/$t.out" "$OUT/$v-$t.out" 2>/dev/null
  )
  echo "$(date -u +%TZ) git $v $t: $(grep -cE '^ok ' "$OUT/$v-$t.out" 2>/dev/null) ok," \
    "$(grep -cE '^not ok ' "$OUT/$v-$t.out" 2>/dev/null) not ok" | tee -a "$OUT/progress.log"
}

IFS=, read -ra slot_list <<<"$SLOTS"
per=$(( ${#slot_list[@]} / JOBS ))
test "$per" -ge 1 || { echo "--jobs $JOBS needs at least $JOBS slots" >&2; exit 2; }
for v in ${VERSIONS//,/ }; do
  i=0 pids=()
  for t in $TESTS; do
    # Concurrent scripts never share a slot: job j gets slots j*per .. j*per+per-1.
    s=$(IFS=,; echo "${slot_list[*]:$(( (i % JOBS) * per )):$per}")
    run_one "$v" "$t" "$s" &
    pids+=($!)
    i=$((i + 1))
    if test $((i % JOBS)) = 0; then wait "${pids[@]}"; pids=(); fi
  done
  test ${#pids[@]} -gt 0 && wait "${pids[@]}"
done

test -n "$fault_pid" && kill "$fault_pid" 2>/dev/null
test -n "$RESTORE_CMD" && CONFORMANCE_HOST=$HOST bash -c "$RESTORE_CMD" >>"$OUT/fault.log" 2>&1
python3 "$HERE/report.py" "$OUT" >"$OUT/report.md"
echo "results: $OUT/report.md"
