//! `k256-replay patch {apply,history}`
//!
//! Two modes for `apply`:
//!   - Single patch via flags: `--pubkey X --mode merge --lamports N
//!     [--owner P] [--executable BOOL] [--rent-epoch N]
//!     [--data-base64 ... | --data-file PATH]`.
//!   - Multi-patch from JSON: `--from-json patches.json`. The JSON must
//!     match the orchestrator's `PatchRequestBody` shape:
//!     `{ "patches": [...], "confirm_dangerous": true, "allow_epoch_boundary": false }`.
//!     The CLI sets `confirm_dangerous: true` for you on the flag path
//!     when `--yes` is passed, but does NOT mutate `--from-json` bodies
//!     beyond filling in `confirm_dangerous` if absent.
//!
//! All patch operations are destructive (they mutate canonical mainnet
//! state in the user's fork). `--yes` is required on every flag-form
//! invocation; the JSON form must carry `"confirm_dangerous": true` or
//! the CLI inserts it after a `--yes`.

use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use comfy_table::{presets::UTF8_BORDERS_ONLY, ContentArrangement, Table};
use owo_colors::OwoColorize;
use serde_json::Value;

use crate::client::ReplayClient;
use crate::types::{PatchEntry, PatchRequest};
use crate::PatchAction;

pub fn run(client: &ReplayClient, action: PatchAction, colour: bool) -> Result<()> {
    match action {
        PatchAction::Apply {
            pubkey,
            mode,
            lamports,
            owner,
            executable,
            rent_epoch,
            data_base64,
            data_file,
            from_json,
            allow_epoch_boundary,
            yes,
        } => apply(
            client,
            ApplyArgs {
                pubkey,
                mode,
                lamports,
                owner,
                executable,
                rent_epoch,
                data_base64,
                data_file,
                from_json,
                allow_epoch_boundary,
                yes,
            },
            colour,
        ),
        PatchAction::History { json, limit } => history(client, json, limit, colour),
    }
}

struct ApplyArgs {
    pubkey: Option<String>,
    mode: String,
    lamports: Option<u64>,
    owner: Option<String>,
    executable: Option<bool>,
    rent_epoch: Option<u64>,
    data_base64: Option<String>,
    data_file: Option<std::path::PathBuf>,
    from_json: Option<std::path::PathBuf>,
    allow_epoch_boundary: bool,
    yes: bool,
}

fn apply(client: &ReplayClient, args: ApplyArgs, colour: bool) -> Result<()> {
    if !args.yes {
        return Err(anyhow!(
            "patch is destructive (mutates canonical mainnet state in your fork); pass --yes to confirm"
        ));
    }

    let req = if let Some(path) = args.from_json.as_deref() {
        // Bulk path — read the orchestrator's `PatchRequestBody` shape
        // verbatim, but force `confirm_dangerous: true` so the operator
        // doesn't have to remember to set both `--yes` and the field.
        load_multi_patch_json(path)?
    } else {
        // Single-patch flag form.
        let pubkey = args
            .pubkey
            .ok_or_else(|| anyhow!("--pubkey is required (or pass --from-json to apply a batch)"))?;
        let data_base64 = resolve_data(args.data_base64.as_deref(), args.data_file.as_deref())?;
        let entry = PatchEntry {
            pubkey,
            mode: args.mode,
            lamports: args.lamports,
            owner: args.owner,
            executable: args.executable,
            rent_epoch: args.rent_epoch,
            data_base64,
        };
        PatchRequest {
            patches: vec![entry],
            confirm_dangerous: true,
            allow_epoch_boundary: args.allow_epoch_boundary,
        }
    };

    if req.patches.is_empty() {
        return Err(anyhow!("nothing to patch — `patches` array is empty"));
    }

    let resp = client.patch_apply(&req)?;
    let bold = |s: &str| {
        if colour {
            s.bold().to_string()
        } else {
            s.to_string()
        }
    };
    println!("{}   {} patch(es)", bold("applied"), req.patches.len());
    // The orchestrator forwards the validator's response JSON
    // verbatim. We print the salient summary fields when we recognise
    // them; everything else is available via `--from-json` round-trips.
    print_apply_summary(&resp, colour);
    Ok(())
}

fn resolve_data(b64: Option<&str>, file: Option<&Path>) -> Result<Option<String>> {
    match (b64, file) {
        (Some(b), None) => Ok(Some(b.to_string())),
        (None, Some(p)) => {
            let bytes = fs::read(p).with_context(|| format!("reading {}", p.display()))?;
            Ok(Some(base64::engine::general_purpose::STANDARD.encode(bytes)))
        }
        (None, None) => Ok(None),
        (Some(_), Some(_)) => Err(anyhow!("pass either --data-base64 or --data-file, not both")),
    }
}

fn load_multi_patch_json(path: &Path) -> Result<PatchRequest> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading patch JSON from {}", path.display()))?;
    let mut v: Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing patch JSON from {}", path.display()))?;
    // Force confirm_dangerous = true; the CLI is the confirmation
    // surface here. allow_epoch_boundary defaults false unless the JSON
    // explicitly sets it.
    if let Some(obj) = v.as_object_mut() {
        obj.insert("confirm_dangerous".to_string(), Value::Bool(true));
        if !obj.contains_key("allow_epoch_boundary") {
            obj.insert("allow_epoch_boundary".to_string(), Value::Bool(false));
        }
    } else {
        return Err(anyhow!(
            "patch JSON must be a top-level object with a `patches` array"
        ));
    }
    let req: PatchRequest = serde_json::from_value(v)
        .context("patch JSON didn't match PatchRequest shape (see README)")?;
    Ok(req)
}

