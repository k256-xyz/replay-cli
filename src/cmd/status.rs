//! `k256-replay status [--watch]`
//!
//! Prints phase / slots / versions / advance state / mutation state /
//! workbench cache in one screen. Layout (with `--watch`, redrawn every
//! 1.5s):
//!
//! ```text
//! phase    ready                            slot       422,097,261
//! orch     4a81d6d031 (2026-05-25)          validator  agave 4.0.0-workbench
//! plugin   yellowstone                      rpc        listening
//! snapshot 422,097,233                      mutation   clean
//! advance  ready · target 422,097,261 · current 422,097,261 · job adv-1779…
//! cache    1,192 fixtures   3 slots   67 MB / 1.07 GB   cleared 5
//! (cap is the validator's `TX_FIXTURE_CACHE_MAX_BYTES` constant —
//! older binaries report 1 GiB, current/post-rebuild reports 5 GiB.)
//! ```

use std::io::Write;
use std::thread::sleep;
use std::time::Duration;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::client::ReplayClient;
use crate::output::{fmt_stats_line, fmt_u64_str};
use crate::types::Status;

pub fn run(client: &ReplayClient, watch: bool, json: bool, colour: bool) -> Result<()> {
    if json {
        let s = client.status()?;
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }
    if !watch {
        return print_once(client, colour);
    }
    // Watch mode: clear screen + cursor home, sleep, repeat. ESC[2J wipes
    // the screen; ESC[H sends the cursor home. Behaves cleanly under
    // tmux / kitty / iTerm. Ctrl-C exits.
    loop {
        print!("\x1b[2J\x1b[H");
        std::io::stdout().flush().ok();
        if let Err(e) = print_once(client, colour) {
            eprintln!("error: {e:#}");
        }
        sleep(Duration::from_millis(1500));
    }
}

fn print_once(client: &ReplayClient, colour: bool) -> Result<()> {
    let s = client.status()?;
    let stats = client.fixture_stats().ok();

    let label = |k: &str| if colour { k.dimmed().to_string() } else { k.to_string() };
    let v = |s: &str| if colour { s.bold().to_string() } else { s.to_string() };

    let phase = paint_phase(&s.phase, colour);
    let slot = s
        .current_slot
        .map(|n| fmt_u64_str(&n.to_string()))
        .unwrap_or_else(|| "—".into());
    let orch = s.orchestrator_version.as_deref().unwrap_or("—");
    let val = s.validator_version.as_deref().unwrap_or("—");
    let plugin = s.geyser_plugin.as_deref().unwrap_or("none");
    let rpc = match s.rpc_listening {
        Some(true) => paint("listening", "ok", colour),
        Some(false) => paint("not ready", "warn", colour),
        None => "—".to_string(),
    };
    let snapshot = s
        .snapshot_slot
        .map(|n| fmt_u64_str(&n.to_string()))
        .unwrap_or_else(|| "—".into());
    let mutation = paint_mutation(&s, colour);
    let advance_line = format_advance(&s, colour);

    println!("{:8} {:34}   {:9} {}", label("phase"),    v(&phase),    label("slot"),      v(&slot));
    println!("{:8} {:34}   {:9} {}", label("orch"),     v(orch),      label("validator"), v(val));
    println!("{:8} {:34}   {:9} {}", label("plugin"),   v(plugin),    label("rpc"),       v(&rpc));
    println!("{:8} {:34}   {:9} {}", label("snapshot"), v(&snapshot), label("mutation"),  v(&mutation));
    println!("{:8} {}", label("advance"), advance_line);
    if let Some(stats) = stats {
        println!("{}", fmt_stats_line(&stats, colour));
    } else {
        println!("{}   (workbench stats unavailable)", label("cache"));
    }
    // Host-disk pressure line — only when the orchestrator surfaces it
    // (older binaries don't, in which case we stay silent). When
    // `refuse_new_checkpoint` or `warn` are set we render the line in
    // red / yellow so the operator sees pressure on the next status
    // poll, not after `checkpoint save` returns 507.
    if let Some(d) = &s.disk {
        let pct = format!("{:.1}%", d.pct_used);
        let bytes = format!(
            "{} / {}",
            humansize::format_size(d.used_bytes, humansize::DECIMAL),
            humansize::format_size(d.total_bytes, humansize::DECIMAL),
        );
        let line = if d.refuse_new_checkpoint {
            paint(
                &format!(
                    "host disk    {}   {}   (checkpoint save WILL be refused — prune first)",
                    pct, bytes
                ),
                "err",
                colour,
            )
        } else if d.warn {
            paint(
                &format!(
                    "host disk    {}   {}   (warn — consider `k256-replay checkpoint prune --keep-latest <N> --yes`)",
                    pct, bytes
                ),
                "warn",
                colour,
            )
        } else {
            format!("{}{}   {pct}   {bytes}", label("host disk"), " ".repeat(7))
        };
        println!("{line}");
    }
    Ok(())
}

