#!/usr/bin/env bash
# 05-inspect-idl.sh — read any program's Anchor IDL through the CLI.
#
# Wraps the gateway's public `GET /idl/<programId>` route. The CLI
# derives the IDL account PDA (`anchor:idl` seed + program ID), fetches
# the account from mainnet RPC, zlib-decompresses the JSON, and returns
# `{ program_id, idl_address, idl }`. Edge-cached 1h. 404 means the
# program does not publish an Anchor IDL (System / Token / native
# loaders / Phoenix / Kamino).

set -euo pipefail
cd "$(dirname "$0")/.."

cyan() { printf '\033[36m%s\033[0m\n' "$*"; }
dim()  { printf '\033[2m%s\033[0m\n' "$*"; }

# Well-known Anchor programs that publish IDLs.
# Indexed arrays — works on bash 3.2 (macOS default) without declare -A.
demos=(
  "Whirlpool|whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc"
  "Drift|dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH"
  "MarginFi|MFv2hWf31Z9kbCa1snEPYctwafyhdvnV7FZnsebVacA"
)

for entry in "${demos[@]}"; do
  name="${entry%%|*}"
  program_id="${entry##*|}"
  cyan "=== ${name} (${program_id}) ==="
  echo "$ k256-replay idl ${program_id}"
  echo
  k256-replay idl "$program_id"
  echo
done

cyan "Full IDL JSON (Whirlpool):"
echo "$ k256-replay idl whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc --json \\"
echo "    | jq '.idl | {name: .metadata.name, instructions: (.instructions | length), accounts: (.accounts | length), types: (.types | length)}'"
echo
k256-replay idl whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc --json \
  | jq '.idl | {name: .metadata.name, instructions: (.instructions | length), accounts: (.accounts | length), types: (.types | length)}'
echo

cyan "What happens for a non-Anchor program (SPL Token):"
echo "$ k256-replay idl TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
echo
# We intentionally let this exit non-zero — exit code 4 == 404 — so
# operators can see how the CLI surfaces the "no IDL" case. `|| true`
# keeps the recipe from aborting under `set -e`.
k256-replay idl TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA || true
