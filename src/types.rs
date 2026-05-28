//! Wire types for the k256 Replay orchestrator + gateway.
//!
//! Mirrors the JSON shapes returned by
//! `k256-replay-app/k256/replay-orchestrator/src/handlers/*.rs`. Every
//! u64-valued field on the orchestrator side serializes as a **decimal
//! string** (`"422097261"`) so JS clients keep precision; the CLI
//! follows the same discipline and only parses to `u64` at the
//! rendering edge.
//!
//! `Serialize` is derived alongside `Deserialize` so the `--json`
//! output paths (`diff --json`, `status --json`, `advance status --json`)
//! can round-trip the wire fixture verbatim without a hand-written
//! wrapper. Adding a struct field is therefore a single-line change:
//! declare it, then render it where it matters.

use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────
// /status
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct Status {
    pub phase: String,
    pub current_slot: Option<u64>,
    pub snapshot_slot: Option<u64>,
    pub orchestrator_version: Option<String>,
    pub validator_version: Option<String>,
    pub rpc_listening: Option<bool>,
    pub geyser_plugin: Option<String>,
    pub advance: Option<AdvanceStateSummary>,
    pub mutation: Option<MutationState>,
    pub uptime_secs: Option<u64>,
    pub shred_version: Option<u64>,
    pub snapshot_file: Option<String>,
    pub last_error: Option<String>,
    /// Host disk gauge sourced from fc-agent's `/status`. Surfaces
    /// `pct_used`, `free_bytes`, `total_bytes`, `refuse_new_checkpoint`,
    /// and a `warn` boolean so the CLI's `status` view can render a
    /// red/yellow band when /data is near full BEFORE writes start
    /// failing with ENOSPC. `None` when fc-agent isn't reachable or
    /// the orchestrator binary predates this field — fall back to "no
    /// disk info" rather than guessing.
    #[serde(default)]
    pub disk: Option<DiskUsage>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiskUsage {
    pub path: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub pct_used: f64,
    pub checkpoint_budget_bytes: u64,
    pub refuse_new_checkpoint: bool,
    pub warn: bool,
}

/// Embedded form of `AdvanceJob` inside `/status.advance`. Wire field
/// is `id` (not `job_id`); the previous `job_id: Option<String>` in
/// this struct silently parsed every status as having no job id.
#[derive(Debug, Deserialize, Serialize)]
pub struct AdvanceStateSummary {
    pub id: Option<String>,
    pub status: Option<String>,
    pub target_slot: Option<u64>,
    pub current_slot: Option<u64>,
    pub start_slot: Option<u64>,
    pub error: Option<String>,
    pub percent: Option<f32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MutationState {
    pub dirty: Option<bool>,
    pub last_mutation_slot: Option<u64>,
    pub last_mutation_kind: Option<String>,
    pub mutation_count: Option<u64>,
}

// ─────────────────────────────────────────────────────────────────────
// /advance
// ─────────────────────────────────────────────────────────────────────

/// Response from `POST /advance` (HTTP 202). Schema is the
/// orchestrator's `AdvanceAccepted` struct.
#[derive(Debug, Deserialize, Serialize)]
pub struct AdvanceAccepted {
    pub job_id: String,
    pub start_slot: u64,
    pub target_slot: u64,
    pub status: String,
}

/// Full job descriptor — `GET /advance` returns `{ "job": AdvanceJob | null }`,
/// `GET /advance/history` returns `{ "jobs": [AdvanceJob...] }`.
#[derive(Debug, Deserialize, Serialize)]
pub struct AdvanceJob {
    pub id: String,
    pub start_slot: u64,
    pub target_slot: u64,
    pub current_slot: Option<u64>,
    pub rpc_url: String,
    pub root: bool,
    pub status: String,
    pub error: Option<String>,
    pub exit_code: Option<i32>,
    pub started_at_unix: u64,
    pub finished_at_unix: Option<u64>,
    pub percent: f32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AdvanceLatest {
    pub job: Option<AdvanceJob>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AdvanceHistory {
    pub jobs: Vec<AdvanceJob>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AdvanceCancelResp {
    pub status: String,
    pub job: Option<AdvanceJob>,
}

// ─────────────────────────────────────────────────────────────────────
// /kill
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct KillResponse {
    pub phase: String,
}

// ─────────────────────────────────────────────────────────────────────
// /fixtures/*
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct FixtureStats {
    pub fixture_count: u64,
    pub slot_count: u64,
    pub oldest_slot: Option<String>,
    pub newest_slot: Option<String>,
    pub bytes_used: u64,
    pub bytes_cap: u64,
    pub evicted_count: u64,
    pub oversized_count: u64,
    pub cleared_count: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ClearResult {
    pub cleared: bool,
    pub fixtures_removed: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Fixture {
    pub schema_version: u32,
    pub signature: String,
    pub slot: String,
    pub captured_at_unix_seconds: String,
    pub source: String,
    pub execution: Execution,
    pub transaction: TxMeta,
    pub accounts: Vec<AccountDiff>,
    pub cache: CacheMeta,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Execution {
    pub state_status: String, // "complete" | "fees_only_partial" | "not_executed"
    pub result: String,       // "success" | "executed_error" | "load_error"
    pub error: Option<String>,
    pub fee_lamports: Option<String>,
    pub compute_units_consumed: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TxMeta {
    pub signatures: Vec<String>,
    pub recent_blockhash: String,
    pub account_keys: Vec<String>,
    pub writable_indexes: Vec<u32>,
    pub readonly_indexes: Vec<u32>,
    pub signer_indexes: Vec<u32>,
    pub fee_payer_index: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AccountDiff {
    pub index: u32,
    pub pubkey: String,
    pub declared_writable: bool,
    pub signer: bool,
    pub fee_payer: bool,
    pub invoked: bool,
    pub instruction_account: bool,
    pub pre: Option<AccountSnapshot>,
    pub post: Option<AccountSnapshot>,
    /// `Some(true)` changed, `Some(false)` unchanged, `None` unknown
    /// (one side too large for sha256 hashing).
    pub changed: Option<bool>,
    pub changed_fields: Vec<String>,
    pub data_comparison: String,
    pub data: AccountData,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AccountSnapshot {
    pub lamports: String,
    pub owner: String,
    pub executable: bool,
    pub rent_epoch: String,
    pub data_len: u64,
    pub data_sha256: Option<String>,
    pub data_hash_omitted_reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AccountData {
    pub pre_base64: Option<String>,
    pub post_base64: Option<String>,
    pub omitted_reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CacheMeta {
    pub data_policy: String,
    pub inline_account_data_max_bytes: u64,
    pub fixture_truncated: bool,
}

// ─────────────────────────────────────────────────────────────────────
// /checkpoints
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct CheckpointSaveAck {
    pub accepted: bool,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Checkpoint {
    pub id: String,
    pub label: Option<String>,
    pub slot: Option<u64>,
    /// Unix timestamp as a decimal string (matches the orchestrator's
    /// u64-as-string convention). `None` is tolerated for forward-
    /// compat but the live wire shape always sets it.
    pub created_at: Option<String>,
}

impl Checkpoint {
    /// Parse `created_at` back to a Unix-seconds `u64`. `None` when the
    /// field is absent or the string isn't a decimal integer. Used by
    /// `checkpoint prune` to filter / sort by age.
    pub fn created_at_unix(&self) -> Option<u64> {
        self.created_at.as_deref().and_then(|s| s.parse().ok())
    }
}

// `GET /checkpoints` returns either `{ checkpoints: [...] }` or a bare
// array depending on orchestrator build. Tolerate both.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CheckpointList {
    Wrapped { checkpoints: Vec<Checkpoint> },
    Bare(Vec<Checkpoint>),
}

impl CheckpointList {
    pub fn into_vec(self) -> Vec<Checkpoint> {
        match self {
            CheckpointList::Wrapped { checkpoints } => checkpoints,
            CheckpointList::Bare(v) => v,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// /accounts/patch
// ─────────────────────────────────────────────────────────────────────

/// POST body for `/accounts/patch`. Mirrors `PatchRequestBody` in the
/// orchestrator. `confirm_dangerous` must be `true` (the orchestrator
/// returns `400 confirm_dangerous_required` otherwise — the CLI sets
/// it to true on the caller's behalf when they pass `--yes`).
///
/// `Deserialize` is derived so `cmd::patch::apply --from-json <PATH>`
/// can round-trip the orchestrator's exact shape directly off disk.
#[derive(Debug, Deserialize, Serialize)]
pub struct PatchRequest {
    pub patches: Vec<PatchEntry>,
    pub confirm_dangerous: bool,
    pub allow_epoch_boundary: bool,
}

/// One account patch. `mode` is `"merge"` (only the fields the operator
/// sets are touched) or `"replace"` (every unset scalar/data resets to
/// its default — lamports=0, data=empty). The orchestrator returns
/// `400 bad_mode` for anything else.
#[derive(Debug, Deserialize, Serialize)]
pub struct PatchEntry {
    pub pubkey: String,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lamports: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rent_epoch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
}

/// `GET /accounts/patch/history` and `/programs/deploy/history` both
/// return `{ "records": [arbitrary JSON objects] }`. We keep the rows
/// as `serde_json::Value` because the audit shape is informational
/// (different across mutation kinds) and the CLI just prints the
/// salient fields. Operators wanting the raw shape use `--json`.
#[derive(Debug, Deserialize)]
pub struct MutationHistory {
    pub records: Vec<serde_json::Value>,
}

// ─────────────────────────────────────────────────────────────────────
// /programs/deploy
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DeployRequest {
    pub program_id: String,
    pub elf_base64: String,
    /// `"deployed"` or `"finalized"`. `deployed` requires `authority_address`;
    /// `finalized` requires `next_version_address`.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_version_address: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub force_replace_finalized: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub force_replace_legacy_loader: bool,
    pub confirm_dangerous: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeployResponse {
    pub program_id: String,
    pub operation: String,
    pub loader: String,
    pub base_slot: u64,
    pub materialized_slot: u64,
    pub synthetic_deployment_slot: u64,
    pub effective_slot: u64,
    pub status: String,
    pub authority_address: Option<String>,
    pub next_version_address: Option<String>,
    pub elf_len: usize,
    pub elf_sha256: String,
    pub account_sha256_before: Option<String>,
    pub account_sha256_after: String,
    pub cache_published: bool,
}

// ─────────────────────────────────────────────────────────────────────
// /plugin/*
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct PluginUploadResponse {
    pub id: String,
    pub config_path: String,
    pub lib_path: String,
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PluginEntry {
    pub id: String,
    pub config_path: String,
    pub lib_path: String,
    pub name: String,
    pub uploaded_unix: u64,
    pub size_bytes: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PluginActive {
    pub kind: String, // "bundled" | "uploaded" | "custom"
    pub config_path: String,
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PluginList {
    pub plugins: Vec<PluginEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<PluginActive>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PluginDeleteResp {
    /// The orchestrator's response shape is `{ "deleted": bool }` (and
    /// optional `message` on error). Kept lenient so the CLI prints
    /// "deleted" or surfaces the orchestrator's error message verbatim.
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub message: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────
// /logs/tail
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct LogsTail {
    pub lines: Vec<String>,
}

// ─────────────────────────────────────────────────────────────────────
// Catalog (dashboard /api/snapshots) — used by `replay snapshots`
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct SnapshotsResponse {
    pub items: Vec<Snapshot>,
    pub count: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Snapshot {
    pub id: String,
    pub cluster: String,
    pub agave_version: String,
    pub slot: u64,
    pub size_bytes: u64,
    pub bank_hash_short: Option<String>,
    pub status: String,
}
