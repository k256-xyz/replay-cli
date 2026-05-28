//! `k256-replay checkpoint save|list|restore|prune`

use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use comfy_table::{presets::UTF8_BORDERS_ONLY, ContentArrangement, Table};
use owo_colors::OwoColorize;

use crate::client::ReplayClient;
use crate::CheckpointAction;

pub fn run(client: &ReplayClient, action: CheckpointAction, colour: bool) -> Result<()> {
    match action {
        CheckpointAction::Save { label } => save(client, label.as_deref(), colour),
        CheckpointAction::List { json } => list(client, json, colour),
        CheckpointAction::Restore { id } => restore(client, &id),
        CheckpointAction::Prune {
            id,
            older_than,
            keep_latest,
            dry_run,
            yes,
        } => prune(
            client,
            PruneArgs {
                ids: id,
                older_than: older_than.as_deref(),
                keep_latest,
                dry_run,
                yes,
            },
            colour,
        ),
    }
}

struct PruneArgs<'a> {
    ids: Vec<String>,
    older_than: Option<&'a str>,
    keep_latest: Option<usize>,
    dry_run: bool,
    yes: bool,
}

/// Parse `1h` / `45m` / `7d` into seconds. We deliberately do NOT
/// accept calendar units (weeks, months) — operators bin checkpoints
/// by ops time, not calendar time, so simple multiplication is the
/// honest contract.
fn parse_duration(s: &str) -> Result<u64> {
    let s = s.trim();
    let (n, unit) = s
        .strip_suffix(['s', 'm', 'h', 'd'])
        .map(|n| (n, &s[s.len() - 1..]))
        .ok_or_else(|| anyhow!("--older-than: expected '<N>s|m|h|d' (got '{s}')"))?;
    let v: u64 = n
        .parse()
        .map_err(|_| anyhow!("--older-than: '{n}' is not a non-negative integer"))?;
    let mult = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 60 * 60 * 24,
        _ => unreachable!("strip_suffix matched"),
    };
    Ok(v * mult)
}

fn prune(client: &ReplayClient, args: PruneArgs<'_>, colour: bool) -> Result<()> {
    let dim = |s: &str| if colour { s.dimmed().to_string() } else { s.to_string() };

    // Validate the filter combination upfront so the user gets a clean
    // error instead of "0 checkpoints matched" on a typo.
    if args.ids.is_empty() && args.older_than.is_none() && args.keep_latest.is_none() {
        return Err(anyhow!(
            "prune needs at least one of `--id <ID>`, `--older-than <DURATION>`, or `--keep-latest <N>`"
        ));
    }
    if !args.dry_run && !args.yes {
        return Err(anyhow!(
            "prune is destructive; pass `--yes` to confirm or `--dry-run` to preview"
        ));
    }

    // Resolve the list of checkpoints to delete. The orchestrator
    // surfaces every checkpoint via `GET /checkpoints`; we filter
    // client-side so the wire schema stays minimal and the operator
    // can preview with `--dry-run` cheaply.
    let list = client.checkpoints()?.into_vec();
    if list.is_empty() {
        println!("no checkpoints to prune.");
        return Ok(());
    }

    // Build the to-delete set per the filters.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let older_secs = args.older_than.map(parse_duration).transpose()?;

    // Sort newest-first so `--keep-latest N` is "skip the first N".
    // Checkpoints without a parseable `created_at` sort to the end so
    // they're treated as oldest (delete-first); this matches the safest
    // default — we never accidentally protect an opaque row.
    let mut sorted: Vec<_> = list.to_vec();
    sorted.sort_by_key(|c| std::cmp::Reverse(c.created_at_unix().unwrap_or(0)));

    let mut to_delete: Vec<_> = Vec::new();
    if !args.ids.is_empty() {
        let ids: std::collections::HashSet<&str> = args.ids.iter().map(String::as_str).collect();
        to_delete = sorted.into_iter().filter(|c| ids.contains(c.id.as_str())).collect();
    } else {
        let skip = args.keep_latest.unwrap_or(0);
        for c in sorted.into_iter().skip(skip) {
            if let Some(threshold) = older_secs {
                // No timestamp ⇒ treat as oldest (definitely older than the threshold).
                let age = c
                    .created_at_unix()
                    .map(|t| now.saturating_sub(t))
                    .unwrap_or(u64::MAX);
                if age < threshold {
                    continue;
                }
            }
            to_delete.push(c);
        }
    }

    if to_delete.is_empty() {
        println!("no checkpoints matched the filter — nothing to delete.");
        return Ok(());
    }

    println!("would delete {} checkpoint(s):", to_delete.len());
    for c in &to_delete {
        let label = c
            .label
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("(no label)");
        let created = c.created_at.as_deref().unwrap_or("(no timestamp)");
        println!(
            "  {} {} {}",
            if colour { c.id.bold().to_string() } else { c.id.clone() },
            dim(label),
            dim(&format!("(created {created})")),
        );
    }

    if args.dry_run {
        println!();
        println!("{}", dim("dry-run: nothing deleted. Rerun without `--dry-run` (and with `--yes`) to apply."));
        return Ok(());
    }

    let mut deleted = 0;
    let mut failed: Vec<(String, String)> = Vec::new();
    for c in &to_delete {
        match client.checkpoint_delete(&c.id) {
            Ok(_) => {
                deleted += 1;
                println!("  ✓ {}", c.id);
            }
            Err(e) => {
                failed.push((c.id.clone(), format!("{e:#}")));
                println!("  ✗ {} — {}", c.id, e);
            }
        }
    }
    println!();
    println!("deleted {} / {} requested", deleted, to_delete.len());
    if !failed.is_empty() {
        return Err(anyhow!(
            "{} delete(s) failed — re-run to retry the remaining ids",
            failed.len()
        ));
    }
    Ok(())
}

