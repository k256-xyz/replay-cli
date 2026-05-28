//! `k256-replay clear --yes`

use anyhow::{anyhow, Result};
use owo_colors::OwoColorize;

use crate::client::ReplayClient;

pub fn run(client: &ReplayClient, yes: bool, colour: bool) -> Result<()> {
    if !yes {
        eprintln!("refusing to clear the workbench cache without confirmation.");
        eprintln!("  k256-replay clear --yes");
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
