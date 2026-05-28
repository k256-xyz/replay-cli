#!/usr/bin/env bash
# 03-batch-patch.sh — atomically apply three patches in one request.
#
# Why: simulating a market state often means putting several accounts in
# a specific configuration at the same instant. Either all three land
# together or none of them do — that's what the orchestrator's
# `PatchRequestBody.patches` array guarantees.

set -euo pipefail
cd "$(dirname "$0")/.."

cyan() { printf '\033[36m%s\033[0m\n' "$*"; }
dim()  { printf '\033[2m%s\033[0m\n' "$*"; }

cyan "1. minting three fresh keypairs…"
declare -a pubkeys=()
for i in 1 2 3; do
  k=$(mktemp -t "k256-replay-batch-${i}.XXXXXX.json")
  solana-keygen new --no-bip39-passphrase --silent --force -o "$k" >/dev/null
  p=$(solana-keygen pubkey "$k")
  pubkeys+=("$p")
  rm -f "$k"
  dim "   #$i: $p"
done
echo

cyan "2. building the multi-patch JSON from patches/multi.json template…"
out=$(mktemp -t k256-replay-multi.XXXXXX.json)
jq \
  --arg p1 "${pubkeys[0]}" \
  --arg p2 "${pubkeys[1]}" \
  --arg p3 "${pubkeys[2]}" \
  '.patches[0].pubkey = $p1
 | .patches[1].pubkey = $p2
 | .patches[2].pubkey = $p3' \
  patches/multi.json > "$out"
dim "   wrote: $out"
echo "$ cat $out"
cat "$out" | jq .
echo

cyan "3. applying the batch (one HTTP call, all three or none)…"
echo "$ k256-replay patch apply --from-json $out --yes"
echo
k256-replay patch apply --from-json "$out" --yes
echo

cyan "4. ask the orchestrator for the last 5 patch records…"
dim "   the JSON array is oldest-first; \`.[-5:]\` is the tail."
echo "$ k256-replay patch history --json | jq '.[-5:][].pubkey'"
echo
k256-replay patch history --json | jq -r '.[-5:][] | .pubkey // .patches[]?.pubkey // empty'

rm -f "$out"