fn save(client: &ReplayClient, label: Option<&str>, colour: bool) -> Result<()> {
    let ack = client.checkpoint_save(label)?;
    if !ack.accepted {
        return Err(anyhow!(
            "orchestrator rejected the checkpoint: {}",
            ack.message.as_deref().unwrap_or("no message")
        ));
    }
    let dim = |s: &str| if colour { s.dimmed().to_string() } else { s.to_string() };
    println!(
        "checkpoint accepted   {}",
        dim(ack.message.as_deref().unwrap_or("save running in background"))
    );

    let Some(label) = label else {
        let hint = "list saved checkpoints with `k256-replay checkpoint list`.";
        println!("{}", dim(hint));
        return Ok(());
    };

    // Poll /checkpoints up to 480s for the label to show up. The save
    // runs on fc-agent, which pauses the guest *and* can leave the
    // orchestrator unreachable for ~60-300s on a fully-warm validator
    // (snapshot footprint scales with AccountsDB hot set). We must
    // outlast that window — anything shorter masks a successful save
    // as a CLI failure.
    let deadline = Instant::now() + Duration::from_secs(480);
    let dim = |s: &str| if colour { s.dimmed().to_string() } else { s.to_string() };
    let mut last_err: Option<String> = None;
    let mut last_log = Instant::now();
    while Instant::now() < deadline {
        match client.checkpoints() {
            Ok(list) => {
                if let Some(ckpt) = list
                    .into_vec()
                    .into_iter()
                    .find(|c| c.label.as_deref() == Some(label))
                {
                    println!(
                        "checkpoint saved      id {}{}",
                        if colour { ckpt.id.bold().to_string() } else { ckpt.id.clone() },
                        ckpt.slot
                            .map(|s| format!("   slot {}", s))
                            .unwrap_or_default(),
                    );
                    return Ok(());
                }
                last_err = None;
            }
            Err(e) => {
                last_err = Some(format!("{e:#}"));
            }
        }
        // Heartbeat every ~10s so the user knows the CLI is still
        // waiting through a long fc-agent pause; quiet otherwise.
        if last_log.elapsed() >= Duration::from_secs(10) {
            let remaining = deadline.saturating_duration_since(Instant::now()).as_secs();
            match last_err.as_deref() {
                // 5xx / unreachable while the guest is paused mid-save is the
                // expected state, not a failure. Make that explicit so an
                // operator watching the heartbeats can tell normal from broken.
                Some(err) => eprintln!(
                    "{} polling — orchestrator briefly unreachable, this is expected during the fc-agent snapshot window ({remaining}s budget left)\n{} {}",
                    dim("…"), dim("    last:"), dim(err)
                ),
                None => eprintln!("{} polling for label '{label}' ({remaining}s left)", dim("…")),
            }
            last_log = Instant::now();
        }
        sleep(Duration::from_millis(2000));
    }
    eprintln!(
        "note: didn't find label '{label}' in /checkpoints within 480s.{}",
        last_err
            .as_deref()
            .map(|e| format!(" last orchestrator error: {e}"))
            .unwrap_or_default(),
    );
    eprintln!("      it might still be saving — run `k256-replay checkpoint list`.");
    Ok(())
}

