//! `k256-replay logs [-n N] [--follow]`
//!
//! Two modes:
//!   - **tail (default)**: one-shot `GET /logs/tail?lines=N`.
//!   - **follow (`-f` / `--follow`)**: long-lived SSE on `/logs/stream`.
//!     Reads bytes off the response as they arrive, splits on `\n`,
//!     filters SSE control frames (`event: keepalive`, blank lines)
//!     and prints each `data: <line>`. Press Ctrl-C to stop.

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use owo_colors::OwoColorize;

use crate::client::ReplayClient;

pub fn run(client: &ReplayClient, lines: Option<u32>, follow: bool, colour: bool) -> Result<()> {
    if follow {
        return follow_stream(client, colour);
    }
    let out = client.logs_tail(lines)?;
    for line in out.lines {
        println!("{line}");
    }
    Ok(())
}

fn follow_stream(client: &ReplayClient, colour: bool) -> Result<()> {
    let resp = client.logs_stream()?;
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        // First Ctrl-C: flip the flag and let the next read attempt
        // (or the next stdout flush) bail out cleanly. Second Ctrl-C
        // immediately exits via the default SIGINT handler we install
        // when the closure runs the second time.
        let mut hits = 0u32;
        ctrlc::set_handler(move || {
            hits += 1;
            stop.store(true, Ordering::SeqCst);
            if hits >= 2 {
                std::process::exit(130);
            }
        })
        .context("installing Ctrl-C handler")?;
    }

    let banner = "(streaming /logs/stream — Ctrl-C to stop)";
    eprintln!(
        "{}",
        if colour {
            banner.dimmed().to_string()
        } else {
            banner.to_string()
        }
    );

    let mut reader = BufReader::new(resp);
    let mut line = String::new();
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                // Server closed. Surface a hint — most often this
                // means the validator restarted / the orchestrator
                // dropped the SSE connection.
                let msg = "(stream closed by the server)";
                eprintln!(
                    "{}",
                    if colour {
                        msg.dimmed().to_string()
                    } else {
                        msg.to_string()
                    }
                );
                return Ok(());
            }
            Ok(_) => {
                let trimmed = line.trim_end_matches(['\n', '\r']);
                if trimmed.is_empty() {
                    continue; // SSE record separator.
                }
                if let Some(rest) = trimmed.strip_prefix("data:") {
                    // SSE rule: leading space after the colon is part
                    // of the framing, not the payload.
                    let payload = rest.strip_prefix(' ').unwrap_or(rest);
                    println!("{payload}");
                    continue;
                }
                if trimmed.starts_with(':') {
                    // SSE comment (often a heartbeat) — drop silently.
                    continue;
                }
                if trimmed.starts_with("event:") || trimmed.starts_with("id:") {
                    // Control records — skip; we only care about data.
                    continue;
                }
                // Anything else, surface verbatim. The orchestrator
                // shouldn't emit this, but a future encoding change
                // would otherwise silently drop lines.
                println!("{trimmed}");
            }
            Err(e) => {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                return Err(anyhow!("reading /logs/stream: {e}"));
            }
        }
    }
    Ok(())
}
