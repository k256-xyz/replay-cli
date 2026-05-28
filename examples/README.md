# `k256-replay` examples

Real, runnable recipes for everything the CLI exposes. Every script
is self-contained — copy-paste, set `REPLAY_API_KEY`, and it works
against `https://api-replay.k256.xyz` (the gateway that routes by
bearer to your box).

## Contents

```
examples/
├── memo-program.so          # real BPF ELF — SPL Memo program, dumped from mainnet
├── patches/
│   ├── lamports.json        # single account: bump lamports + set owner
│   └── multi.json           # 3 patches in one atomic batch (lamports, owner, data)
└── recipes/
    ├── 00-prereqs.sh        # checks: solana CLI, jq, REPLAY_API_KEY, gateway reachable
    ├── 01-deploy-and-verify.sh   # upload memo-program.so → confirm on chain
    ├── 02-patch-and-verify.sh    # patch lamports → read back via RPC
    ├── 03-batch-patch.sh         # multi-patch from JSON
    ├── 04-checkpoint-rewind.sh   # save → mutate → restore → re-verify
    ├── 05-inspect-idl.sh         # fetch a real Anchor IDL through /idl/<programId>
    ├── 06-tail-logs.sh           # SSE follow against the live validator
    └── 07-workbench-loop.sh      # workbench-loop intro: find a captured sig, print its diff,
                                  #   then print the follow-up loop (checkpoint/patch/advance/restore)
```

## Quick start

```bash
# 1. Get a bearer from the Access page in the web console
#    (https://app.k256.xyz/app/replay/<your-id>/access)
export REPLAY_API_KEY="rpl_live_…"

# 2. Confirm everything is wired up
./recipes/00-prereqs.sh

# 3. Run any recipe
./recipes/01-deploy-and-verify.sh
```

Every recipe prints what it's about to do, runs it, and (where it can)
verifies the result. The verification matrix:

| recipe | self-verifies on chain? |
| ------ | ----------------------- |
| 00-prereqs | n/a — environment check |
| 01-deploy-and-verify | yes, when `$FORK_RPC` is set (curls getAccountInfo, checks ELF magic) |
| 02-patch-and-verify | yes, when `$FORK_RPC` is set (curls getBalance, checks lamports + owner) |
| 03-batch-patch | no — prints the orchestrator audit log only. Verify the 3 accounts yourself with `solana balance` (or run recipe 02's verification per pubkey) |
| 04-checkpoint-rewind | yes, when `$FORK_RPC` is set (polls getBalance until the account is absent / 0 lamports) |
| 05-inspect-idl | yes — successful IDL parse + the 404 path |
| 06-tail-logs | partial — confirms /logs/tail returns lines and SSE connects |
| 07-workbench-loop | partial — finds a captured signature and prints its diff; the patch / advance / restore follow-ups are yours to run by hand |

**Rule the recipes follow:** orchestrator routes (status, advance,
diff, checkpoint, patch, deploy, plugin, fixtures, idl, …) go through
`k256-replay` exclusively, against `https://api-replay.k256.xyz`. The
only `curl`s in the recipes are against the fork's **Solana JSON-RPC**
at port 18899 — the canonical Solana surface `@solana/web3.js` and
`solana-cli --url …` already speak. The CLI doesn't wrap Solana RPC
on purpose; use `solana account` / `solana program show` / `web3.js`
for those reads. Calling `k256-replay …` from the recipes means a
broken CLI fails loud on the same paths real users exercise.

## About `memo-program.so`

A real, working program: the SPL Memo program (`MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr`),
dumped from mainnet on 2026-05-26.

```
file:     memo-program.so
size:     74,800 bytes
sha256:   f520eaf096361abbb9639ea4dc3e5388a87b9330e121f476607b87c46ef67954
format:   ELF 64-bit LSB shared object, eBPF, version 1 (SYSV), stripped
behavior: prepends `Memo (len N): <utf8 bytes>` to a transaction's log
```

Refresh it any time from mainnet:

```bash
solana program dump \
  -u https://api.mainnet-beta.solana.com \
  MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr \
  memo-program.so
```

## About `patches/*.json`

Both files match the orchestrator's `PatchRequestBody` shape verbatim —
the CLI sends them to `POST /accounts/patch` after merging in
`confirm_dangerous: true` when `--yes` is passed.

`patches/lamports.json` mutates a single fresh keypair. Edit `pubkey`
before running, or let `recipes/02-patch-and-verify.sh` mint a fresh
one for you.

`patches/multi.json` shows three patches landing in one atomic batch:
one fresh System account funded with 3 SOL, one System account whose
data is set to a UTF-8 string, and one System account whose owner is
flipped to the SPL Token program. All three apply or all three roll
back — atomic at the validator.
