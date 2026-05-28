#!/usr/bin/env bash
# 07-workbench-loop.sh — introduction to the workbench iteration.
#
# What this script DOES:
#   - read cache stats from /fixtures/stats
#   - find a captured signature from the newest captured slot
#     (falling back to slot-1 / slot-2 if the newest isn't yet
#     visible at the fork's processed commitment)
#   - print the state-diff table for that fixture
#   - print copy-pasteable follow-ups (checkpoint, patch, advance,
#     diff again, restore) with the captured sig pre-filled
#
# What this script does NOT do:
#   - it does not save a checkpoint, patch any account, advance the
#     fork, or restore. Those are the follow-ups you run by hand
#     after reading the diff.
#   - it does not re-execute the captured transaction under a
#     mutation. `k256-replay diff <sig>` always returns the fixture
#     that was captured when the tx first landed; for "same tx under
#     patched state" you need to splice the tx into a future slot
#     (see /advance's `splices` array) and diff the new signature.
#
# Run this against a freshly-booted fork (newest_slot from /fixtures
# stats should be within a few hundred slots of when the fork was
# booted, so /fixtures still has populated captures from the boot's
# advance).

set -euo pipefail
cd "$(dirname "$0")/.."

cyan() { printf '\033[36m%s\033[0m\n' "$*"; }
dim()  { printf '\033[2m%s\033[0m\n' "$*"; }
red()  { printf '\033[31m%s\033[0m\n' "$*"; }

cyan "1. snapshotting the fork's current state…"
status=$(k256-replay status --json)
phase=$(jq -r .phase <<<"$status")
slot=$(jq -r .current_slot <<<"$status")
fixtures=$(jq -r .advance.id <<<"$status")
dim "   phase: ${phase}, slot: ${slot}"
if [[ "$phase" != "ready" ]]; then
  red "✗ fork is not ready — try again in a few seconds"
  exit 1
fi
echo

cyan "2. dumping the latest fixture cache stats…"
echo "$ k256-replay fixtures stats --json | jq ."
stats=$(k256-replay fixtures stats --json)
echo "$stats" | jq .
echo

n_fixtures=$(jq -r .fixture_count <<<"$stats")
if [[ "$n_fixtures" -lt 1 ]]; then
  red "✗ no fixtures in cache yet — advance the fork first:"
  red "    k256-replay advance start --to \$(( ${slot} + 1 )) --wait"
  exit 1
fi
dim "   cache holds ${n_fixtures} fixtures across $(jq -r .slot_count <<<"$stats") slots"
echo

cyan "3. picking a captured signature from the most recent captured slot…"
newest_slot=$(jq -r .newest_slot <<<"$stats")
dim "   newest captured slot: ${newest_slot}"

if [[ -z "${FORK_RPC:-}" ]]; then
  cat <<EOF
\$FORK_RPC is not set — the recipe can't query the fork to find a
captured signature for you. Set it (Solana RPC URL from your server's
Access page) and re-run, or do it by hand:

  # Get the slot's signatures via standard Solana JSON-RPC:
  curl -sS \$FORK_RPC -X POST -H 'content-type: application/json' \\
    -d '{"jsonrpc":"2.0","id":1,"method":"getBlock","params":[${newest_slot},{"transactionDetails":"signatures","maxSupportedTransactionVersion":0,"rewards":false}]}' \\
    | jq -r '.result.signatures[]?' \\
    | while read sig; do
        if k256-replay diff "\$sig" --json >/dev/null 2>&1; then
          echo "captured: \$sig"; break
        fi
      done
EOF
  exit 0
fi

# Sample sigs from a captured slot and probe each with `k256-replay
# diff` until we get a hit. Sample every 25th to walk the whole block
# fast (~50 probes covers a typical 1200-tx slot).
#
# Edge case: the fixtures cache can include a slot the fork RPC
# doesn't yet expose via `getBlock` (the validator captured the slot
# but commitment hasn't caught up to make the block queryable yet).
# Scan up to SCAN_DEPTH slots back from `newest_slot` — covers a
# fresh /advance where the most-recent slots are still in transit
# AND the more common case where the cache holds a handful of older
# captured slots that have all moved to a queryable commitment.
SCAN_DEPTH=10
captured=""
probed_slot=""
total_probed=0
slots_probed=0
for offset in $(seq 0 "$SCAN_DEPTH"); do
  slot_to_try=$((newest_slot - offset))
  slots_probed=$((slots_probed + 1))
  echo "$ curl ... getBlock(${slot_to_try}, transactionDetails=signatures)"
  sigs_json=$(curl -sS --max-time 10 "$FORK_RPC" -X POST -H 'content-type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getBlock\",\"params\":[${slot_to_try},{\"transactionDetails\":\"signatures\",\"commitment\":\"confirmed\",\"maxSupportedTransactionVersion\":0,\"rewards\":false}]}" 2>/dev/null)
  total_sigs=$(jq -r '.result.signatures | length // 0' <<<"$sigs_json" 2>/dev/null || echo 0)
  if [[ "$total_sigs" == "0" || "$total_sigs" == "null" ]]; then
    dim "   slot ${slot_to_try}: not yet visible at RPC's confirmed commitment; trying earlier slot"
    continue
  fi
  dim "   slot ${slot_to_try}: ${total_sigs} signatures total"
  candidates=$(jq -r '.result.signatures[]?' <<<"$sigs_json" | awk 'NR % 25 == 1' | head -60)
  for sig in $candidates; do
    total_probed=$((total_probed + 1))
    if k256-replay diff "$sig" --json >/dev/null 2>&1; then
      captured="$sig"
      probed_slot="$slot_to_try"
      break 2
    fi
  done
  dim "   slot ${slot_to_try}: no hit in $(echo "$candidates" | wc -l | tr -d ' ') probes"
done

if [[ -z "$captured" ]]; then
  red "✗ probed ${total_probed} signatures across ${slots_probed} slots, no fixture hit."
  red "  the cache may have just been cleared or the box rotated past these slots."
  red "  try: k256-replay fixtures stats   (shows what slots still have captures)"
  exit 1
fi
dim "   probed ${total_probed} sigs across captured slots, hit: ${captured} (slot ${probed_slot})"
echo

cyan "4. inspecting the state diff (the killer command)…"
echo "$ k256-replay diff $captured"
echo
k256-replay diff "$captured" | head -30
echo
dim "   (use \`--json\` for the wire shape, \`--bytes\` for inline base64)"
echo

cyan "5. recommended follow-ups:"
cat <<EOF
You now have a real captured signature ($captured) to A/B against.

  # Save a checkpoint BEFORE you mutate so you can rewind any time:
  k256-replay checkpoint save --label baseline

  # Pick an account the transaction touched (from the diff above) and
  # mutate it:
  k256-replay patch apply --pubkey <ACCT> --mode merge --lamports 999999999 --yes

  # Drive forward and look at how the protocol behaves under the patch:
  k256-replay advance start --to \$(( ${slot} + 1 )) --wait
  k256-replay diff $captured

  # When you're done, rewind:
  k256-replay checkpoint restore <ckpt-id>
EOF
