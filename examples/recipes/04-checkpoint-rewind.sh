#!/usr/bin/env bash
# 04-checkpoint-rewind.sh — the killer Replay loop.
#
# 1. Save a checkpoint of the current fork (the "rewind point").
# 2. Mutate the fork (patch lamports onto a fresh account).
# 3. Restore the checkpoint.
# 4. Confirm the patch is gone — the fork is back to step-1 state.
#
# Restoration uses reflinks under the hood — the file copy is O(1) on
# btrfs/XFS reflink-capable filesystems. `checkpoint restore` returns
# as soon as the orchestrator accepts the request (202 Accepted); the
# validator is then dropped, the memory snapshot is reflinked into
# place, and the fork comes back up. During that ~30-120 s window the
# gateway returns 502/503 — that's expected. After `restore` returns,
# poll `k256-replay status` until `phase=ready` and `rpc=listening`
# before issuing follow-up calls. This recipe's step 5 polls the
# fork's Solana RPC for up to 120 s on its own.

set -euo pipefail
cd "$(dirname "$0")/.."

cyan()  { printf '\033[36m%s\033[0m\n' "$*"; }
dim()   { printf '\033[2m%s\033[0m\n' "$*"; }
red()   { printf '\033[31m%s\033[0m\n' "$*"; }
yellow(){ printf '\033[33m%s\033[0m\n' "$*"; }

# Pre-flight: if the host /data is too full to fit one more checkpoint,
# the orchestrator returns 507 Insufficient Storage and there's nothing
# this recipe can do mid-flight to recover. Surface it here BEFORE we
# mint a keypair / generate clutter, with a clear actionable path:
# either bulk-prune via the CLI or use the dashboard's prune dialog
# on the Checkpoints tab.
cyan "0. pre-flight: checking host /data has room for one more checkpoint…"
disk_json=$(k256-replay status --json 2>/dev/null || echo '{}')
refuse=$(jq -r '.disk.refuse_new_checkpoint // false' <<<"$disk_json")
pct=$(jq -r '.disk.pct_used // 0 | floor' <<<"$disk_json")
free_bytes=$(jq -r '.disk.free_bytes // 0' <<<"$disk_json")
budget=$(jq -r '.disk.checkpoint_budget_bytes // 0' <<<"$disk_json")
if [[ "$refuse" == "true" ]]; then
  red "✗ host /data is too full for another checkpoint."
  red "  - used: ${pct}%"
  red "  - free: $((free_bytes / 1024 / 1024 / 1024)) GB"
  red "  - one more checkpoint needs ~$((budget / 1024 / 1024 / 1024)) GB"
  echo
  yellow "  free space first, then re-run this recipe:"
  echo "    # see what's eating disk:"
  echo "    k256-replay checkpoint list"
  echo
  echo "    # keep the most recent 1 (or 0 if you have none worth keeping):"
  echo "    k256-replay checkpoint prune --keep-latest 1 --yes"
  echo
  echo "  (or use the dashboard's Checkpoints tab → Prune… button)"
  exit 1
fi
dim "   disk: ${pct}% used, $((free_bytes / 1024 / 1024 / 1024)) GB free (ok)"
echo

label="demo-rewind-$(date +%s)"
keypair=$(mktemp -t k256-replay-victim.XXXXXX.json)
solana-keygen new --no-bip39-passphrase --silent --force -o "$keypair" >/dev/null
victim=$(solana-keygen pubkey "$keypair")
rm -f "$keypair"
dim "   victim pubkey: $victim"
dim "   checkpoint label: $label"
echo

cyan "1. saving baseline checkpoint (orchestrator pauses fc-agent briefly)…"
echo "$ k256-replay checkpoint save --label $label"
echo
k256-replay checkpoint save --label "$label"
echo

cyan "2. resolving the checkpoint id we just minted…"
ckpt_id=$(k256-replay checkpoint list --json | jq -r --arg L "$label" '.[] | select(.label == $L) | .id' | head -1)
if [[ -z "$ckpt_id" ]]; then
  printf '\033[31m✗ could not resolve checkpoint id for label %s\033[0m\n' "$label"
  exit 1
fi
dim "   ckpt id: $ckpt_id"
echo

cyan "3. mutating the fork: 5 SOL onto ${victim}…"
# `replace` because the keypair is brand-new — `merge` is for tweaking
# accounts that already exist on the fork. See recipe 02's header.
k256-replay patch apply \
  --pubkey "$victim" \
  --mode replace \
  --lamports 5000000000 \
  --owner 11111111111111111111111111111111 \
  --executable false \
  --rent-epoch 18446744073709551615 \
  --data-base64 "" \
  --yes
echo

cyan "4. restoring the baseline checkpoint (validator briefly restarts)…"
echo "$ k256-replay checkpoint restore $ckpt_id"
echo
k256-replay checkpoint restore "$ckpt_id"
echo

cyan "5. checking the victim is gone (lamports==0, account doesn't exist)…"
if [[ -z "${FORK_RPC:-}" ]]; then
  cat <<EOF
\$FORK_RPC is not set, so the recipe skips the live RPC check. Get the
Solana RPC URL from your server's Access page in the dashboard, then
re-run with:

  export FORK_RPC="<url>"
  ./recipes/04-checkpoint-rewind.sh

Then independently verify:

  solana balance ${victim} --url \$FORK_RPC
  # → 0  (account does not exist after the restore)

  curl -sS \$FORK_RPC -X POST -H 'content-type: application/json' \\
    -d '{"jsonrpc":"2.0","id":1,"method":"getAccountInfo","params":["${victim}"]}'
  # → "value": null
EOF
else
  # The restore is async — fc-agent reflinks the snapshot in place and
  # the validator briefly drops off the network. We poll for up to
  # 150s. Two valid "rolled back" signals from getBalance:
  #   * balance = 0      — account exists with no lamports (rare)
  #   * value   = null   — account doesn't exist at all (normal)
  # While the validator is mid-restart, getBalance either errors out
  # at the network layer or the JSON-RPC returns an error envelope.
  # Label those clearly so the heartbeat isn't confusing.
  ok=0
  for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do
    sleep 10
    resp=$(curl -sS --max-time 6 "$FORK_RPC" -X POST -H 'content-type: application/json' \
      -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getBalance\",\"params\":[\"${victim}\"]}" 2>/dev/null || true)
    if [[ -z "$resp" ]]; then
      bal="rpc-down"
    else
      # `.result.value` is `null` when the account is absent, an integer
      # when it has lamports, and absent when the JSON-RPC returned an
      # error envelope. Map every shape to a single token.
      bal=$(jq -r '
        if .error then "rpc-err:" + (.error.message // "unknown")
        elif (.result.value // "absent") == "absent" then "absent"
        elif .result.value == null then "absent"
        elif .result.value == 0 then "0"
        else (.result.value | tostring)
        end' <<<"$resp" 2>/dev/null || echo "parse-failed")
    fi
    if [[ "$bal" == "absent" || "$bal" == "0" ]]; then
      printf '\033[32m   ✓ verified after %ds: account is %s — the patch was rolled back\033[0m\n' "$((i*10))" "$bal"
      ok=1
      break
    fi
    dim "   t+$((i*10))s: balance=${bal} (fork still restarting / restore landing — retrying)"
  done
  if [[ "$ok" -ne 1 ]]; then
    printf '\033[31m   ✗ account still has lamports after 150s\033[0m\n'
    printf '\033[31m     check `k256-replay status` and `k256-replay logs -n 200`; retry manually\033[0m\n'
    exit 1
  fi
fi
