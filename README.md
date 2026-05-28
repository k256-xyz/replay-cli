# `k256-replay` — operator + agent CLI for k256 Replay

**Replay** is Solana mainnet, paused on a snapshot you choose. You boot
a fork, advance it forward, splice transactions into a future block,
patch any account, deploy programs, manage Geyser plugins, and inspect
the **exact pre/post account state diff** the validator committed for
every non-vote transaction.

The web console at <https://app.k256.xyz/app/replay> is the operator
home; the [`/llms.txt`](https://k256.xyz/replay/llm-docs.md) doc is the
canonical HTTP contract for agents. This CLI sits between them — a
shell-friendly companion for operators iterating on the workbench loop
and scripts that drive the fork in CI.

The companion repo
[k256-xyz/replay-examples](https://github.com/k256-xyz/replay-examples)
shows the **read** transports (`rpc` / `ws` / `grpc`) without
abstractions; the CLI here is the **write + diff** side.

> Not affiliated with Temporal's `delorean-client`. delorean is a
> single-shot fixture replayer. `k256-replay` drives a live, mutable
> fork.

## Prerequisites

The CLI itself only needs Rust. The bundled recipes additionally call
`solana-keygen`, `solana`, `jq`, and `curl` to mint keys and verify
on-chain state — make sure these are on `PATH` before running them:

| tool          | why                                                                      |
| ------------- | ------------------------------------------------------------------------ |
| Rust stable   | builds the CLI binary                                                    |
| Solana CLI    | `solana-keygen` (mint program / wallet keys), `solana account` / `solana balance` / `solana block` (read fork state via JSON-RPC) |
| `jq`          | parses CLI `--json` output in scripts                                    |
| `curl`        | optional — for direct JSON-RPC against the fork in verification steps    |

Install the Solana CLI with [the official installer](https://solana.com/developers/guides/getstarted/setup-local-development);
`jq` and `curl` ship with most package managers (`brew install jq` /
`apt install jq curl`).

## Install

```bash
cargo install --git https://github.com/k256-xyz/replay-cli --locked
```

This drops a single static binary `k256-replay` into `~/.cargo/bin`.
Add that to `PATH` if it isn't already:

```bash
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.zshrc   # or ~/.bashrc
```

## Configure

```bash
export REPLAY_API_KEY="rpl_live_…"          # from the Access page
```

Auth precedence: `--key <TOKEN>` → `REPLAY_API_KEY` → error. The CLI
does not write your key to disk.

Endpoint precedence: `--endpoint <URL>` → `REPLAY_ENDPOINT` →
`https://api-replay.k256.xyz`. Every customer reaches their own box
through the gateway; it routes by bearer to the box that owns the
key. Don't point `--endpoint` at the box directly — the orchestrator
port isn't a customer-facing surface.

## First 5 minutes

A fresh-from-zero walkthrough. Assumes you've finished **Prerequisites**
and **Configure** above.

```bash
# 1. Confirm the bearer reaches the gateway and the fork is ready.
k256-replay status
#   phase    ready                                slot      …
#   …
#   cache    … fixtures   … slots   … MB / 5 GB

# 2. Inspect a real Anchor IDL (no bearer needed — proves the gateway is up
#    even before you touch the fork).
k256-replay idl whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc

# 3. Advance the fork one slot. `--wait` blocks until phase=ready again.
NOW=$(k256-replay status --json | jq -r .current_slot)
k256-replay advance start --to $((NOW + 1)) --wait

# 4. Take a baseline checkpoint so you can rewind any time.
k256-replay checkpoint save --label first-5-minutes

# 5. Mutate the fork — seed a fresh account with 2.5 SOL. We use
#    `replace` (not `merge`) because the account doesn't exist yet
#    on the fork; see `patch apply` below for the merge-vs-replace
#    table.
PUB=$(solana-keygen new --no-bip39-passphrase --silent --force -o /tmp/k.json && solana-keygen pubkey /tmp/k.json)
k256-replay patch apply --pubkey "$PUB" --mode replace \
  --lamports 2500000000 \
  --owner 11111111111111111111111111111111 \
  --executable false \
  --rent-epoch 18446744073709551615 \
  --data-base64 "" \
  --yes

# 6. Verify on chain via the fork's Solana RPC. Get $FORK_RPC from your
#    server's page in the dashboard (Access → "Solana RPC URL").
solana balance "$PUB" --url "$FORK_RPC"
#   2.5 SOL

# 7. Rewind under the same checkpoint to see lamports return to 0.
CKPT=$(k256-replay checkpoint list --json | jq -r '.[] | select(.label=="first-5-minutes") | .id' | head -1)
k256-replay checkpoint restore "$CKPT"
```

That covers `status`, `idl`, `advance`, `checkpoint save/restore`,
`patch`, and the Solana-RPC verification loop — the spine of every
real workflow. Everything else in this README extends from those
seven steps.

Two URLs you'll hit constantly:

- **Gateway / orchestrator** (this CLI talks to it): `https://api-replay.k256.xyz` — the default, never override unless you're on staging.
- **Fork Solana RPC** (your client / `solana` / `web3.js`): shown on your server's Access page in [the dashboard](https://app.k256.xyz/app/replay). Standard Solana JSON-RPC.

## Shell completion

```bash
k256-replay completion bash > ~/.local/share/bash-completion/completions/k256-replay
k256-replay completion zsh  > /usr/local/share/zsh/site-functions/_k256-replay
k256-replay completion fish > ~/.config/fish/completions/k256-replay.fish
```

Tab-completion covers every subcommand, every flag, every fixed value
(modes, statuses, shell names). `completion` and `idl` are the
subcommands that work without a bearer (`idl` proxies a public
mainnet read; everything else routes by bearer to your box).

## At a glance

```bash
k256-replay status                              # phase, slot, advance, workbench cache
k256-replay status --watch                      # live `top`-style refresh
k256-replay status --json | jq .                # raw /status body

k256-replay advance start --to 422097300 --wait # drive the fork
k256-replay advance status                      # current/last job
k256-replay advance history -n 5                # recent jobs
k256-replay advance cancel --yes                # SIGTERM the rpc-shreds child

k256-replay diff <SIG>                          # colored state-diff table
k256-replay diff <SIG> --json | jq .accounts    # wire shape
k256-replay diff <SIG> --bytes                  # pre/post base64

k256-replay checkpoint save --label baseline
k256-replay checkpoint list
k256-replay checkpoint restore ckpt_…           # instant reflink CoW rewind

k256-replay patch apply --pubkey <PUBKEY> --mode merge --lamports 1000000000 --yes   # existing account
k256-replay patch apply --pubkey <PUBKEY> --mode replace --lamports 1000000000 \
    --owner 11111111111111111111111111111111 --executable false --rent-epoch 18446744073709551615 \
    --data-base64 "" --yes                                                            # fresh account
k256-replay patch apply --from-json patches.json --yes
k256-replay patch history -n 10

k256-replay deploy program --program-id <ID> --elf ./prog.so \
    --status deployed --authority <ADDR> --yes
k256-replay deploy history

k256-replay plugin list
k256-replay plugin upload --lib geyser.so --config config.json
k256-replay plugin delete --id <ID> --yes

k256-replay idl <PROGRAM_ID>                    # decoded Anchor IDL summary
k256-replay idl <PROGRAM_ID> --json | jq .idl   # full IDL envelope

k256-replay fixtures stats                      # cache snapshot
k256-replay fixtures stats --json               # raw /fixtures/stats body
k256-replay fixtures sample -n 5                # 5 captured sigs to diff
k256-replay fixtures sample -n 1 --json         # one captured sig (script-friendly)
k256-replay fixtures clear --yes                # drop every captured fixture

k256-replay clear --yes                         # alias of `fixtures clear`
k256-replay logs -n 200                         # one-shot tail
k256-replay logs --follow                       # live SSE stream

k256-replay snapshots                           # browse the catalog (dashboard auth)
k256-replay kill --yes                          # stop the validator
```

## The workbench loop

The killer flow this CLI is built around:

```bash
k256-replay checkpoint save --label baseline                              # rewind point
NOW=$(k256-replay status --json | jq -r .current_slot)
k256-replay advance start --to $((NOW + 1)) --wait                         # drive forward
SIG=$(... pick a signature from the new block ...)
k256-replay diff $SIG                                                      # what changed?

# tweak something:
k256-replay patch apply --pubkey <ACCT> --mode merge --lamports 1000000000 --yes

# To see the SAME transaction execute against the patched state you
# must splice it into a future slot — `diff <SIG>` always returns the
# captured fixture from when the tx first landed; `advance` alone
# replays canonical mainnet shreds, it does NOT re-execute past txs.
# Send a splice via POST /advance with `splices: [{ slot, position,
# transactions_base64 }]` — see the orchestrator's `Transaction splices`
# section. The new signature you get is the one to `diff`.

k256-replay checkpoint restore ckpt_…                                      # rewind for a clean rerun
```

The diff table mirrors the web console's **Blocks → State diff** tab
line for line: per-account row with role chips, change classification
(`changed` / `unchanged` / `unknown` / `role-only`), 2-line lamports
stack, owner short form, data summary (`same` / `len N` / `len N → M` /
`not captured`).

## Subcommands

Every read command supports `--json` so scripts can pipe to `jq`.
Every destructive command requires `--yes`.

### `status [--watch] [--json]`

Phase, slot, snapshot slot, RPC ready, mutation.dirty, advance state,
workbench cache stats — one screen. `--watch` redraws every 1.5 s
(Ctrl-C to quit). `--json` emits the raw `/status` body **only** (no
cache stats; the rendered view fetches `/fixtures/stats` on top of
`/status` and joins them client-side). For raw cache stats use
`k256-replay fixtures stats --json`.

### `diff <SIG> [--json] [--bytes]`

`GET /fixtures/tx/:signature`. Pretty-prints the per-account diff.
`--bytes` shows base64 pre/post for changed rows. `--json` emits the
wire fixture verbatim — the agent-friendly shape.

### `advance start|status|history|cancel`

| verb     | route                  | purpose                                          |
| -------- | ---------------------- | ------------------------------------------------ |
| start    | `POST /advance`        | kick a new advance; `--wait` blocks until ready  |
| status   | `GET  /advance`        | active job, or most recent if idle               |
| history  | `GET  /advance/history`| up to 20 completed jobs (orchestrator-side cap)  |
| cancel   | `POST /advance/cancel` | SIGTERM the rpc-shreds child of the active job   |

`advance start --to <N>` pre-checks `current_slot` locally so a typo
returns "must be greater than X" without a round-trip. `--wait` polls
`/status` every 1.5 s and prints per-slot progress until one of:

- **success** — phase is `ready` AND `current_slot >= target` (returns 0)
- **partial** — orchestrator marked `advance.status=done` but the fork
  didn't reach the target — error with the gap surfaced (returns 1)
- **failed** / **cancelled** — error with the cause (returns 1)
- **timeout** — wait budget exceeded (returns 1)

`--wait-timeout SEC` defaults to 600.

### `checkpoint save|list|restore`

| verb     | route               | purpose                                                                |
| -------- | ------------------- | ---------------------------------------------------------------------- |
| save     | `POST /checkpoint`  | snapshot the fork; `--label` is stored alongside the id                |
| list     | `GET  /checkpoints` | id / label / slot / created_at table (or `--json` for the raw payload) |
| restore  | `POST /restore`     | reflink CoW rewind to a saved checkpoint                               |

`checkpoint save --label baseline` polls `/checkpoints` for up to 240 s
to confirm the save landed — `fc-agent` pauses the guest briefly while
the memory snapshot is taken, so the orchestrator can be unreachable
mid-save and that's expected.

### `patch apply|history`

`apply` has two modes — pick based on whether the account already
exists on the fork:

| mode      | account must exist? | what's touched                                       | use for                                              |
| --------- | ------------------- | ---------------------------------------------------- | ---------------------------------------------------- |
| `merge`   | yes (returns 400 if not) | only the fields you set on the request          | tweaking real-world state (whirlpool reserve, mint authority, …) |
| `replace` | no — creates if missing  | every field is set explicitly; missing fields default (lamports=0, owner=System, executable=false, rent_epoch=u64::MAX, data=empty) | seeding fresh accounts (test wallets, mock PDAs) |

For a fresh keypair, `merge` would return `400 invalid_params: account
not found` from the validator (this is a safety net to catch typo'd
pubkeys). Use `replace` and fill in the four extra fields.

**Single patch via flags** —
```bash
k256-replay patch apply \
    --pubkey <PUBKEY> \
    --mode merge \              # or `replace`
    --lamports 1000000000 \
    --owner <PROGRAM_ID> \
    --executable false \
    --rent-epoch 18446744073709551615 \
    --data-base64 ... \         # or --data-file PATH (CLI base64-encodes)
    --allow-epoch-boundary \    # optional; refused by the orchestrator otherwise
    --yes
```

**Bulk patches via JSON** —
```bash
cat patches.json
{
  "patches": [
    { "pubkey": "...", "mode": "merge", "lamports": 1000000000 },
    { "pubkey": "...", "mode": "replace", "data_base64": "..." }
  ],
  "allow_epoch_boundary": false
}

k256-replay patch apply --from-json patches.json --yes
```

The CLI sets `confirm_dangerous: true` on the wire for you when you
pass `--yes`. Without `--yes` the command refuses locally before
sending anything.

`history -n N` (default 20) lists the audit log (capped at 2,000
records server-side). `--json` for the raw row shapes.

### `deploy program|history`

```bash
k256-replay deploy program \
    --program-id <PUBKEY> \
    --elf ./build/program.so \          # CLI reads + base64-encodes; cap ≈ 13 MiB
    --status deployed \                 # or `finalized`
    --authority <ADDR> \                # required for --status deployed
    --next-version <ADDR> \             # required for --status finalized
    --force-replace-finalized \         # destructive
    --force-replace-legacy-loader \     # destructive
    --yes
```

The orchestrator writes a Loader-v4 account at `--program-id`. Pre-
flight: `--status deployed` requires `--authority`; `--status
finalized` requires `--next-version`. Either status overwrites the
program's existing bytes.

`history -n N` for the deploy audit log; `--json` for the raw shape.

### `plugin list|upload|delete`

Custom Geyser plugins. The `.so` must be `x86_64-unknown-linux-gnu`,
glibc 2.39, ABI-matched to the validator, and bind external traffic
to `0.0.0.0:10000`.

```bash
k256-replay plugin list                          # active + uploaded
k256-replay plugin list --json
k256-replay plugin upload --lib geyser.so --config config.json
k256-replay plugin delete --id 1779456000123 --yes
```

`upload` returns a `config_path` — pass that as the `geyser` field on
the next `/boot` to make the validator load it. (`/boot` is
console-only because it needs catalog auth; the CLI doesn't wrap it.)

### `kill --yes`

`POST /kill` SIGTERMs the validator. The orchestrator stays up; HTTP
remains reachable so you can `/boot` again from the web console.
Refuses without `--yes`.

### `idl <PROGRAM_ID> [--json]`

`GET /idl/<programId>` on the gateway. Reads any program's on-chain
Anchor IDL (the account `anchor idl init` publishes), decompresses
the JSON, and prints a one-screen summary by default:

```
program whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc
  name               whirlpool
  version            0.9.0
  idl account        2KFqE4RWoPVbvodo8vbggCFeHPS8TDvgpwp79ALMrcyn
  instructions       66
  account types      12
  defined types      37
  events             6
  error codes        70
```

`--json` emits the canonical `{ program_id, idl_address, idl }`
envelope for piping to `jq`. 404 → exit code 4 with a hint that the
program isn't an Anchor program (System / Token / native loaders /
Phoenix / Kamino don't publish IDLs). The IDL is mainnet-backed and
edge-cached — it does NOT come from your fork.

### `fixtures stats|sample|clear`

| verb     | route                  | purpose                                                  |
| -------- | ---------------------- | -------------------------------------------------------- |
| stats    | `GET  /fixtures/stats` | cache snapshot — bytes used / cap, fixture count, slot range, evicted / oversized / cleared counters |
| sample   | _(client-side)_        | walk the captured slot range backwards via Solana RPC's `getBlock` + probe `/fixtures/tx` for hits. Returns up to N captured signatures ready to feed into `diff`. |
| clear    | `POST /fixtures/clear` | drop every captured fixture for the current session     |

`fixtures stats` is the same body the `status` command shows inline,
broken out for cron / monitoring loops.

`fixtures sample -n N` is the answer to "how do I find a captured
signature without writing my own slot-scanner". It bridges the
orchestrator (which knows the cached slot range) and Solana RPC
(which knows the signatures per slot), then probes the orchestrator's
`/fixtures/tx` to confirm each hit. `--rpc-url` defaults to
`$FORK_RPC`. Use `--json` for `jq` piping.

`fixtures clear` is the canonical name; the top-level `clear --yes`
is kept as a legacy alias and goes to the same endpoint.

### `clear --yes`

Alias of `fixtures clear --yes`. Both call `POST /fixtures/clear`;
existing scripts keep working.

### `logs [-n N] [--follow]`

Default (one-shot tail) hits `/logs/tail?lines=N`. `--follow` opens
the SSE stream at `/logs/stream` — Ctrl-C stops; second Ctrl-C exits
immediately. The CLI suppresses SSE keepalive comments (the `:` heart-
beats every 15 s) so the output is clean validator stdout.

### `snapshots [--limit N] [--dashboard URL] [--json]`

Browses `/api/snapshots` on the dashboard origin (uses your WorkOS
session cookie, not the bearer — log in to `app.k256.xyz` first).
`--dashboard` is escape-hatch only (staging environments); production
always uses the default. `/boot` is console-only; the catalog row a
fresh provision picks up flows from the deployer, not the CLI.

### `completion <SHELL>`

Emits a shell-completion script for `bash`, `zsh`, `fish`,
`elvish`, or `powershell`. Doesn't need a bearer.

## Exit codes

Distinct non-zero exit codes let scripts branch without parsing error
text:

| code | class                                                  |
| ---- | ------------------------------------------------------ |
| 0    | success                                                |
| 1    | local error (config, network, decoding)                |
| 2    | **401 unauthenticated _or_ a `clap` argument-validation error** — clap exits with the standard Unix usage-error code, which collides. The stderr message disambiguates: HTTP errors carry `401`/the bearer-rejected hint; clap prints `error: the following required arguments were not provided` / `error: invalid value …`. |
| 3    | 403 forbidden                                          |
| 4    | 404 not found (e.g. fixture / checkpoint)              |
| 5    | 409 conflict (e.g. phase not ready, no active advance) |
| 6    | 400 bad request (e.g. signature, confirm flag)         |
| 7    | 5xx orchestrator                                       |

## Colour and TTY

ANSI escapes auto-disable when stdout isn't a terminal or when
`NO_COLOR=1` is set (per <https://no-color.org/>). Force off with
`--no-color`.

## What the CLI does NOT wrap (and why)

| route                          | reason                                                                                     |
| ------------------------------ | ------------------------------------------------------------------------------------------ |
| `POST /boot`                   | requires catalog lookup (`snapshot_slot` → `r2_key`/`sha256`/`canonical_filename`/`size_bytes`); dashboard-side WorkOS cookie auth. Use `app.k256.xyz/app/replay`. |
| `POST /internal/rotate-key`    | admin-only operation; rotate via the deployer.                                             |
| `GET /healthz` / `GET /readyz` | unauthenticated probes meant for load balancers, not humans.                               |

## Examples

Eight runnable recipes ship in [`examples/`](examples/). Every script
is self-contained — set `REPLAY_API_KEY`, run, watch it drive the
fork through `https://api-replay.k256.xyz`, then read the script to
learn the wire shape. None of them assume you've used Replay before.

```
examples/
├── memo-program.so          # real BPF ELF — SPL Memo, dumped from mainnet
├── patches/
│   ├── lamports.json        # single-account patch body
│   └── multi.json           # three patches in one atomic batch
└── recipes/
    ├── 00-prereqs.sh        # verify environment + gateway reachability
    ├── 01-deploy-and-verify.sh
    ├── 02-patch-and-verify.sh
    ├── 03-batch-patch.sh
    ├── 04-checkpoint-rewind.sh
    ├── 05-inspect-idl.sh
    ├── 06-tail-logs.sh
    └── 07-workbench-loop.sh
```

Run any recipe straight from the repo:

```bash
git clone https://github.com/k256-xyz/replay-cli
cd replay-cli/examples
export REPLAY_API_KEY="rpl_live_…"          # gateway / orchestrator bearer
export FORK_RPC="https://…"                  # optional — Solana JSON-RPC URL
                                             # for the fork; recipes use it to
                                             # independently verify writes
                                             # landed on chain. Set it once
                                             # and recipes 01 / 02 / 04 will
                                             # auto-check instead of just
                                             # printing the curl.
./recipes/00-prereqs.sh           # one-time sanity check
./recipes/01-deploy-and-verify.sh # then anything else
```

Find `FORK_RPC` on your server's Access page at
[`https://app.k256.xyz/app/replay`](https://app.k256.xyz/app/replay) →
"Solana RPC URL".

### What each recipe demonstrates

| recipe | what you'll do | how you verify it |
| ------ | -------------- | ----------------- |
| **01 — deploy** | Upload `memo-program.so` (74,800 bytes, real SPL Memo) to a fresh program ID via `deploy program`. | The CLI prints the orchestrator's `cache_published`, `elf_sha256`, and `acct_after` digests. `deploy history --json \| jq '.[-1]'` shows the audit record. `getAccountInfo` on the fork returns `executable=true`, `owner=LoaderV4…`. |
| **02 — patch** | Mint a fresh keypair, push +2.5 SOL onto it, set its owner to System Program. | `solana balance <pubkey>` returns 2.5 SOL. `patch history --json \| jq '.[-1]'` shows the audit record. |
| **03 — batch patch** | Three patches (different lamports, owner, and account data) in one atomic request. | `patch history --json` shows the three rows. Independent on-chain verification is up to you — `solana balance <pubkey> --url $FORK_RPC` per account. |
| **04 — checkpoint rewind** | Save baseline → patch +5 SOL → restore baseline → confirm patch is gone. | `solana balance` returns 0 (the account no longer exists). |
| **05 — IDL inspection** | `k256-replay idl` on Whirlpool / Drift / MarginFi, plus the 404 path for a non-Anchor program. | Each IDL renders with instruction / account-type / event / error counts. The 404 case shows the CLI's friendly hint and exits 4. |
| **06 — log streams** | One-shot `logs -n 30` and live `logs --follow` SSE. | You see real validator stdout. |
| **07 — workbench loop intro** | `k256-replay fixtures stats` → pick a captured signature from the newest slot → print its `diff`. Stops after the first diff and prints copy-pasteable follow-ups (checkpoint → patch → advance → diff again → restore) with the captured sig pre-filled. Not yet a fully-automated end-to-end loop. | The first diff shows a real captured transaction; the follow-ups are yours to run. |

### The bundled `.so`

`examples/memo-program.so` is a verbatim copy of the SPL Memo program
(`MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr`), dumped from mainnet
on 2026-05-26:

```
size:    74,800 bytes
sha256:  f520eaf096361abbb9639ea4dc3e5388a87b9330e121f476607b87c46ef67954
format:  ELF 64-bit LSB shared object, eBPF, version 1 (SYSV), stripped
```

Refresh it any time with `solana program dump` (see `examples/README.md`).
It's the same byte-for-byte SPL Memo program — once deployed to a
fresh program id on your fork, *external* Solana clients pointed at
the fork's RPC can `sendTransaction` to invoke it (logs
`Memo (len N): <utf8 bytes>`). The CLI does not sign or submit
transactions itself; use `@solana/web3.js` or `solana program
invoke`-style flows from your own client.

### CLI first, Solana RPC only for chain verification

The recipes call orchestrator routes (status, advance, diff,
checkpoint, patch, deploy, plugin, fixtures, idl, logs, snapshots)
**exclusively through `k256-replay`** against
`https://api-replay.k256.xyz`. Every CLI plumbing path (auth header,
error envelope decode, exit-code mapping) is exercised by the same
recipes that ship to end users, so a broken CLI fails loud where you
can see it.

The only `curl`s left in the recipes hit the fork's **Solana
JSON-RPC** — the same surface `@solana/web3.js` and
`solana-cli --url …` already speak — for canonical "did-the-write-
land" reads (`getAccountInfo`, `getBalance`). The CLI doesn't wrap
Solana RPC on purpose; use `solana account` / `solana program show`
/ `web3.js` for those.

### When something fails

The CLI maps every error class to a distinct exit code (see
[Exit codes](#exit-codes)) so scripts can branch without parsing
strings: `0` success, `2` 401 auth, `4` 404 not-found, `5` 409
conflict, `7` 5xx origin. Every recipe also prints the underlying
command shape before running it, so when a recipe stalls you can
copy the exact `k256-replay …` invocation from the script output
and rerun it interactively.

## License

Apache-2.0. See `LICENSE`.
