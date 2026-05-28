//! `k256-replay snapshots`

use anyhow::Result;
use comfy_table::{presets::UTF8_BORDERS_ONLY, ContentArrangement, Table};
use humansize::{format_size, DECIMAL};
use owo_colors::OwoColorize;

use crate::client::{ApiError, ReplayClient};
use crate::output::fmt_u64_str;
use crate::PresentedExit;

pub fn run(
    client: &ReplayClient,
    dashboard: &str,
    limit: u32,
    json: bool,
    colour: bool,
) -> Result<()> {
    let resp = match client.snapshots(dashboard) {
        Ok(r) => r,
        Err(e) => {
            // The catalog lives on the dashboard origin and is cookie-
            // authed (WorkOS session). The CLI's Replay bearer is NOT
            // accepted here, and the CLI has no way to obtain a WorkOS
            // cookie from a script. Surface that explicitly instead of
            // the raw `401 unauthenticated` so users don't think their
            // Replay key was rejected.
            if let Some(api) = e.downcast_ref::<ApiError>() {
                if api.status.as_u16() == 401 {
                    eprintln!(
                        "snapshots requires a dashboard session — the CLI's Replay bearer is not enough."
                    );
                    eprintln!();
                    eprintln!("the catalog lives on the dashboard origin ({dashboard}),");
                    eprintln!("authenticated by a WorkOS session cookie. The CLI has no way to");
                    eprintln!("obtain that cookie from a script.");
                    eprintln!();
                    eprintln!("browse and boot snapshots from the web console:");
                    eprintln!("  https://app.k256.xyz/app/replay");
                    eprintln!();
                    eprintln!("the CLI does not wrap /boot anyway; this command is here for");
                    eprintln!("scripts that already have a cookie they can pipe in.");
                    return Err(PresentedExit { code: 2 }.into());
                }
            }
            return Err(e);
        }
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }
    let paint_status = |s: &str| {
        if !colour {
            return s.to_string();
        }
        match s {
            "available" => s.green().to_string(),
            "deleting" => s.red().bold().to_string(),
            other => other.dimmed().to_string(),
        }
    };

    let mut t = Table::new();
    t.load_preset(UTF8_BORDERS_ONLY)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["id", "cluster", "agave", "slot", "bank", "size", "status"]);

    let limit_usize = limit as usize;
    let shown = resp.items.iter().take(limit_usize);
    let mut shown_count = 0u64;
    for snap in shown {
        t.add_row(vec![
            snap.id.clone(),
            snap.cluster.clone(),
            snap.agave_version.clone(),
            fmt_u64_str(&snap.slot.to_string()),
            snap.bank_hash_short.clone().unwrap_or_else(|| "—".into()),
            format_size(snap.size_bytes, DECIMAL),
            paint_status(&snap.status),
        ]);
        shown_count += 1;
    }
    println!("{t}");
    println!();
    println!("total {} rows (showing {})", resp.count, shown_count);
    println!("boot a row via the dashboard `/app/replay` console — the CLI does not /boot.");
    Ok(())
}