fn paint_phase(phase: &str, colour: bool) -> String {
    if !colour {
        return phase.to_string();
    }
    match phase {
        "ready" => phase.green().bold().to_string(),
        "advancing" => phase.cyan().bold().to_string(),
        "dead" | "stopped" => phase.red().bold().to_string(),
        _ => phase.yellow().to_string(),
    }
}

fn paint(s: &str, tone: &str, colour: bool) -> String {
    if !colour {
        return s.to_string();
    }
    match tone {
        "ok" => s.green().to_string(),
        "warn" => s.yellow().to_string(),
        "err" => s.red().bold().to_string(),
        _ => s.to_string(),
    }
}

fn paint_mutation(s: &Status, colour: bool) -> String {
    let Some(m) = s.mutation.as_ref() else {
        return "—".to_string();
    };
    match m.dirty {
        // `dirty` flips the first time the fork has been patched,
        // deployed against, or spliced. It means the bank no longer
        // matches canonical mainnet PoH — expected and intended on a
        // workbench; it only resets on a fresh `/boot`.
        // `checkpoint restore` does NOT clear the flag.
        Some(true) => {
            // Surface the count + last kind alongside `dirty` so an
            // operator scanning `/status` can answer "what changed?"
            // without going through `/patch/history` or
            // `/programs/deploy/history`.
            let mut detail_parts: Vec<String> = Vec::with_capacity(2);
            if let Some(c) = m.mutation_count {
                detail_parts.push(format!(
                    "{} mutation{}",
                    c,
                    if c == 1 { "" } else { "s" }
                ));
            }
            if let Some(k) = m.last_mutation_kind.as_deref() {
                detail_parts.push(format!("last: {k}"));
            }
            let suffix = if detail_parts.is_empty() {
                String::new()
            } else {
                format!(" · {}", detail_parts.join(", "))
            };
            paint(
                &format!("dirty (diverged from mainnet — expected on a workbench){suffix}"),
                "warn",
                colour,
            )
        }
        Some(false) => paint("clean", "ok", colour),
        None => "—".to_string(),
    }
}

fn format_advance(s: &Status, colour: bool) -> String {
    let Some(a) = s.advance.as_ref() else {
        return "—".to_string();
    };
    let mut parts = Vec::with_capacity(4);
    let status = a.status.as_deref().unwrap_or("—");
    parts.push(if colour { status.bold().to_string() } else { status.to_string() });
    if let Some(t) = a.target_slot {
        parts.push(format!("target {}", fmt_u64_str(&t.to_string())));
    }
    if let Some(c) = a.current_slot {
        parts.push(format!("current {}", fmt_u64_str(&c.to_string())));
    }
    if let Some(id) = a.id.as_deref() {
        parts.push(format!("job {id}"));
    }
    if let Some(p) = a.percent {
        if (0.0..100.0).contains(&p) {
            parts.push(format!("{p:.0}%"));
        }
    }
    parts.join(" · ")
}
