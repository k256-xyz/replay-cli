//! `k256-replay` — operator + agent CLI for k256 Replay.
//!
//! See the README for the operator quickstart and the in-product
//! `/llms.txt` doc at <https://k256.xyz/replay/llm-docs.md> for the
//! full HTTP surface (every request body and error kind). This binary
//! is a shell-friendly companion to the operator console
//! (<https://app.k256.xyz/app/replay>); the killer flow is the
//! workbench loop: `advance` → `diff <sig>` → tweak →
//! `checkpoint restore` → `advance` → `diff` again.
//!
//! Auth precedence:
//!   1. `--key <TOKEN>` flag
//!   2. `REPLAY_API_KEY` env var
//!
//! Endpoint precedence:
//!   1. `--endpoint <URL>` flag
//!   2. `REPLAY_ENDPOINT` env var
//!   3. `https://api-replay.k256.xyz` (the gateway — routes by bearer
//!      to the box that owns the key; the orchestrator port on the
//!      box itself is not a customer-facing surface)

use std::io::Write;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};

mod client;
mod cmd;
mod output;
mod types;

use client::{ReplayClient, DEFAULT_ENDPOINT};

/// Long-form version string: `<crate>-v<semver> (<git-sha>)`. `build.rs`
/// stamps `K256_REPLAY_CLI_GIT_SHA` at compile time; `env!` reads it as
/// a `&'static str` so clap can render it directly.
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("K256_REPLAY_CLI_GIT_SHA"),
    ")",
);

#[derive(Parser, Debug)]
#[command(
    name = "k256-replay",
    version = LONG_VERSION,
    about = "Drive a k256 Replay fork from your shell — status, advance, diff, checkpoint, patch, deploy.",
    long_about = "Operator + agent CLI for k256 Replay. Drive a live Solana mainnet replay validator: advance the fork, capture state-diff fixtures, save and restore checkpoints, patch any account, deploy programs, manage Geyser plugins. See https://k256.xyz/replay/llm-docs.md for the full HTTP API the CLI wraps.",
    propagate_version = true,
    after_help = "Common flows:\n  k256-replay status                              # is the box ready?\n  k256-replay advance start --to 422097300 --wait\n  k256-replay diff 5j6F…AbcD                      # see what changed\n  k256-replay checkpoint save --label baseline    # rewind point\n  k256-replay patch apply --pubkey X --mode merge --lamports 1000000000 --yes   # existing account\n  k256-replay patch apply --pubkey X --mode replace --lamports 1000000000 \\\n      --owner 11111111111111111111111111111111 --executable false \\\n      --rent-epoch 18446744073709551615 --data-base64 \"\" --yes                  # fresh account\n  k256-replay deploy program --program-id X --elf ./prog.so --status deployed --authority A --yes\n  k256-replay plugin list\n  k256-replay logs --follow                       # live SSE tail\n\nExit codes:\n  0 success · 1 local error · 2 401-or-clap · 3 403 · 4 404 · 5 409 · 6 400 · 7 5xx\n\nShell completion:\n  k256-replay completion bash > ~/.local/share/bash-completion/completions/k256-replay\n  k256-replay completion zsh  > /usr/local/share/zsh/site-functions/_k256-replay\n  k256-replay completion fish > ~/.config/fish/completions/k256-replay.fish"
)]
struct Cli {
    /// Replay API bearer (also `REPLAY_API_KEY`).
    #[arg(long, env = "REPLAY_API_KEY", global = true, hide_env_values = true)]
    key: Option<String>,

    /// Gateway base URL. Every customer reaches their box through the
    /// gateway; it routes by bearer to the box that owns the key. The
    /// default is correct in production — override only when pointing
    /// the CLI at a non-prod gateway.
    #[arg(long, env = "REPLAY_ENDPOINT", global = true, default_value = DEFAULT_ENDPOINT)]
    endpoint: String,

