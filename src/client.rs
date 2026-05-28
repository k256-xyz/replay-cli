//! Thin REST client wrapping the gateway (or a direct orchestrator).
//!
//! Surfaces just the verbs the CLI uses. Every method returns the live
//! body parsed into a typed struct so callers don't pay the serde dance
//! twice. Error mapping is intentionally lossy — for the CLI we only
//! care about the orchestrator's `{error, message}` payload, not the
//! full reqwest error tree.

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::blocking::{multipart, Client, RequestBuilder, Response};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;

use crate::types::{
    AdvanceAccepted, AdvanceCancelResp, AdvanceHistory, AdvanceJob, AdvanceLatest,
    CheckpointList, CheckpointSaveAck, ClearResult, DeployRequest, DeployResponse, Fixture,
    FixtureStats, KillResponse, LogsTail, MutationHistory, PatchRequest, PluginDeleteResp,
    PluginList, PluginUploadResponse, SnapshotsResponse, Status,
};

/// Gateway. Every customer reaches their box through here; it routes
/// by bearer to the box that owns the key. The orchestrator port on
/// the box itself is not a customer-facing surface.
pub const DEFAULT_ENDPOINT: &str = "https://api-replay.k256.xyz";

/// Default dashboard origin for the snapshot catalog. Distinct from the
/// gateway because the catalog is dashboard-side (WorkOS session) per
/// the architecture rules.
pub const DEFAULT_DASHBOARD: &str = "https://app.k256.xyz";

#[derive(Debug, Clone)]
pub struct ReplayClient {
    base: String,
    bearer: String,
    http: Client,
    /// Long-lived `Client` for streaming endpoints (`/logs/stream`).
    /// Set to no timeout so `replay logs --follow` doesn't drop the
    /// SSE connection after 30 s.
    streaming_http: Client,
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: Option<String>,
    pub message: Option<String>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 401 with a recognised bearer-reject code gets a tailored hint
        // instead of the raw JSON envelope. Codex flagged the previous
        // output (`401 invalid_token: {"error":"invalid_token"}`) as
        // unfriendly; this trades the JSON dump for a clear next step.
        if self.status.as_u16() == 401 {
            let kind = self.code.as_deref().unwrap_or("unauthenticated");
            match kind {
                "invalid_token" | "unauthenticated" => {
                    return write!(
                        f,
                        "401 — the gateway rejected your bearer. Check \
                         REPLAY_API_KEY is the value shown on the Access \
                         page (https://app.k256.xyz/app/replay/<server-id>/access)."
                    );
                }
                _ => {}
            }
        }
        match (&self.code, &self.message) {
            (Some(c), Some(m)) => write!(f, "{} {}: {}", self.status.as_u16(), c, m),
            (Some(c), None) => write!(f, "{} {}", self.status.as_u16(), c),
            (None, Some(m)) => write!(f, "{} {}", self.status.as_u16(), m),
            (None, None) => write!(f, "{}", self.status),
        }
    }
}

impl std::error::Error for ApiError {}