fn print_apply_summary(resp: &Value, colour: bool) {
    let dim = |s: &str| {
        if colour {
            s.dimmed().to_string()
        } else {
            s.to_string()
        }
    };
    // The validator-side response carries per-patch result records.
    // We pick the fields a human cares about; agents should use --json
    // (round-trip the body verbatim via curl) for the full shape.
    if let Some(applied) = resp.get("applied_count").and_then(Value::as_u64) {
        println!("applied_count   {applied}");
    }
    let base_slot = resp.get("base_slot").and_then(Value::as_u64);
    let applied_slot = resp.get("applied_slot").and_then(Value::as_u64);
    if let (Some(b), Some(a)) = (base_slot, applied_slot) {
        println!("base_slot       {b}");
        println!("applied_slot    {a}");
        // The validator builds a synthetic child bank from `base_slot`
        // and writes the patch there at `applied_slot`. `base_slot` is
        // the agreed parent (last rooted slot before patching); it
        // does NOT advance when you call /advance — patches always
        // graft onto the same base until the fork is rebooted. So
        // `applied_slot` typically lags /status's `current_slot` by
        // however many slots have been advanced since boot. That's
        // working as designed: it lets later patches replay against
        // the same canonical base.
        if a < b + 8 {
            println!(
                "{}",
                dim("hint: applied_slot lags current_slot by design — patches anchor to base_slot, not the fork head"),
            );
        }
    } else if let Some(slot) = resp.get("synthetic_child_slot").and_then(Value::as_u64) {
        println!("child_bank_slot {slot}");
    }
    if let Some(patches) = resp.get("patches").and_then(Value::as_array) {
        for (i, p) in patches.iter().enumerate() {
            let pk = p
                .get("pubkey")
                .and_then(Value::as_str)
                .unwrap_or("(no pubkey)");
            let mode = p.get("mode").and_then(Value::as_str).unwrap_or("?");
            let lam_before = p.get("lamports_before").and_then(Value::as_u64);
            let lam_after = p.get("lamports_after").and_then(Value::as_u64);
            print!("  [{i}] {pk}   mode={mode}");
            if let (Some(b), Some(a)) = (lam_before, lam_after) {
                print!("   lamports {} → {}", b, a);
            }
            println!();
        }
        return;
    }
    // Fallback: dump the body so the operator can see what the
    // validator sent back.
    let pretty = serde_json::to_string_pretty(resp).unwrap_or_else(|_| resp.to_string());
    println!("{}", dim(&pretty));
}

fn history(client: &ReplayClient, json: bool, limit: usize, colour: bool) -> Result<()> {
    // `/accounts/patch/history` returns the full mutation audit log
    // (kinds: `account_patch`, `program_deploy`, `transaction_splice`).
    // Filter to patches only so this command does what its name says;
    // deploys live in `deploy history`.
    let records: Vec<_> = client
        .patch_history()?
        .into_iter()
        .filter(|r| r.get("kind").and_then(Value::as_str) == Some("account_patch"))
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&records)?);
        return Ok(());
    }
    if records.is_empty() {
        println!("no patches applied yet on this session.");
        println!("(use `k256-replay deploy history` for program deploys, or");
        println!(" pass `--json` against /accounts/patch/history for the raw");
        println!(" mutation log including deploys and splices.)");
        return Ok(());
    }
    let mut t = Table::new();
    t.load_preset(UTF8_BORDERS_ONLY)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["t", "kind", "pubkey", "mode", "lamports", "data"]);
    let dim = |s: &str| {
        if colour {
            s.dimmed().to_string()
        } else {
            s.to_string()
        }
    };
    for r in records.iter().rev().take(limit) {
        let ts = r
            .get("ts_unix")
            .and_then(Value::as_u64)
            .map(|n| n.to_string())
            .or_else(|| {
                r.get("ts").and_then(Value::as_str).map(str::to_string)
            })
            .unwrap_or_else(|| "—".into());
        let kind = r
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("patch")
            .to_string();
        let pubkey = r
            .get("pubkey")
            .and_then(Value::as_str)
            .map(short_pubkey)
            .unwrap_or_else(|| "—".into());
        let mode = r
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("—")
            .to_string();
        let lam = match (
            r.get("lamports_before").and_then(Value::as_u64),
            r.get("lamports_after").and_then(Value::as_u64),
        ) {
            (Some(b), Some(a)) => format!("{} → {}", b, a),
            (None, Some(a)) => format!("→ {}", a),
            _ => "—".into(),
        };
        let data = match (
            r.get("data_len_before").and_then(Value::as_u64),
            r.get("data_len_after").and_then(Value::as_u64),
        ) {
            (Some(b), Some(a)) if b != a => format!("len {} → {}", b, a),
            (Some(b), Some(_)) => format!("len {}", b),
            _ => "—".into(),
        };
        t.add_row(vec![dim(&ts), kind, pubkey, mode, lam, data]);
    }
    println!("{t}");
    Ok(())
}

fn short_pubkey(p: &str) -> String {
    if p.len() <= 8 {
        return p.to_string();
    }
    format!("{}…{}", &p[..4], &p[p.len() - 4..])
}
