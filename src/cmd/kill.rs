//! `k256-replay kill --yes`
//!
//! `POST /kill` SIGTERMs the validator. The orchestrator stays up; the
//! HTTP API remains reachable so the operator can `/boot` again from the
//! web console. We refuse without `--yes` because killing mid-advance
//! aborts the rpc-shreds child and loses any in-flight progress.

use anyhow::{anyhow, Result};
use owo_colors::OwoColorize;

use crate::client::ReplayClient;

pub fn run(client: &ReplayClient, yes: bool, colour: bool) -> Result<()> {
    if !yes {
        return Err(anyhow!(
            "refusing to kill without --yes (drops in-flight advance, RPC stops)"
        ));
    }
    let resp = client.kill()?;
    let phase = if colour {
        resp.phase.red().bold().to_string()
    } else {
        resp.phase.clone()
    };
    println!("validator killed   phase {phase}");
    println!("orchestrator still up — boot a fresh session from the web console.");
    Ok(())
}
