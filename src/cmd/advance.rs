//! `k256-replay advance {start,status,history,cancel}`
//!
//! The advance subcommand surface mirrors the orchestrator's four
//! advance-related routes:
//!
//! | verb     | route                  | purpose                                     |
//! | -------- | ---------------------- | ------------------------------------------- |
//! | start    | `POST /advance`        | kick a new advance job; `--wait` blocks     |
//! | status   | `GET  /advance`        | active or most-recent job                   |
//! | history  | `GET  /advance/history`| up to 20 completed jobs (orch-side cap)     |
//! | cancel   | `POST /advance/cancel` | SIGTERM the rpc-shreds child of active job  |

use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use comfy_table::{presets::UTF8_FULL, Table};
use owo_colors::OwoColorize;

use crate::client::ReplayClient;
use crate::output::fmt_u64_str;
use crate::types::{AdvanceAccepted, AdvanceJob};
use crate::AdvanceAction;

pub fn run(client: &ReplayClient, action: AdvanceAction, colour: bool) -> Result<()> {
    match action {
        AdvanceAction::Start {
            to,
            wait,
            wait_timeout,
        } => start(client, to, wait, wait_timeout, colour),
        AdvanceAction::Status { json } => status(client, json, colour),
        AdvanceAction::History { json, limit } => history(client, json, limit, colour),
        AdvanceAction::Cancel { yes } => cancel(client, yes, colour),
    }
}

fn start(
    client: &ReplayClient,
    target: u64,
    wait: bool,
    wait_timeout_secs: u64,
    colour: bool,
) -> Result<()> {
    // Pre-flight check so we fail fast on an obvious typo (target <=
    // current_slot is always rejected by the orchestrator with `400
    // target_too_low`; making the CLI surface it before the round trip
    // saves the user a second of confusion).
    let status = client.status()?;
    let current = status.current_slot.unwrap_or(0);
    if target <= current {
        return Err(anyhow!(
            "target_slot {} must be greater than current_slot {}",
            target,
            current
        ));
    }

    let accepted = client.advance_start(target)?;
    print_accepted(&accepted, colour);

    if !wait {
        let hint = "poll `k256-replay advance status` (or pass --wait to block here).";
        println!(
            "{}",
            if colour {
                hint.dimmed().to_string()
            } else {
                hint.to_string()
            }
        );
        return Ok(());
    }

    // Poll /status until the job terminates. There are four ways out:
    //
    //   * `advance.status == "failed"` or `"cancelled"`  → bubble up as
    //     a hard error so scripts can branch.
    //   * `advance.status == "done"` AND phase=ready AND
    //      current_slot >= target                         → success.
    //   * `advance.status == "done"` but current_slot < target OR phase
    //     not yet ready                                   → partial: the
    //     job finished without reaching the target. We surface that
    //     explicitly as an error rather than hanging — the previous
    //     "wait for current_slot >= target" loop would spin forever
    //     when the orchestrator marked the job done but reported a
    //     stale current_slot (real bug seen in the field).
    //   * deadline exceeds wait_timeout_secs                → timeout.
    //
    // Print progress on every slot change so the operator sees motion.
    let deadline = Instant::now() + Duration::from_secs(wait_timeout_secs);
    let mut last_slot: Option<u64> = None;
    while Instant::now() < deadline {
        let s = client.status()?;
        let adv_status = s.advance.as_ref().and_then(|a| a.status.as_deref());
        let adv_current = s.advance.as_ref().and_then(|a| a.current_slot);
        let adv_target = s.advance.as_ref().and_then(|a| a.target_slot);

        if adv_status == Some("failed") {
            return Err(anyhow!(
                "advance status=failed (see `k256-replay advance history` for the error)"
            ));
        }
        if adv_status == Some("cancelled") {
            return Err(anyhow!(
                "advance status=cancelled (someone called /advance/cancel — see `advance history`)"
            ));
        }
        if let Some(cur) = s.current_slot {
            if last_slot != Some(cur) {
                let dot = if colour {
                    "…".dimmed().to_string()
                } else {
                    "…".to_string()
                };
                println!("  {dot} phase={} slot={}", s.phase, fmt_u64_str(&cur.to_string()));
                last_slot = Some(cur);
            }
        }

        // Success: phase ready AND fork has reached the target.
        if s.phase == "ready" && s.current_slot.unwrap_or(0) >= target {
            println!(
                "done   slot {}",
                fmt_u64_str(&s.current_slot.unwrap_or(target).to_string())
            );
            return Ok(());
        }

        // Job is `done` but we haven't met the target — partial. Don't
        // hang the loop; bubble up so the operator can see the gap.
        // `current_slot` on the embedded advance summary is the
        // authoritative "where did the job get to" — it may lead the
        // top-level `s.current_slot` (which is the bank's current_slot
        // and can lag behind the rpc-shreds child's frontier).
        if adv_status == Some("done") {
            let reached = adv_current
                .or(s.current_slot)
                .unwrap_or(0);
            if reached >= target {
                // The bank slot will catch up on the next status; we
                // already have what we asked for.
                println!("done   slot {}", fmt_u64_str(&reached.to_string()));
                return Ok(());
            }
            return Err(anyhow!(
                "advance status=done but fork only reached slot {} (target was {}, advance.target={})\n\
                 the orchestrator marked the job complete without hitting your target. \
                 try `k256-replay advance history -n 5` to see the exit code, then re-issue \
                 `advance start --to {}` to drive the rest of the way",
                fmt_u64_str(&reached.to_string()),
                fmt_u64_str(&target.to_string()),
                adv_target
                    .map(|t| fmt_u64_str(&t.to_string()))
                    .unwrap_or_else(|| "?".into()),
                target,
            ));
        }

        if matches!(s.phase.as_str(), "dead" | "stopped") {
            return Err(anyhow!(
                "validator entered terminal phase '{}' during advance",
                s.phase
            ));
        }
        sleep(Duration::from_millis(1500));
    }
    Err(anyhow!(
        "advance did not reach slot {} within {}s",
        target,
        wait_timeout_secs
    ))
}

