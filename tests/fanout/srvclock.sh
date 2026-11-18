# Sourced by the row drivers (rows.sh, costrow.sh, b034.sh): stamps on Cloudflare's clock, not the orb's
# (tests/fanout/README.md, "Clocks"). Measures the orb's offset once per driver, unless SRV_OFFSET_MS is already set
# (a parent driver's measurement), and exports it so the node tracers it starts can log it too.
#   stamp   -> 2026-11-18T11:17:42Z on the server's clock
if [ -z "${SRV_OFFSET_MS:-}" ]; then
  SRV_OFFSET_MS=$(node tests/fanout/srvclock.mjs 2>/dev/null | jq -r '.offsetMs // empty' 2>/dev/null)
  # Unmeasured, the stamps stay on the orb's clock, and every status line says so.
  SRV_OFFSET_MS=${SRV_OFFSET_MS:-unmeasured}
fi
export SRV_OFFSET_MS
stamp() {
  case $SRV_OFFSET_MS in
    unmeasured) echo "$(date -u +%FT%TZ) (orb clock)" ;;
    *) date -u -d "@$(( ($(date +%s%3N) + SRV_OFFSET_MS) / 1000 ))" +%FT%TZ ;;
  esac
}
