//! `k256-replay deploy {program,history}`
//!
//! `deploy program` wraps `POST /programs/deploy`. The CLI reads the
//! ELF from disk and base64-encodes it for the operator (the
//! orchestrator hard-caps the base64 payload at ~13 MiB). Two status
//! shapes are valid:
//!   - `--status deployed`  → `--authority <address>` is required.
//!   - `--status finalized` → `--next-version <address>` is required.
//!
//! Both modes are destructive — they overwrite the on-chain bytes the
//! validator returns for that program. Pass `--yes` to confirm.

use std::fs;

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use comfy_table::{presets::UTF8_BORDERS_ONLY, ContentArrangement, Table};
use owo_colors::OwoColorize;
use serde_json::Value;

use crate::client::ReplayClient;
use crate::types::{DeployRequest, DeployResponse};
use crate::DeployAction;

const ELF_BASE64_HARD_CAP: usize = 13 * 1024 * 1024;

pub fn run(client: &ReplayClient, action: DeployAction, colour: bool) -> Result<()> {
    match action {
        DeployAction::Program {
            program_id,
            elf,
            elf_base64,
            status,
            authority_address,
            next_version_address,
            force_replace_finalized,
            force_replace_legacy_loader,
            yes,
        } => program(
            client,
            ProgramArgs {
                program_id,
                elf,
                elf_base64,
                status,
                authority_address,
                next_version_address,
                force_replace_finalized,
                force_replace_legacy_loader,
                yes,
            },
            colour,
        ),
        DeployAction::History { json, limit } => history(client, json, limit, colour),
    }
}

struct ProgramArgs {
    program_id: String,
    elf: Option<std::path::PathBuf>,
    elf_base64: Option<String>,
    status: String,
    authority_address: Option<String>,
    next_version_address: Option<String>,
    force_replace_finalized: bool,
    force_replace_legacy_loader: bool,
    yes: bool,
}

fn program(client: &ReplayClient, args: ProgramArgs, colour: bool) -> Result<()> {
    if !args.yes {
        return Err(anyhow!(
            "deploy is destructive (overwrites the program account's bytes in your fork); pass --yes to confirm"
        ));
    }

    // Resolve ELF: --elf reads + encodes; --elf-base64 trusts the
    // caller's already-encoded payload.
    let elf_base64 = match (args.elf.as_deref(), args.elf_base64.as_deref()) {
        (Some(p), None) => {
            let bytes = fs::read(p).with_context(|| format!("reading {}", p.display()))?;
            let b = base64::engine::general_purpose::STANDARD.encode(&bytes);
            if b.len() > ELF_BASE64_HARD_CAP {
                return Err(anyhow!(
                    "encoded ELF length {} exceeds orchestrator cap {} (≈10 MiB raw)",
                    b.len(),
                    ELF_BASE64_HARD_CAP
                ));
            }
            b
        }
        (None, Some(s)) => {
            if s.is_empty() {
                return Err(anyhow!("--elf-base64 is empty"));
            }
            if s.len() > ELF_BASE64_HARD_CAP {
                return Err(anyhow!(
                    "encoded ELF length {} exceeds orchestrator cap {} (≈10 MiB raw)",
                    s.len(),
                    ELF_BASE64_HARD_CAP
                ));
            }
            s.to_string()
        }
        (None, None) => return Err(anyhow!("pass --elf <PATH> or --elf-base64 <STRING>")),
        (Some(_), Some(_)) => return Err(anyhow!("pass either --elf or --elf-base64, not both")),
    };

    // Cross-flag validation here so the user sees the error before
    // round-tripping. The orchestrator enforces the same rules and
    // returns 400 on violation, but local is faster + clearer.
    match args.status.as_str() {
        "deployed" => {
            if args.authority_address.is_none() {
                return Err(anyhow!(
                    "--authority <address> is required when --status deployed"
                ));
            }
        }
        "finalized" => {
            if args.next_version_address.is_none() {
                return Err(anyhow!(
                    "--next-version <address> is required when --status finalized"
                ));
            }
        }
        other => return Err(anyhow!("--status must be `deployed` or `finalized`, got {other}")),
    }

    let req = DeployRequest {
        program_id: args.program_id,
        elf_base64,
        status: args.status,
        authority_address: args.authority_address,
        next_version_address: args.next_version_address,
        force_replace_finalized: args.force_replace_finalized,
        force_replace_legacy_loader: args.force_replace_legacy_loader,
        confirm_dangerous: true,
    };

    let resp = client.deploy(&req)?;
    print_deploy_summary(&resp, colour);
    Ok(())
}

fn print_deploy_summary(r: &DeployResponse, colour: bool) {
    let bold = |s: &str| {
        if colour {
            s.bold().to_string()
        } else {
            s.to_string()
        }
    };
    let dim = |s: &str| {
        if colour {
            s.dimmed().to_string()
        } else {
            s.to_string()
        }
    };
    println!("{}   program {}", bold(&r.operation), bold(&r.program_id));
    println!("  loader            {}", r.loader);
    println!("  status            {}", r.status);
    println!("  effective_slot    {}", r.effective_slot);
    println!("  materialized_slot {}", r.materialized_slot);
    println!("  base_slot         {}", r.base_slot);
    println!("  elf_len           {}", r.elf_len);
    println!("  elf_sha256        {}", dim(&r.elf_sha256));
    if let Some(b) = r.account_sha256_before.as_deref() {
        println!("  acct_before       {}", dim(b));
    }
    println!("  acct_after        {}", dim(&r.account_sha256_after));
    println!(
        "  cache_published   {}",
        if r.cache_published { "yes" } else { "no" }
    );
    if let Some(a) = r.authority_address.as_deref() {
        println!("  authority         {a}");
    }
    if let Some(n) = r.next_version_address.as_deref() {
        println!("  next_version      {n}");
    }
}

fn history(client: &ReplayClient, json: bool, limit: usize, colour: bool) -> Result<()> {
    // `/programs/deploy/history` returns the full mutation audit log
    // (kinds: `account_patch`, `program_deploy`, `transaction_splice`).
    // Filter to deploys only so this command does what its name says;
    // patches live in `patch history`.
    let records: Vec<_> = client
        .deploy_history()?
        .into_iter()
        .filter(|r| r.get("kind").and_then(Value::as_str) == Some("program_deploy"))
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&records)?);
        return Ok(());
    }
    if records.is_empty() {
        println!("no programs deployed yet on this session.");
        return Ok(());
    }
    let mut t = Table::new();
    t.load_preset(UTF8_BORDERS_ONLY)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            "t",
            "operation",
            "program",
            "loader",
            "status",
            "elf_len",
            "effective_slot",
        ]);
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
            .unwrap_or_else(|| "—".into());
        let op = r
            .get("operation")
            .and_then(Value::as_str)
            .unwrap_or("deploy")
            .to_string();
        let prog = r
            .get("program_id")
            .and_then(Value::as_str)
            .map(short_pubkey)
            .unwrap_or_else(|| "—".into());
        let loader = r
            .get("loader")
            .and_then(Value::as_str)
            .unwrap_or("—")
            .to_string();
        let status = r
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("—")
            .to_string();
        let elf_len = r
            .get("elf_len")
            .and_then(Value::as_u64)
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".into());
        let slot = r
            .get("effective_slot")
            .and_then(Value::as_u64)
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".into());
        t.add_row(vec![dim(&ts), op, prog, loader, status, elf_len, slot]);
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