fn status(client: &ReplayClient, json: bool, colour: bool) -> Result<()> {
    let job = client.advance_latest()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&job)?);
        return Ok(());
    }
    let Some(job) = job else {
        println!("(no advance job yet on this box)");
        return Ok(());
    };
    print_job_detail(&job, colour);
    Ok(())
}

fn history(client: &ReplayClient, json: bool, limit: usize, colour: bool) -> Result<()> {
    let jobs = client.advance_history()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&jobs)?);
        return Ok(());
    }
    if jobs.is_empty() {
        println!("(no advance jobs yet on this box)");
        return Ok(());
    }
    let mut t = Table::new();
    t.load_preset(UTF8_FULL).set_header(vec![
        "id", "status", "start", "target", "current", "duration", "error",
    ]);
    let dim = |s: &str| {
        if colour {
            s.dimmed().to_string()
        } else {
            s.to_string()
        }
    };
    for j in jobs.iter().rev().take(limit) {
        let dur = match j.finished_at_unix {
            Some(end) => format!("{}s", end.saturating_sub(j.started_at_unix)),
            None => "—".into(),
        };
        let err = j.error.as_deref().unwrap_or("");
        t.add_row(vec![
            j.id.clone(),
            paint_status(&j.status, colour),
            fmt_u64_str(&j.start_slot.to_string()),
            fmt_u64_str(&j.target_slot.to_string()),
            j.current_slot
                .map(|c| fmt_u64_str(&c.to_string()))
                .unwrap_or_else(|| "—".into()),
            dur,
            if err.is_empty() { "".into() } else { dim(err) },
        ]);
    }
    println!("{t}");
    Ok(())
}

fn cancel(client: &ReplayClient, yes: bool, colour: bool) -> Result<()> {
    if !yes {
        return Err(anyhow!(
            "refusing to cancel without --yes (sigterms the active rpc-shreds child)"
        ));
    }
    let resp = client.advance_cancel()?;
    let bold = |s: &str| {
        if colour {
            s.bold().to_string()
        } else {
            s.to_string()
        }
    };
    println!("status   {}", bold(&resp.status));
    if let Some(job) = resp.job {
        print_job_detail(&job, colour);
    }
    Ok(())
}

fn print_accepted(accepted: &AdvanceAccepted, colour: bool) {
    let bold = |s: &str| {
        if colour {
            s.bold().to_string()
        } else {
            s.to_string()
        }
    };
    println!(
        "advance {}   job {}   start {}   target {}",
        bold(&accepted.status),
        bold(&accepted.job_id),
        fmt_u64_str(&accepted.start_slot.to_string()),
        fmt_u64_str(&accepted.target_slot.to_string()),
    );
}

fn print_job_detail(j: &AdvanceJob, colour: bool) {
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
    println!(
        "{}   job {}   start {}   target {}   current {}   {:.0}%",
        paint_status(&j.status, colour),
        bold(&j.id),
        fmt_u64_str(&j.start_slot.to_string()),
        fmt_u64_str(&j.target_slot.to_string()),
        j.current_slot
            .map(|c| fmt_u64_str(&c.to_string()))
            .unwrap_or_else(|| "—".into()),
        j.percent,
    );
    let mut detail = Vec::new();
    detail.push(format!("started {}", j.started_at_unix));
    if let Some(end) = j.finished_at_unix {
        detail.push(format!("finished {} ({}s)", end, end.saturating_sub(j.started_at_unix)));
    }
    if let Some(c) = j.exit_code {
        detail.push(format!("exit {c}"));
    }
    if let Some(err) = j.error.as_deref() {
        detail.push(format!("error: {err}"));
    }
    println!("  {}", dim(&detail.join(" · ")));
}

fn paint_status(status: &str, colour: bool) -> String {
    if !colour {
        return status.to_string();
    }
    match status {
        "done" => status.green().bold().to_string(),
        "running" | "accepted" | "queued" => status.cyan().bold().to_string(),
        "cancelled" | "cancelling" => status.yellow().bold().to_string(),
        "failed" => status.red().bold().to_string(),
        _ => status.to_string(),
    }
}