fn list(client: &ReplayClient, json: bool, colour: bool) -> Result<()> {
    let list = client.checkpoints()?.into_vec();
    if json {
        println!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }
    if list.is_empty() {
        println!("no checkpoints saved.");
        println!("  k256-replay checkpoint save --label before-experiment");
        return Ok(());
    }
    let mut t = Table::new();
    t.load_preset(UTF8_BORDERS_ONLY)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["id", "label", "slot", "age", "created"]);
    let dim = |s: &str| if colour { s.dimmed().to_string() } else { s.to_string() };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    for c in &list {
        let (age_cell, created_cell) = match c.created_at_unix() {
            Some(unix) => {
                let age = (now - unix as i64).max(0) as u64;
                (humanize_age(age), format_iso8601_utc(unix))
            }
            None => (dim("—"), dim("—")),
        };
        t.add_row(vec![
            c.id.clone(),
            c.label.clone().unwrap_or_else(|| dim("—")),
            c.slot
                .map(|s| crate::output::fmt_u64_str(&s.to_string()))
                .unwrap_or_else(|| dim("—")),
            age_cell,
            created_cell,
        ]);
    }
    println!("{t}");
    println!();
    let hint = "restore: k256-replay checkpoint restore <id>";
    println!("{}", if colour { hint.dimmed().to_string() } else { hint.to_string() });
    Ok(())
}

/// "2m", "47m", "3h12m", "5d3h". `created_at` is unix seconds — we
/// turn it into a wall-clock-relative duration so the operator
/// doesn't have to do epoch math to decide what's recent.
fn humanize_age(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    let m = secs / 60;
    if m < 60 {
        return format!("{m}m");
    }
    let h = m / 60;
    let m_rem = m % 60;
    if h < 24 {
        if m_rem == 0 {
            return format!("{h}h");
        }
        return format!("{h}h{m_rem}m");
    }
    let d = h / 24;
    let h_rem = h % 24;
    if h_rem == 0 {
        format!("{d}d")
    } else {
        format!("{d}d{h_rem}h")
    }
}

/// Format unix seconds as ISO 8601 in UTC, e.g. "2026-05-26 19:13:22Z".
/// We avoid pulling in chrono/time for a single format call — manual
/// math on civil-time fields is exact for the post-1970 range and
/// keeps the binary tiny.
fn format_iso8601_utc(unix_secs: u64) -> String {
    const SECS_PER_DAY: u64 = 86_400;
    let days_since_epoch = unix_secs / SECS_PER_DAY;
    let secs_in_day = unix_secs % SECS_PER_DAY;
    let hour = (secs_in_day / 3600) as u32;
    let minute = ((secs_in_day % 3600) / 60) as u32;
    let second = (secs_in_day % 60) as u32;

    // Howard Hinnant's civil_from_days, adapted for u64.
    let z = days_since_epoch as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}Z",
        year, m, d, hour, minute, second
    )
}

fn restore(client: &ReplayClient, id: &str) -> Result<()> {
    client.restore(id)?;
    println!("restore accepted   id {id}");
    println!("RPC briefly unavailable while the fork comes back. Poll `k256-replay status`.");
    Ok(())
}