    /// Disable ANSI colour. Auto-disables when stdout isn't a TTY or
    /// `NO_COLOR` is set; pass `--no-color` to force off in either case.
    #[arg(long, global = true)]
    no_color: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Print phase, slot, validator version, advance state, and
    /// workbench cache stats. `--watch` re-prints every 1.5 s.
    /// `--json` emits the raw `/status` body ONLY — cache stats live
    /// on a separate endpoint; use `k256-replay fixtures stats --json`
    /// for those.
    Status {
        /// Loop forever, refreshing every 1.5 s. Press Ctrl-C to quit.
        #[arg(short = 'w', long)]
        watch: bool,
        /// Print the raw JSON `/status` body instead of the rendered table.
        #[arg(long, conflicts_with = "watch")]
        json: bool,
    },

    /// Fetch the state-diff fixture for one transaction and pretty-print
    /// the per-account diff. The killer workbench command.
    ///
    /// JSON shape (`--json`):
    ///   - `signature`           captured tx signature
    ///   - `execution.success`   true if the tx executed
    ///   - `execution.error`     fee / compute / runtime error if any
    ///   - `execution.fee_paid`  lamports
    ///   - `accounts[]`          per-account rows, indexed in tx order
    ///       `.pubkey`           full base58
    ///       `.changed`          bool (lamports/owner/executable/data delta)
    ///       `.changed_fields`   ["lamports","owner","data",...]
    ///       `.pre.lamports`     pre-execution (Option<u64>)
    ///       `.post.lamports`    post-execution
    ///       `.data.pre_base64`  pre-execution data (when `--bytes` would print it)
    ///       `.data.post_base64` post-execution data
    ///       `.roles`            instruction / signer / writable flags
    ///   - `cache.fixture_truncated`  true if inline bytes were stripped
    ///   - `cache.inline_account_data_max_bytes`  per-side data cap
    ///
    /// Pipe into jq for scripting:
    ///   k256-replay diff <sig> --json | jq '.accounts[] | select(.changed) | .pubkey'
    Diff {
        /// Base58 transaction signature (88 chars).
        signature: String,

        /// Emit the raw fixture JSON instead of the rendered table.
        /// See the JSON shape note above for the field schema.
        #[arg(long, conflicts_with = "bytes")]
        json: bool,

        /// Show the base64 pre/post bytes inline for each changed row
        /// (verbose; default elides them).
        #[arg(long)]
        bytes: bool,
    },

    /// Advance the fork forward, query the live job, or cancel it.
    Advance {
        #[command(subcommand)]
        action: AdvanceAction,
    },

    /// Save, list, or restore a checkpoint of the current fork.
    Checkpoint {
        #[command(subcommand)]
        action: CheckpointAction,
    },

    /// Apply account patches or inspect the mutation audit log.
    Patch {
        #[command(subcommand)]
        action: PatchAction,
    },

    /// Deploy a program from a base64 ELF, or inspect the deploy audit log.
    Deploy {
        #[command(subcommand)]
        action: DeployAction,
    },

