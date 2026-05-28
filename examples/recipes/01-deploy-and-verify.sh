#!/usr/bin/env bash
# 01-deploy-and-verify.sh — upload `memo-program.so` to a fresh program ID,
# then confirm the bytecode landed via two independent paths:
#
#   a) `k256-replay deploy history`     ← orchestrator audit log
#   b) `getAccountInfo` on the fork RPC ← canonical proof
#
# This is the simplest "do a real thing on a real fork" recipe. The .so
# is a verbatim copy of the SPL Memo program — a real Solana program,
# not a placeholder.

set -euo pipefail
cd "$(dirname "$0")/.."

cyan() { printf '\033[36m%s\033[0m\n' "$*"; }
dim()  { printf '\033[2m%s\033[0m\n' "$*"; }

cyan "1. minting a fresh program keypair…"
keypair=$(mktemp -t k256-replay-prog.XXXXXX.json)
solana-keygen new --no-bip39-passphrase --silent --force -o "$keypair" >/dev/null
program_id=$(solana-keygen pubkey "$keypair")
authority=$program_id      # for the demo, the program is also its own authority
dim "   program id: ${program_id}"
dim "   keypair:    ${keypair}"
echo

cyan "2. confirming the orchestrator can see the fork (via the CLI)…"
phase=$(k256-replay status --json | jq -r .phase)
slot=$(k256-replay status --json | jq -r '.current_slot // "—"')
dim "   phase: ${phase}   slot: ${slot}"
if [[ "$phase" != "ready" ]]; then
  printf '\033[31m✗ fork is not ready (phase=%s) — wait for boot to finish, then re-run\033[0m\n' "$phase"
  exit 1
fi
# The deploy below goes through the orchestrator (api-replay.k256.xyz).
# The verification step (5) goes through the fork's Solana RPC URL —
# that's what `$FORK_RPC` points at (export it from your server's
# Access page). The CLI does NOT wrap Solana RPC queries.

cyan "3. uploading the Memo ELF (74,800 bytes) to ${program_id:0:8}…${program_id: -4}…"
echo "$ k256-replay deploy program --program-id ${program_id:0:8}… --elf examples/memo-program.so \\"
echo "    --status deployed --authority ${authority:0:8}… --yes"
echo
k256-replay deploy program \
  --program-id "$program_id" \
  --elf "$(pwd)/memo-program.so" \
  --status deployed \
  --authority "$authority" \
  --yes
echo

cyan "4. reading the orchestrator's deploy history (audit log)…"
dim "   the JSON array is oldest-first; \`.[-1]\` is the deploy we just did."
echo "$ k256-replay deploy history --json | jq '.[-1]'"
echo
k256-replay deploy history --json | jq '.[-1]'
echo

cyan "5. verify on chain via the fork's Solana RPC."
if [[ -z "${FORK_RPC:-}" ]]; then
  cat <<EOF
\$FORK_RPC is not set, so the recipe skips the live RPC check. Get the
Solana RPC URL from your server's Access page in the dashboard
(https://app.k256.xyz/app/replay) and re-run with:

  export FORK_RPC="<url>"
  ./recipes/01-deploy-and-verify.sh

What the verification does (you can also run it by hand):

  curl -sS \$FORK_RPC -X POST -H 'content-type: application/json' \\
    -d '{"jsonrpc":"2.0","id":1,"method":"getAccountInfo","params":["${program_id}",{"encoding":"base64"}]}'

Expected:
  - "owner": "LoaderV411111111111111111111111111111111111"
  - "executable": true
  - data offset 48..52 (base64) == "f0VMRg==" (i.e. \\x7fELF — the ELF magic)
EOF
else
  echo "$ curl -sS \$FORK_RPC -X POST ... getAccountInfo ${program_id:0:8}…${program_id: -4}"
  resp=$(curl -sS --max-time 10 "$FORK_RPC" -X POST -H 'content-type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccountInfo\",\"params\":[\"${program_id}\",{\"encoding\":\"base64\",\"dataSlice\":{\"offset\":48,\"length\":4}}]}")
  owner=$(jq -r '.result.value.owner // "null"'      <<<"$resp")
  exec_=$(jq -r '.result.value.executable // "null"' <<<"$resp")
  elf=$(jq -r '.result.value.data[0] // "null"'      <<<"$resp")
  printf '   %-12s %s\n' "owner"      "$owner"
  printf '   %-12s %s\n' "executable" "$exec_"
  printf '   %-12s %s   (expect "f0VMRg==" — the ELF magic)\n' "elf magic" "$elf"
  if [[ "$owner" == "LoaderV411111111111111111111111111111111111" && "$exec_" == "true" && "$elf" == "f0VMRg==" ]]; then
    printf '\033[32m   ✓ verified: the program is live and the bytecode landed\033[0m\n'
  else
    printf '\033[31m   ✗ verification failed — see the response above\033[0m\n'
    exit 1
  fi
fi

rm -f "$keypair"
