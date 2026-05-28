#!/usr/bin/env bash
# 00-prereqs.sh — fail fast if your environment isn't ready.
#
# Checks:
#   1. `k256-replay` is on PATH
#   2. `solana-keygen` is on PATH (used by recipes to mint fresh keys)
#   3. `jq` is on PATH (used by recipes to parse JSON)
#   4. `REPLAY_API_KEY` is set
#   5. the gateway answers `/status` with HTTP 200
#
# Run this once before any other recipe. Exits 0 when ready, non-zero
# with a clear message when something is missing.

set -euo pipefail

cyan() { printf '\033[36m%s\033[0m\n' "$*"; }
red()  { printf '\033[31m%s\033[0m\n' "$*"; }
green(){ printf '\033[32m%s\033[0m\n' "$*"; }

fail=0

need() {
  local cmd="$1" hint="$2"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    red "✗ missing: $cmd"
    echo "    $hint"
    fail=1
  else
    green "✓ $cmd: $(command -v "$cmd")"
  fi
}

cyan "checking local tools…"
need k256-replay   "install with: cargo install --git https://github.com/k256-xyz/replay-cli --locked"
need solana-keygen "install the Solana CLI from https://solana.com/developers/guides/getstarted/setup-local-development#install-the-solana-cli"
need solana        "ships with the Solana CLI — same installer as solana-keygen"
need jq            "brew install jq  /  apt install jq"
need curl          "comes standard on macOS and Linux"

cyan ""
cyan "checking REPLAY_API_KEY…"
if [[ -z "${REPLAY_API_KEY:-}" ]]; then
  red "✗ REPLAY_API_KEY is unset"
  echo "    export REPLAY_API_KEY=\"rpl_live_…\"   # grab from Access page"
  fail=1
else
  green "✓ REPLAY_API_KEY is set (${REPLAY_API_KEY:0:12}…${REPLAY_API_KEY: -4})"
fi

if [[ $fail -ne 0 ]]; then
  exit 1
fi

cyan ""
cyan "checking gateway via the CLI…"
endpoint="${REPLAY_ENDPOINT:-https://api-replay.k256.xyz}"

# We deliberately use the CLI here (NOT curl). The whole point of these
# recipes is to exercise the CLI's plumbing; if `k256-replay status`
# can reach the gateway with your bearer, every other recipe will too.
# The CLI's exit code carries the auth/network/server classification
# (see README → Exit codes), which is what we branch on below.
#
# We capture exit code into `$cli_status` before the `case` because
# `set -e` would otherwise abort the script on a non-zero CLI exit
# (which is exactly the case we want to handle here).
status_json=$(k256-replay status --json 2>/tmp/replay-status.err) || cli_status=$?
cli_status=${cli_status:-0}
case "$cli_status" in
  0)
    phase=$(jq -r .phase   <<<"$status_json")
    slot=$(jq -r .current_slot <<<"$status_json")
    green "✓ gateway: ${endpoint}  (phase=${phase}, slot=${slot})"
    # Surface host-disk pressure so the operator sees a yellow / red
    # band BEFORE `checkpoint save` or `/boot` hit ENOSPC. Older
    # orchestrator binaries don't include the `disk` field; in that
    # case we stay silent rather than guess.
    pct=$(jq -r '.disk.pct_used // empty'    <<<"$status_json")
    refuse=$(jq -r '.disk.refuse_new_checkpoint // false' <<<"$status_json")
    warn=$(jq -r '.disk.warn // false'      <<<"$status_json")
    if [[ "$refuse" == "true" ]]; then
      free_b=$(jq -r '.disk.free_bytes // 0'   <<<"$status_json")
      total_b=$(jq -r '.disk.total_bytes // 0' <<<"$status_json")
      red "✗ host disk pressure: ${pct}% used (free $((free_b / 1024 / 1024 / 1024)) GB of $((total_b / 1024 / 1024 / 1024)) GB)"
      red "    \`k256-replay checkpoint save\` will be refused until you prune. Try:"
      red "    k256-replay checkpoint prune --keep-latest 1 --yes"
    elif [[ "$warn" == "true" ]]; then
      printf '\033[33m! host disk pressure: %s%% used — consider pruning checkpoints with\033[0m\n' "$pct"
      printf '\033[33m    k256-replay checkpoint prune --keep-latest 3 --yes\033[0m\n'
    fi
    ;;
  2)
    red "✗ gateway rejected your REPLAY_API_KEY (HTTP 401)"
    echo "    Did you copy a key from the wrong workspace? Grab it from the Access page."
    exit 1
    ;;
  3)
    red "✗ gateway forbade the request (HTTP 403)"
    echo "    Your bearer is valid but doesn't own a box. Provision one from the dashboard."
    exit 1
    ;;
  7)
    red "✗ orchestrator returned 5xx — the fork is unhealthy right now"
    cat /tmp/replay-status.err
    exit 1
    ;;
  *)
    red "✗ k256-replay status failed (exit ${cli_status})"
    cat /tmp/replay-status.err
    exit 1
    ;;
esac
rm -f /tmp/replay-status.err

green ""
green "all checks passed. Run any of the other recipes next."
