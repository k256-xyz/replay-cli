#!/usr/bin/env bash
# 02-patch-and-verify.sh — change an account's lamports + owner, then
# read the result back so you can see it actually landed.
#
# This is the canonical write+read loop. Real numbers, real verification.
#
# Mode choice — `merge` vs `replace`:
#   - `merge`:   only the fields you set are touched; the account must
#                already exist on the fork. Use for tweaking real-world
#                state (e.g. a Whirlpool reserve).
#   - `replace`: every field is set explicitly; missing fields default
#                (lamports=0, data=empty). Creates the account if it
#                doesn't exist. Use for fresh keypairs.
#
# This recipe creates a brand-new keypair, so `replace` is the right
# mode — `merge` would return `400 invalid_params: account not found`.

set -euo pipefail
cd "$(dirname "$0")/.."

cyan() { printf '\033[36m%s\033[0m\n' "$*"; }
dim()  { printf '\033[2m%s\033[0m\n' "$*"; }

cyan "1. minting a fresh account keypair to patch onto…"
keypair=$(mktemp -t k256-replay-patch.XXXXXX.json)
solana-keygen new --no-bip39-passphrase --silent --force -o "$keypair" >/dev/null
pubkey=$(solana-keygen pubkey "$keypair")
dim "   target: ${pubkey}"
echo

cyan "2. patching: 2.5 SOL, owner = System Program (replace mode)…"
# `replace` mode CREATES the account (and overwrites if it already
# exists), so it's the right mode for fresh keypairs. `merge` is for
# tweaking accounts that ALREADY exist on the fork — see the recipe's
# header comment.
cat <<EOF
$ k256-replay patch apply \\
    --pubkey ${pubkey} \\
    --mode replace \\
    --lamports 2500000000 \\
    --owner 11111111111111111111111111111111 \\
    --executable false \\
    --rent-epoch 18446744073709551615 \\
    --data-base64 "" \\
    --yes
EOF
echo
k256-replay patch apply \
  --pubkey "$pubkey" \
  --mode replace \
  --lamports 2500000000 \
  --owner 11111111111111111111111111111111 \
  --executable false \
  --rent-epoch 18446744073709551615 \
  --data-base64 "" \
  --yes
echo

cyan "3. orchestrator patch history (audit log)…"
dim "   the JSON array is oldest-first; \`.[-1]\` is the patch we just landed."
echo "$ k256-replay patch history --json | jq '.[-1]'"
echo
k256-replay patch history --json | jq '.[-1]'
echo

cyan "4. verify on chain via the fork's Solana RPC."
if [[ -z "${FORK_RPC:-}" ]]; then
  cat <<EOF
\$FORK_RPC is not set, so the recipe skips the live RPC check. Get the
Solana RPC URL from your server's Access page in the dashboard, then
re-run with:

  export FORK_RPC="<url>"
  ./recipes/02-patch-and-verify.sh

Independent verification options (run any one of these):

  solana balance ${pubkey} --url \$FORK_RPC
  # → 2.5 SOL

  curl -sS \$FORK_RPC -X POST -H 'content-type: application/json' \\
    -d '{"jsonrpc":"2.0","id":1,"method":"getAccountInfo","params":["${pubkey}"]}'
  # → "lamports": 2500000000, "owner": "11111111111111111111111111111111"

  # web3.js: connection.getBalance(new PublicKey("${pubkey}"))
  # → 2_500_000_000
EOF
else
  echo "$ curl -sS \$FORK_RPC -X POST ... getAccountInfo ${pubkey:0:8}…${pubkey: -4}"
  resp=$(curl -sS --max-time 10 "$FORK_RPC" -X POST -H 'content-type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccountInfo\",\"params\":[\"${pubkey}\"]}")
  lamports=$(jq -r '.result.value.lamports // 0' <<<"$resp")
  owner=$(jq -r '.result.value.owner // "null"'  <<<"$resp")
  printf '   %-12s %s   (expect 2500000000 = 2.5 SOL)\n' "lamports" "$lamports"
  printf '   %-12s %s   (expect 11111111111111111111111111111111)\n' "owner" "$owner"
  if [[ "$lamports" == "2500000000" && "$owner" == "11111111111111111111111111111111" ]]; then
    printf '\033[32m   ✓ verified: the patch landed on the validator\033[0m\n'
  else
    printf '\033[31m   ✗ verification failed — see the response above\033[0m\n'
    exit 1
  fi
fi

rm -f "$keypair"
