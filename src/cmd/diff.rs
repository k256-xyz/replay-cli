//! `k256-replay diff <SIGNATURE> [--json] [--bytes]` — the killer command.
//!
//! Fetch the state-diff fixture the validator captured for one tx and
//! print a colored, terminal-fit table mirroring the web State diff tab.
//! `--json` dumps the raw wire fixture for piping into `jq`. `--bytes`
//! prints the base64 pre/post for every captured row.

use anyhow::{anyhow, Result};
use owo_colors::OwoColorize;

use crate::client::{ApiError, ReplayClient};
use crate::output::{diff_table, execution_line, fmt_u64_str, header_line, short_pubkey};
use crate::types::Fixture;
use crate::{api_status_to_code, PresentedExit};

pub fn run(
    client: &ReplayClient,
    signature: &str,
    json: bool,
    bytes: bool,
    colour: bool,
) -> Result<()> {
    validate_signature(signature)?;

    let fixture = match client.fixture_get(signature) {
        Ok(f) => f,
        Err(e) => {
            // Friendly 404 — fixtures get evicted, vote txs are skipped,
            // and a fresh boot wipes the cache. Tell the user what's
            // most likely, not just the bare error code.
            if let Some(api) = e.downcast_ref::<ApiError>() {
                if api.code.as_deref() == Some("fixture_not_found") {
                    eprintln!(
                        "no fixture for {} in this replay session.",
                        short_pubkey(signature)
                    );
                    eprintln!();
                    eprintln!("most likely cause:");
                    eprintln!("  • the transaction is a vote (skipped on purpose)");
                    eprintln!("  • the fork hasn't advanced past that slot yet");
                    eprintln!("  • the fixture was evicted (FIFO cap — see `k256-replay fixtures stats`)");
                    eprintln!("  • someone just called `k256-replay fixtures clear`");
                    eprintln!();
                    eprintln!("inspect cache health:");
                    eprintln!("  k256-replay fixtures stats");
                    return Err(anyhow!(PresentedExit {
                        code: api_status_to_code(api.status.as_u16())
                    }));
                }
            }
            return Err(e);
        }
    };

    if json {
        // `Fixture` derives `Serialize`, so the wire shape round-trips
        // verbatim — `--json | jq` is interchangeable with hitting
        // GET /fixtures/tx/:sig directly.
        println!("{}", serde_json::to_string_pretty(&fixture)?);
        return Ok(());
    }

    // Header: signature, slot, CU, fee.
    if colour {
        println!("{}", header_line(&fixture).bold());
    } else {
        println!("{}", header_line(&fixture));
    }
    println!("{}", execution_line(&fixture.execution, &fixture.accounts, colour));
    println!();
    println!("{}", diff_table(&fixture.accounts, colour));

    // The table truncates pubkeys to fit a terminal width. For the
    // operator's next step (`patch apply --pubkey <X>` against an
    // account that changed) the truncated form is unusable, so dump
    // the full base58 of every changed-or-readonly-invoked row below
    // the table. Quiet when no rows changed.
    let changed: Vec<&_> = fixture
        .accounts
        .iter()
        .filter(|r| r.changed == Some(true) || !r.changed_fields.is_empty())
        .collect();
    if !changed.is_empty() {
        println!();
        let label = if colour {
            "changed accounts (full pubkey, copy-paste-ready):"
                .dimmed()
                .to_string()
        } else {
            "changed accounts (full pubkey, copy-paste-ready):".to_string()
        };
        println!("{label}");
        for row in changed {
            println!("  {}", row.pubkey);
        }
    }

    if bytes {
        print_bytes(&fixture, colour);
    } else if fixture
        .accounts
        .iter()
        .any(|r| r.data.pre_base64.is_some() || r.data.post_base64.is_some())
    {
        let hint = "(run with --bytes to print pre/post base64 for every changed row.)";
        println!();
        println!("{}", if colour { hint.dimmed().to_string() } else { hint.to_string() });
    }

    if fixture.cache.fixture_truncated {
        println!();
        println!(
            "note: this fixture is truncated — inline bytes stripped because the total exceeded the 16 MiB per-fixture cap. Account data {} byte cap still applies per-side.",
            fmt_u64_str(&fixture.cache.inline_account_data_max_bytes.to_string()),
        );
    }
    Ok(())
}

fn validate_signature(s: &str) -> Result<()> {
    // A Solana signature is 64 bytes; base58 round-trips. We don't bound
    // the input string length here because the edge case of an all-zero
    // 64-byte signature is exactly 64 chars in base58, while typical
    // signatures are 87-88. The decode + length check below covers both
    // and rejects every malformed input we care about.
    let decoded = bs58::decode(s)
        .into_vec()
        .map_err(|e| anyhow!("signature is not valid base58: {e}"))?;
    if decoded.len() != 64 {
        return Err(anyhow!(
            "signature decoded to {} bytes; Solana signatures are 64 bytes",
            decoded.len()
        ));
    }
    Ok(())
}

fn print_bytes(fixture: &Fixture, colour: bool) {
    let label = |s: &str| {
        if colour { s.dimmed().to_string() } else { s.to_string() }
    };
    let mut printed_any = false;
    for row in &fixture.accounts {
        if row.data.pre_base64.is_none() && row.data.post_base64.is_none() {
            continue;
        }
        if !printed_any {
            println!();
            printed_any = true;
        }
        println!("{} #{} {}", label("row"), row.index, short_pubkey(&row.pubkey));
        if let Some(pre) = row.data.pre_base64.as_deref() {
            println!("  {} {}", label("pre "), pre);
        }
        if let Some(post) = row.data.post_base64.as_deref() {
            println!("  {} {}", label("post"), post);
        }
        if let Some(reason) = row.data.omitted_reason.as_deref() {
            println!("  {} omitted: {reason}", label("note"));
        }
    }
}