    /// List uploaded Geyser plugins, upload a new one, or delete one.
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },

    /// Stop the running validator. The orchestrator stays up; use the
    /// web console (or a fresh `/boot`) to start a new session.
    Kill {
        /// Bypass the confirmation prompt. Required.
        #[arg(long)]
        yes: bool,
    },

    /// Drop every captured workbench fixture. Refuses without `--yes`.
    /// Alias of `fixtures clear`; kept at the top level for legacy
    /// scripts.
    Clear {
        /// Bypass the safety prompt. Required because clear is irreversible.
        #[arg(long)]
        yes: bool,
    },

    /// Inspect or manage the workbench fixture cache (`/fixtures/*`).
    Fixtures {
        #[command(subcommand)]
        action: FixturesAction,
    },

    /// Fetch a program's on-chain Anchor IDL through the gateway
    /// (`GET /idl/<programId>`). Read-only, mainnet-backed,
    /// edge-cached. 404 when the program does not publish an IDL.
    Idl {
        /// Base58 program id.
        program_id: String,
        /// Emit the canonical `{program_id, idl_address, idl}` envelope
        /// for piping into `jq`.
        #[arg(long)]
        json: bool,
    },

    /// Tail recent validator log lines or follow them live.
    Logs {
        /// Number of lines to return (1..10000). Default 200 server-side.
        #[arg(short = 'n', long)]
        lines: Option<u32>,
        /// Stream new lines indefinitely (SSE). Press Ctrl-C to stop.
        /// Mutually exclusive with `--lines`.
        #[arg(short = 'f', long, conflicts_with = "lines")]
        follow: bool,
    },

    /// List bootable snapshots from the dashboard catalog. Uses your
    /// dashboard WorkOS session (you must be logged in to app.k256.xyz);
    /// for terminal-only flows, see the `--dashboard` flag.
    Snapshots {
        /// Dashboard origin hosting `/api/snapshots`. Override for staging.
        #[arg(long, default_value = client::DEFAULT_DASHBOARD)]
        dashboard: String,
        /// Max rows.
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: u32,
        /// Emit the raw JSON catalog instead of the rendered table.
        #[arg(long)]
        json: bool,
    },

    /// Print shell completion script. Save it where your shell will
    /// pick it up; the help text shows the canonical paths.
    Completion {
        /// Shell to generate completions for.
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Pre-flight check — verifies your environment can talk to the
    /// gateway and run the example recipes. Catches the most common
    /// "why is nothing working" causes (missing API key, expired
    /// token, broken local Solana RPC, jq / solana CLI not on PATH)
    /// in one place. Exit 0 = ready; non-zero = at least one check
    /// failed (and printed an actionable hint).
    Doctor,
}