impl ReplayClient {
    pub fn new(base: impl Into<String>, bearer: impl Into<String>) -> Result<Self> {
        // 30 s covers `/advance` which can take ~25 s on a cold box.
        // Faster requests (status, fixtures/*) finish in well under a
        // second.
        let http = Client::builder()
            .user_agent(concat!("k256-replay-cli/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()
            .context("building HTTP client")?;
        // No timeout on the streaming client; `logs --follow` holds
        // the connection open indefinitely. The blocking response is
        // consumed line-by-line; reading EOF or Ctrl-C ends it.
        let streaming_http = Client::builder()
            .user_agent(concat!("k256-replay-cli/", env!("CARGO_PKG_VERSION"), "/stream"))
            .timeout(None)
            .build()
            .context("building streaming HTTP client")?;
        Ok(Self {
            base: base.into().trim_end_matches('/').to_string(),
            bearer: bearer.into(),
            http,
            streaming_http,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn auth(&self, b: RequestBuilder) -> RequestBuilder {
        b.header("authorization", format!("Bearer {}", self.bearer))
    }

    fn send_json<T: for<'de> Deserialize<'de>>(&self, b: RequestBuilder) -> Result<T> {
        let resp = self.auth(b).send().context("sending request")?;
        handle::<T>(resp)
    }

    // ── /status ──────────────────────────────────────────────────────

    pub fn status(&self) -> Result<Status> {
        self.send_json(self.http.get(self.url("/status")))
    }

    /// Raw `/status` body as `serde_json::Value`. Used by `doctor`
    /// which wants to read forward-compatible fields (e.g. `disk`,
    /// future telemetry) without growing the `Status` struct for every
    /// new diagnostic surface.
    pub fn status_json(&self) -> Result<serde_json::Value> {
        self.send_json(self.http.get(self.url("/status")))
    }

    // ── /kill ────────────────────────────────────────────────────────

    pub fn kill(&self) -> Result<KillResponse> {
        self.send_json(self.http.post(self.url("/kill")))
    }

    // ── /advance ─────────────────────────────────────────────────────

    pub fn advance_start(&self, target_slot: u64) -> Result<AdvanceAccepted> {
        let body = serde_json::json!({ "target_slot": target_slot });
        self.send_json(self.http.post(self.url("/advance")).json(&body))
    }

    /// `GET /advance` — current active job, or most recent if idle.
    /// Returns `None` when the orchestrator wrapped `null` (fresh box,
    /// no advance ever run).
    pub fn advance_latest(&self) -> Result<Option<AdvanceJob>> {
        let resp: AdvanceLatest = self.send_json(self.http.get(self.url("/advance")))?;
        Ok(resp.job)
    }

    pub fn advance_history(&self) -> Result<Vec<AdvanceJob>> {
        let resp: AdvanceHistory = self.send_json(self.http.get(self.url("/advance/history")))?;
        Ok(resp.jobs)
    }

    pub fn advance_cancel(&self) -> Result<AdvanceCancelResp> {
        self.send_json(self.http.post(self.url("/advance/cancel")))
    }

    // ── /fixtures/* (Tx workbench) ───────────────────────────────────

    pub fn fixture_stats(&self) -> Result<FixtureStats> {
        self.send_json(self.http.get(self.url("/fixtures/stats")))
    }

    pub fn fixture_get(&self, signature: &str) -> Result<Fixture> {
        // Signatures are base58: alnum without 0/O/I/l, so no URL escape
        // is ever needed. `cmd::diff` rejects non-base58 input before
        // it reaches the client.
        self.send_json(self.http.get(self.url(&format!("/fixtures/tx/{signature}"))))
    }

    pub fn fixture_clear(&self) -> Result<ClearResult> {
        let body = serde_json::json!({ "confirm": true });
        self.send_json(self.http.post(self.url("/fixtures/clear")).json(&body))
    }

    // ── /checkpoint(s) / /restore ────────────────────────────────────

    pub fn checkpoints(&self) -> Result<CheckpointList> {
        self.send_json(self.http.get(self.url("/checkpoints")))
    }

    pub fn checkpoint_save(&self, label: Option<&str>) -> Result<CheckpointSaveAck> {
        let body = label
            .map(|l| serde_json::json!({ "label": l }))
            .unwrap_or_else(|| serde_json::json!({}));
        self.send_json(self.http.post(self.url("/checkpoint")).json(&body))
    }

    /// `DELETE /checkpoints/<id>` — drop one checkpoint's on-disk files
    /// (mem + reflinked acc*.ext4 + data.ext4). Frees host disk space.
    /// The validator keeps running; only the snapshot record is removed.
    /// Server returns 202 (async) for in-flight deletion; the body
    /// echoes `{ id, removed: true }` on success.
    pub fn checkpoint_delete(&self, id: &str) -> Result<Value> {
        self.send_json(
            self.http
                .delete(self.url(&format!("/checkpoints/{id}"))),
        )
    }

    pub fn restore(&self, id: &str) -> Result<Value> {
        let body = serde_json::json!({ "id": id });
        self.send_json(self.http.post(self.url("/restore")).json(&body))
    }

    // ── /accounts/patch ──────────────────────────────────────────────

    pub fn patch_apply(&self, req: &PatchRequest) -> Result<Value> {
        // The orchestrator forwards the validator's response JSON
        // verbatim; we keep it as `Value` so we don't have to track
        // every validator-side wire-field rename. The CLI's renderer
        // prints the summary fields it knows about.
        self.send_json(self.http.post(self.url("/accounts/patch")).json(req))
    }

    pub fn patch_history(&self) -> Result<Vec<Value>> {
        let h: MutationHistory =
            self.send_json(self.http.get(self.url("/accounts/patch/history")))?;
        Ok(h.records)
    }

    // ── /programs/deploy ─────────────────────────────────────────────

    pub fn deploy(&self, req: &DeployRequest) -> Result<DeployResponse> {
        self.send_json(self.http.post(self.url("/programs/deploy")).json(req))
    }

    pub fn deploy_history(&self) -> Result<Vec<Value>> {
        let h: MutationHistory =
            self.send_json(self.http.get(self.url("/programs/deploy/history")))?;
        Ok(h.records)
    }

    // ── /plugin/* ────────────────────────────────────────────────────

    pub fn plugin_list(&self) -> Result<PluginList> {
        self.send_json(self.http.get(self.url("/plugin/list")))
    }

    pub fn plugin_upload(
        &self,
        lib_path: &Path,
        config_path: &Path,
    ) -> Result<PluginUploadResponse> {
        let form = multipart::Form::new()
            .file("lib", lib_path)
            .with_context(|| format!("opening {}", lib_path.display()))?
            .file("config", config_path)
            .with_context(|| format!("opening {}", config_path.display()))?;
        // Plugin uploads can be 5-50 MiB; the default 30 s timeout
        // works on a fast LAN but is tight on residential uplinks.
        // Use the streaming client (no timeout) for the upload.
        let req = self
            .streaming_http
            .post(self.url("/plugin/upload"))
            .multipart(form);
        self.send_json(req)
    }

    pub fn plugin_delete(&self, id: &str) -> Result<PluginDeleteResp> {
        let body = serde_json::json!({ "id": id });
        self.send_json(self.http.post(self.url("/plugin/delete")).json(&body))
    }

    // ── /logs/{tail,stream} ──────────────────────────────────────────

    pub fn logs_tail(&self, lines: Option<u32>) -> Result<LogsTail> {
        let mut url = self.url("/logs/tail");
        if let Some(n) = lines {
            url.push_str(&format!("?lines={n}"));
        }
        self.send_json(self.http.get(url))
    }

    /// Open `/logs/stream` as a Server-Sent-Events connection. The
    /// caller is expected to read line-by-line and look for the
    /// `data: <text>` prefix; control records (`event: keepalive`,
    /// empty lines) are filtered there. The returned `Response`
    /// implements `Read`; closing it closes the stream.
    pub fn logs_stream(&self) -> Result<Response> {
        let req = self
            .streaming_http
            .get(self.url("/logs/stream"))
            .header("accept", "text/event-stream");
        let resp = self
            .auth(req)
            .send()
            .context("opening /logs/stream")?;
        if !resp.status().is_success() {
            // Same error envelope as the rest of the surface; we
            // peek the body here because the caller would lose access
            // to it once they start streaming.
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(parse_api_error(status, &body));
        }
        Ok(resp)
    }

    // ── Public on-chain IDL fetch ────────────────────────────────────
    //
    // `GET /idl/<programId>` resolves the program's Anchor IDL account
    // on mainnet, decompresses the JSON, and returns
    // `{ program_id, idl_address, idl }`. No bearer required upstream
    // (the gateway forwards to its own configured RPC), but we still
    // send Authorization so the same client wiring covers every route.
    // 404 means the program does not publish an Anchor IDL (System,
    // Token, native loaders, Phoenix, …).
    pub fn idl(&self, program_id: &str) -> Result<Value> {
        self.send_json(self.http.get(self.url(&format!("/idl/{program_id}"))))
    }

    // ── Dashboard catalog (separate origin) ──────────────────────────
    //
    // The catalog is hosted by the dashboard under WorkOS session auth,
    // not by the gateway. We call it without the bearer; the user must
    // be logged into the dashboard for the cookie to fly. We use raw
    // reqwest here to avoid threading a second bearer.

    pub fn snapshots(&self, dashboard_url: &str) -> Result<SnapshotsResponse> {
        let url = format!("{}/api/snapshots", dashboard_url.trim_end_matches('/'));
        let resp = self.http.get(&url).send().context("fetching catalog")?;
        handle::<SnapshotsResponse>(resp)
    }
}

fn handle<T: for<'de> Deserialize<'de>>(resp: Response) -> Result<T> {
    let status = resp.status();
    let body = resp.text().unwrap_or_default();
    if status.is_success() {
        return serde_json::from_str(&body)
            .with_context(|| format!("decoding {} response: {}", status, truncate(&body, 200)));
    }
    Err(parse_api_error(status, &body))
}

fn parse_api_error(status: StatusCode, body: &str) -> anyhow::Error {
    // Try the orchestrator error envelope { error, message }. Both fields
    // optional; some gateway-level errors are bare strings or HTML.
    #[derive(Deserialize)]
    struct Env {
        error: Option<String>,
        message: Option<String>,
    }
    let env: Env = serde_json::from_str(body).unwrap_or(Env {
        error: None,
        message: None,
    });
    let fallback_message = match (env.message, body.trim_start()) {
        (Some(m), _) => Some(m),
        (None, "") => None,
        // Cloudflare and many origin-error responses are HTML pages. Don't
        // dump the markup; surface a status-specific hint so the operator
        // knows whether to retry, wait for a restart window, or escalate.
        (None, b) if b.starts_with('<') => Some(cf_5xx_hint(status.as_u16())),
        (None, b) => Some(truncate(b, 200).to_string()),
    };
    anyhow!(ApiError {
        status,
        code: env.error,
        message: fallback_message,
    })
}

/// Map a Cloudflare-style 5xx (HTML-bodied) to a one-line operator hint.
/// Cloudflare's error codes carry meaningful retry / escalation signal
/// — surface that instead of "non-JSON error body".
///
/// Note: the orchestrator now classifies admin-RPC errors and returns
/// 4xx for user errors (invalid_params → 400) so most of these hints
/// only fire for real infrastructure trouble — origin unreachable,
/// timeouts, panics. Cloudflare's edge can still swap in an HTML body
/// when an upstream returns a real 5xx, which is why we keep the
/// hints below.
fn cf_5xx_hint(code: u16) -> String {
    match code {
        502 => "Cloudflare reports the orchestrator returned a bad response. \
                Retry once; if it persists, run `k256-replay status` — the fork \
                may be mid-restart or the orchestrator may have crashed."
            .to_string(),
        520 => "Cloudflare 520 — the orchestrator closed the connection unexpectedly. \
                Retry; if persistent, check `k256-replay status` and `k256-replay logs -n 200`."
            .to_string(),
        521 => "Cloudflare 521 — the box is down or unreachable. \
                Wait 30s and retry; the fork may be booting / restarting."
            .to_string(),
        522 => "Cloudflare 522 — connection to the box timed out. \
                Common during `checkpoint save` / `checkpoint restore` windows while fc-agent \
                pauses or restarts the guest. Wait 30-90s and retry."
            .to_string(),
        523 => "Cloudflare 523 — origin unreachable. \
                Common during `checkpoint save` / `checkpoint restore` (fc-agent \
                pauses the guest for up to ~120s while it snapshots / restores), \
                AND during a fresh boot's initial admin-RPC warm-up. If it \
                persists past ~3 minutes with no other operation in flight, \
                check the dashboard."
            .to_string(),
        524 => "Cloudflare 524 — timed out waiting for the orchestrator to respond. \
                The operation MAY still be running on the box; check `k256-replay status` \
                or the relevant history (`advance history`, `patch history`, `deploy history`)."
            .to_string(),
        _ => format!(
            "non-JSON {code} body (gateway or origin issue — retry once, then run `k256-replay status`)"
        ),
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}
