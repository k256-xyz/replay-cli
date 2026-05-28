//! `k256-replay fixtures <stats|clear>`
//!
//! Lightweight cache-management commands that mirror the orchestrator's
//! `/fixtures/*` surface:
//!
//!   - `fixtures stats`   → `GET  /fixtures/stats` — used by scripts
//!     watching cache pressure; the same numbers appear inline in
//!     `k256-replay status`, but `stats` exposes them as a focused
//!     command for cron / monitoring loops.
//!   - `fixtures clear`   → `POST /fixtures/clear` — destructive; drops
//!     every captured fixture for the current session.
//!     `k256-replay clear` (top-level) is the legacy alias; both go to
//!     the same endpoint so existing scripts keep working.
//!
//! Reads support `--json` for raw piping.

use anyhow::{anyhow, Context, Result};
use humansize::{format_size, DECIMAL};
use owo_colors::OwoColorize;
use serde_json::{json, Value};

use crate::client::ReplayClient;
use crate::output::fmt_stats_line;
use crate::FixturesAction;

pub fn run(client: &ReplayClient, action: FixturesAction, colour: bool) -> Result<()> {
    match action {
        FixturesAction::Stats { json } => stats(client, json, colour),
        FixturesAction::Clear { yes } => clear(client, yes, colour),
        FixturesAction::Sample { n, rpc_url, json } => sample(client, n, rpc_url, json, colour),
    }
}

fn stats(client: &ReplayClient, json: bool, colour: bool) -> Result<()> {
    let s = client.fixture_stats()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }

    // Rendered view: header line + the same compact cache stat line the
    // `status` command shows, plus the process-lifetime counters in
    // their own block (always shown here, even when zero, so monitoring
    // sees a stable schema).
    println!("{}", fmt_stats_line(&s, colour));

    let bytes = format_size(s.bytes_used, DECIMAL);
    let cap = format_size(s.bytes_cap, DECIMAL);
    let pct = if s.bytes_cap == 0 {
        0.0
    } else {
        (s.bytes_used as f64 / s.bytes_cap as f64) * 100.0
    };
    let dim = |s: &str| {
        if colour {
            s.dimmed().to_string()
        } else {
            s.to_string()
        }
    };
    println!();
    println!("  {:<20} {}", dim("bytes_used"), bytes);
    println!("  {:<20} {}", dim("bytes_cap"),  cap);
    println!("  {:<20} {:.2}%", dim("fill"),     pct);
    println!("  {:<20} {}", dim("fixture_count"), s.fixture_count);
    println!("  {:<20} {}", dim("slot_count"),    s.slot_count);
    if let (Some(o), Some(n)) = (s.oldest_slot.as_deref(), s.newest_slot.as_deref()) {
        println!("  {:<20} {}..{}", dim("slot_range"), o, n);
    }
    println!("  {:<20} {}", dim("evicted_count"),   s.evicted_count);
    println!("  {:<20} {}", dim("oversized_count"), s.oversized_count);
    println!("  {:<20} {}", dim("cleared_count"),   s.cleared_count);
    Ok(())
}