#[derive(Subcommand, Debug)]
pub enum AdvanceAction {
    /// POST /advance — kick a new advance job.
    Start {
        /// Target slot to advance to. Must be > current_slot.
        #[arg(long = "to")]
        to: u64,
        /// Block and poll /status until phase=ready (default 600 s budget).
        #[arg(short = 'w', long)]
        wait: bool,
        /// Max seconds to wait when `--wait` is set.
        #[arg(long, default_value_t = 600, requires = "wait")]
        wait_timeout: u64,
    },
    /// GET /advance — current active job, or the most recent finished one.
    Status {
        /// Emit raw JSON.
        #[arg(long)]
        json: bool,
    },
    /// GET /advance/history — last 20 completed jobs (orchestrator-side cap).
    History {
        #[arg(long)]
        json: bool,
        /// Show at most N rows (client-side truncation; the server cap is 20).
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// POST /advance/cancel — SIGTERM the in-flight rpc-shreds child.
    Cancel {
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum CheckpointAction {
    /// Save a new checkpoint with an optional label.
    Save {
        #[arg(long)]
        label: Option<String>,
    },
    /// List saved checkpoints.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Restore a saved checkpoint by id.
    ///
    /// Returns as soon as the orchestrator accepts the restore request
    /// (202 Accepted); the validator is then dropped, the snapshot is
    /// remounted, and the fork comes back up. Poll
    /// `k256-replay status` until `phase=ready` and `rpc=listening`
    /// before issuing follow-up calls — typical window is 30-120s.
    Restore {
        /// Checkpoint id from `checkpoint list`.
        id: String,
    },
    /// Delete one or more checkpoints by id, age, or "keep latest N".
    ///
    /// Frees host disk space when /data is getting full. The validator
    /// stays running; only the on-disk checkpoint files go away. Use
    /// `--keep-latest N` to retain the most recent N checkpoints
    /// (typical: `--keep-latest 1 --yes` to wipe everything except the
    /// freshest rewind point). At least one of `--id`, `--older-than`,
    /// or `--keep-latest` must be supplied; combining filters AND's
    /// them (a checkpoint must match every supplied filter to be
    /// deleted).
    Prune {
        /// Delete a specific checkpoint id. Repeatable; mutually
        /// exclusive with `--older-than` / `--keep-latest`.
        #[arg(long, conflicts_with_all = ["older_than", "keep_latest"])]
        id: Vec<String>,
        /// Delete checkpoints older than this duration. Format: `1h`,
        /// `45m`, `7d` (h/m/d only — no calendar arithmetic).
        #[arg(long, value_name = "DURATION")]
        older_than: Option<String>,
        /// Keep the N most-recently-created checkpoints; delete the
        /// rest. Combine with `--older-than` to delete old + cap count.
        #[arg(long, value_name = "N")]
        keep_latest: Option<usize>,
        /// Print what would be deleted; do not actually delete.
        #[arg(long)]
        dry_run: bool,
        /// Bypass the confirmation prompt. Required when not dry-run.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum PatchAction {
    /// POST /accounts/patch — apply one or more account patches.
    ///
    /// `--mode merge` requires the account to ALREADY EXIST on the
    /// fork; the validator returns `400 invalid_params: account not
    /// found, use replace` otherwise. For a fresh keypair, use
    /// `--mode replace` and supply every field (lamports, owner,
    /// executable, rent_epoch, data_base64) — the missing ones
    /// default to zero/empty. See README → `patch apply` for the
    /// full mode tradeoff table.
    Apply {
        /// Pubkey to patch (mutually exclusive with `--from-json`).
        #[arg(long)]
        pubkey: Option<String>,
        /// Patch mode: `merge` (touch only the fields you set; account
        /// must already exist) or `replace` (every unset scalar resets
        /// to its zero default; creates the account if missing).
        #[arg(long, default_value = "merge", value_parser = ["merge", "replace"])]
        mode: String,
        /// Lamports — set to this value.
        #[arg(long)]
        lamports: Option<u64>,
        /// Owner — base58 program id.
        #[arg(long)]
        owner: Option<String>,
        /// Mark account `executable`.
        #[arg(long)]
        executable: Option<bool>,
        /// Rent epoch.
        #[arg(long)]
        rent_epoch: Option<u64>,
        /// Base64-encoded data payload (use `--data-file` for bytes on
        /// disk; the CLI base64-encodes for you).
        #[arg(long, conflicts_with = "data_file")]
        data_base64: Option<String>,
        /// Path to a raw bytes file; the CLI reads it and base64-encodes
        /// before sending.
        #[arg(long, value_name = "PATH")]
        data_file: Option<std::path::PathBuf>,
        /// Read a multi-patch request body from a JSON file (matches
        /// the orchestrator's `PatchRequestBody`). Mutually exclusive
        /// with single-patch flags.
        #[arg(
            long,
            value_name = "PATH",
            conflicts_with_all = ["pubkey", "lamports", "owner", "executable", "rent_epoch", "data_base64", "data_file"]
        )]
        from_json: Option<std::path::PathBuf>,
        /// Permit patches that cross an epoch boundary.
        #[arg(long)]
        allow_epoch_boundary: bool,
        /// Confirm a destructive operation. Required because patches
        /// mutate canonical mainnet state in the fork.
        #[arg(long)]
        yes: bool,
    },
    /// GET /accounts/patch/history — audit log of every patch.
    History {
        #[arg(long)]
        json: bool,
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Subcommand, Debug)]
pub enum DeployAction {
    /// POST /programs/deploy — deploy or finalize a Loader-v4 program.
    Program {
        /// Target program id. The validator writes the Loader-v4 account
        /// at this address (replacing whatever's there if you pass
        /// `--force-replace-legacy-loader` for legacy-loader programs).
        #[arg(long)]
        program_id: String,
        /// Path to the ELF; the CLI reads + base64-encodes. The
        /// orchestrator hard-caps the base64 payload at ~13 MiB.
        #[arg(long, value_name = "PATH", conflicts_with = "elf_base64")]
        elf: Option<std::path::PathBuf>,
        /// Pre-base64-encoded ELF. Prefer `--elf <PATH>`.
        #[arg(long, conflicts_with = "elf")]
        elf_base64: Option<String>,
        /// Loader-v4 status. `deployed` requires `--authority`;
        /// `finalized` requires `--next-version`.
        #[arg(long, default_value = "deployed", value_parser = ["deployed", "finalized"])]
        status: String,
        /// Required for `--status deployed`.
        #[arg(long = "authority")]
        authority_address: Option<String>,
        /// Required for `--status finalized`.
        #[arg(long = "next-version")]
        next_version_address: Option<String>,
        /// Allow overwriting an already-finalized program (destructive).
        #[arg(long)]
        force_replace_finalized: bool,
        /// Allow converting a legacy-loader-owned program account to
        /// Loader-v4 in place (destructive).
        #[arg(long)]
        force_replace_legacy_loader: bool,
        /// Confirm a destructive operation. Required.
        #[arg(long)]
        yes: bool,
    },
    /// GET /programs/deploy/history — audit log of every deploy.
    History {
        #[arg(long)]
        json: bool,
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Subcommand, Debug)]
pub enum FixturesAction {
    /// GET /fixtures/stats — bytes used / cap, fixture count, slot
    /// range, and the process-lifetime counters (evicted, oversized,
    /// cleared).
    Stats {
        #[arg(long)]
        json: bool,
    },
    /// POST /fixtures/clear — drop every captured fixture for the
    /// current session. Refuses without `--yes`.
    Clear {
        #[arg(long)]
        yes: bool,
    },
    /// Sample N signatures from the captured slot range so you have
    /// something to feed into `k256-replay diff <SIG>`. Walks the most
    /// recent captured slots via Solana RPC (`--rpc-url`, default
    /// `$FORK_RPC` then `http://127.0.0.1:8899`), probes a handful of
    /// signatures per slot through `/fixtures/tx`, and returns the
    /// ones that hit the cache. The orchestrator has no list endpoint
    /// — this command is the no-server-changes glue.
    Sample {
        /// How many signatures to return. Stops scanning once it has
        /// this many hits.
        #[arg(short = 'n', long, default_value_t = 5)]
        n: usize,
        /// Solana JSON-RPC against the fork. Defaults to `$FORK_RPC`
        /// then `http://127.0.0.1:8899`. The orchestrator can't help
        /// here — getBlock is Solana-side, not orch-side.
        #[arg(long, value_name = "URL")]
        rpc_url: Option<String>,
        /// Print one signature per line (machine-friendly). Default
        /// is a human-readable table.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum PluginAction {
    /// GET /plugin/list — uploaded plugins + which one is loaded.
    List {
        #[arg(long)]
        json: bool,
    },
    /// POST /plugin/upload — multipart upload of `.so` + `config.json`.
    /// On success prints the path you should pass as `geyser` on the
    /// next `/boot`.
    Upload {
        /// Path to the compiled `.so`. Must be x86_64-unknown-linux-gnu,
        /// glibc 2.39, and ABI-matched to the running validator.
        #[arg(long, value_name = "PATH")]
        lib: std::path::PathBuf,
        /// Path to the plugin's `config.json`. The orchestrator
        /// rewrites the `libpath` field to where the .so was saved.
        #[arg(long, value_name = "PATH")]
        config: std::path::PathBuf,
    },
    /// POST /plugin/delete — remove one uploaded plugin by id.
    Delete {
        #[arg(long)]
        id: String,
        #[arg(long)]
        yes: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Subcommands that already printed a tailored message
            // (e.g. `replay diff` on 404) wrap the cause in PresentedExit
            // so main doesn't double-print.
            if let Some(p) = e.downcast_ref::<PresentedExit>() {
                return ExitCode::from(p.code);
            }
            eprintln!("error: {e:#}");
            // Distinct exit codes help bash callers branch on auth vs
            // server vs malformed input without parsing the message.
            ExitCode::from(exit_code_for(&e))
        }
    }
}

fn exit_code_for(e: &anyhow::Error) -> u8 {
    if let Some(api) = e.downcast_ref::<client::ApiError>() {
        return api_status_to_code(api.status.as_u16());
    }
    1
}

/// Stable mapping of HTTP status → CLI exit code, documented in README.
pub fn api_status_to_code(status: u16) -> u8 {
    match status {
        401 => 2,
        403 => 3,
        404 => 4,
        409 => 5,
        400 => 6,
        500..=599 => 7,
        _ => 1,
    }
}

/// Signal to `main` that the error has already been rendered to stderr
/// and only the exit code matters. Used by subcommands that print a
/// tailored explanation (e.g. `diff` on `fixture_not_found`).
#[derive(Debug)]
pub struct PresentedExit {
    pub code: u8,
}

impl std::fmt::Display for PresentedExit {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

impl std::error::Error for PresentedExit {}

fn run(cli: Cli) -> Result<()> {
    // `completion` doesn't need an API key — route it before we demand
    // REPLAY_API_KEY so users can scaffold their shell on a fresh
    // machine.
    if let Cmd::Completion { shell } = cli.cmd {
        let mut app = Cli::command();
        let bin = app.get_name().to_string();
        generate(shell, &mut app, bin, &mut std::io::stdout());
        std::io::stdout().flush().ok();
        return Ok(());
    }

    let colour = !cli.no_color && output::use_colour();

    // `idl` is a public gateway route (mainnet-backed, edge-cached) —
    // no bearer required upstream. Let the CLI mirror that so anyone
    // can `k256-replay idl <prog>` without provisioning a Replay key.
    // We pass an empty bearer; reqwest will send "Authorization:
    // Bearer ", which the gateway ignores for `/idl/`.
    if let Cmd::Idl { program_id, json } = &cli.cmd {
        let key = cli.key.clone().unwrap_or_default();
        let client = ReplayClient::new(cli.endpoint.clone(), key)?;
        return cmd::idl::run(&client, program_id, *json, colour);
    }

    // `doctor` runs WITHOUT an API key when none is configured — its
    // purpose is to diagnose a half-configured environment, including
    // "you forgot to set REPLAY_API_KEY". We hand it whatever key the
    // user has (possibly empty); doctor reports auth as one of its
    // checks.
    if matches!(cli.cmd, Cmd::Doctor) {
        let key = cli.key.clone().unwrap_or_default();
        let client = ReplayClient::new(cli.endpoint.clone(), key.clone())?;
        return cmd::doctor::run(&client, cli.endpoint.as_str(), &key, colour);
    }

    let key = cli
        .key
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .context("REPLAY_API_KEY is unset and --key was not provided")?;
    let client = ReplayClient::new(cli.endpoint.clone(), key)?;

    match cli.cmd {
        Cmd::Status { watch, json } => cmd::status::run(&client, watch, json, colour),
        Cmd::Diff { signature, json, bytes } => {
            cmd::diff::run(&client, &signature, json, bytes, colour)
        }
        Cmd::Advance { action } => cmd::advance::run(&client, action, colour),
        Cmd::Checkpoint { action } => cmd::checkpoint::run(&client, action, colour),
        Cmd::Patch { action } => cmd::patch::run(&client, action, colour),
        Cmd::Deploy { action } => cmd::deploy::run(&client, action, colour),
        Cmd::Plugin { action } => cmd::plugin::run(&client, action, colour),
        Cmd::Kill { yes } => cmd::kill::run(&client, yes, colour),
        Cmd::Clear { yes } => cmd::clear::run(&client, yes, colour),
        Cmd::Fixtures { action } => cmd::fixtures::run(&client, action, colour),
        Cmd::Logs { lines, follow } => cmd::logs::run(&client, lines, follow, colour),
        Cmd::Snapshots {
            dashboard,
            limit,
            json,
        } => cmd::snapshots::run(&client, &dashboard, limit, json, colour),
        // Handled above (key-less routes).
        Cmd::Idl { .. } | Cmd::Completion { .. } | Cmd::Doctor => unreachable!(),
    }
}