/// Walk the captured slot range backwards, ask the fork's Solana RPC
/// for the signatures in each slot, probe the orchestrator's
/// `/fixtures/tx/<sig>` for hits. Returns up to `n` signatures.
///
/// Rationale: the orchestrator's `/fixtures/stats` knows the slot
/// range but not the signatures themselves (admin RPC doesn't expose
/// a list endpoint yet). The Solana RPC's `getBlock` does know the
/// signatures, and `/fixtures/tx` confirms the cache hit. Two
/// endpoints, no server-side change.
fn sample(
    client: &ReplayClient,
    want: usize,
    rpc_url: Option<String>,
    json_out: bool,
    colour: bool,
) -> Result<()> {
    if want == 0 {
        return Err(anyhow!("-n must be >= 1"));
    }
    let stats = client.fixture_stats()?;
    if stats.fixture_count == 0 {
        return Err(anyhow!(
            "no fixtures in cache. Advance the fork first:\n  \
             k256-replay advance start --to <slot> --wait"
        ));
    }
    let newest = stats
        .newest_slot
        .as_deref()
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| {
            anyhow!("orchestrator returned an empty newest_slot even though fixture_count > 0")
        })?;
    let oldest = stats
        .oldest_slot
        .as_deref()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(newest);

    let rpc = rpc_url
        .or_else(|| std::env::var("FORK_RPC").ok())
        .unwrap_or_else(|| "http://127.0.0.1:8899".to_string());

    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("building http client")?;

    let dim = |s: &str| {
        if colour {
            s.dimmed().to_string()
        } else {
            s.to_string()
        }
    };

    let mut hits: Vec<(u64, String)> = Vec::with_capacity(want);
    let scan_depth = 20u64; // newest..newest-20, then drift to oldest
    let scan_start = newest;
    let scan_end = scan_start.saturating_sub(scan_depth).max(oldest);
    let mut slot = scan_start;
    let mut visited = 0u64;
    while slot >= scan_end && hits.len() < want {
        visited += 1;
        match get_block_signatures(&http, &rpc, slot) {
            Ok(sigs) if !sigs.is_empty() => {
                // Sample one every ~25 sigs to avoid hammering the
                // server when slots are large.
                let stride = if sigs.len() > 25 { 25 } else { 1 };
                for sig in sigs.iter().step_by(stride) {
                    if hits.len() >= want {
                        break;
                    }
                    if client.fixture_get(sig).is_ok() {
                        hits.push((slot, sig.clone()));
                    }
                }
            }
            Ok(_) => {
                if !json_out {
                    eprintln!("{}", dim(&format!("   slot {slot}: empty block")));
                }
            }
            Err(e) => {
                if !json_out {
                    eprintln!(
                        "{}",
                        dim(&format!("   slot {slot}: not visible yet ({e}) — skipping"))
                    );
                }
            }
        }
        if slot == 0 {
            break;
        }
        slot -= 1;
    }

    if hits.is_empty() {
        return Err(anyhow!(
            "scanned {visited} slot{} from newest_slot={newest} down to {scan_end} but found no cache hits. \
             The cache may be at a slot that RPC hasn't surfaced yet — wait a few seconds and retry.",
            if visited == 1 { "" } else { "s" }
        ));
    }

    if json_out {
        let arr: Vec<Value> = hits
            .iter()
            .map(|(s, sig)| json!({ "slot": s, "signature": sig }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }

    // Human-readable table. Compact: slot + signature, with a
    // copy-paste hint at the bottom.
    println!(
        "found {} captured signature{} (scanned {} slot{} from {newest} downward):",
        hits.len(),
        if hits.len() == 1 { "" } else { "s" },
        visited,
        if visited == 1 { "" } else { "s" },
    );
    println!();
    for (slot, sig) in &hits {
        println!("  {sig}   slot {slot}");
    }
    println!();
    println!(
        "{}",
        dim("inspect one with:  k256-replay diff <signature>")
    );
    Ok(())
}

/// Slim getBlock — just the signatures, at confirmed commitment.
/// Returns an empty Vec if the slot has no transactions but does
/// exist; returns Err if the slot isn't queryable.
fn get_block_signatures(
    http: &reqwest::blocking::Client,
    rpc_url: &str,
    slot: u64,
) -> Result<Vec<String>> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getBlock",
        "params": [slot, {
            "transactionDetails": "signatures",
            "commitment": "confirmed",
            "maxSupportedTransactionVersion": 0,
            "rewards": false,
        }],
    });
    let resp = http.post(rpc_url).json(&body).send()?;
    let v: Value = resp.json()?;
    if let Some(err) = v.get("error") {
        return Err(anyhow!(
            "{}",
            err.get("message").and_then(|m| m.as_str()).unwrap_or("rpc error")
        ));
    }
    let result = v.get("result").ok_or_else(|| anyhow!("rpc returned no result"))?;
    if result.is_null() {
        return Err(anyhow!("null block"));
    }
    let sigs = result
        .get("signatures")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(sigs)
}

fn clear(client: &ReplayClient, yes: bool, colour: bool) -> Result<()> {
    if !yes {
        eprintln!("refusing to clear the workbench cache without confirmation.");
        eprintln!("  k256-replay fixtures clear --yes");
        eprintln!();
        eprintln!("this drops every captured fixture for the current session;");
        eprintln!("evicted_count / oversized_count are NOT reset.");
        return Err(anyhow!("--yes flag required"));
    }
    let out = client.fixture_clear()?;
    if !out.cleared {
        return Err(anyhow!(
            "orchestrator returned cleared=false (removed {} fixtures); state may be inconsistent",
            out.fixtures_removed
        ));
    }
    let s = format!("cleared   removed {} fixtures", out.fixtures_removed);
    println!("{}", if colour { s.green().bold().to_string() } else { s });
    Ok(())
}
