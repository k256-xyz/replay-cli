//! `k256-replay idl <PROGRAM_ID>`
//!
//! Wraps the gateway's public `GET /idl/<programId>` route. Reads the
//! program's on-chain Anchor IDL account from mainnet, decompresses
//! the JSON, and returns:
//!
//! ```json
//! { "program_id": "...", "idl_address": "...", "idl": { ... } }
//! ```
//!
//! Default render: a one-screen summary (name, version, IDL account,
//! instruction count, account-type count, defined-type count) plus the
//! first few names from each list. `--json` emits the canonical IDL
//! body for piping into `jq`.
//!
//! 404 means the program doesn't publish an Anchor IDL — printed as a
//! human-readable hint, not a stack trace.

use anyhow::Result;
use owo_colors::OwoColorize;
use serde_json::Value;

use crate::client::{ApiError, ReplayClient};
use crate::PresentedExit;

pub fn run(client: &ReplayClient, program_id: &str, json: bool, colour: bool) -> Result<()> {
    let body = match client.idl(program_id) {
        Ok(v) => v,
        Err(e) => {
            if let Some(api) = e.downcast_ref::<ApiError>() {
                if api.status.as_u16() == 404 {
                    print_not_found(program_id, colour);
                    return Err(PresentedExit { code: 4 }.into());
                }
            }
            return Err(e);
        }
    };

    if json {
        // The full envelope: `{ program_id, idl_address, idl }`. `jq` can
        // pluck `.idl` to get just the Anchor IDL body.
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }
    print_summary(&body, colour);
    Ok(())
}

fn print_summary(body: &Value, colour: bool) {
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

    let program_id = body
        .get("program_id")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let idl_address = body
        .get("idl_address")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let idl = body.get("idl").unwrap_or(&Value::Null);

    // Anchor IDL versions: 0.29 puts name/version at top level; 0.30 nests
    // them under `metadata`. We read both so the CLI works for either.
    let name = idl
        .get("metadata")
        .and_then(|m| m.get("name"))
        .and_then(Value::as_str)
        .or_else(|| idl.get("name").and_then(Value::as_str))
        .unwrap_or("(unnamed)");
    let version = idl
        .get("metadata")
        .and_then(|m| m.get("version"))
        .and_then(Value::as_str)
        .or_else(|| idl.get("version").and_then(Value::as_str))
        .unwrap_or("(no version)");

    let instructions = idl
        .get("instructions")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    let accounts = idl
        .get("accounts")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    let types = idl
        .get("types")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    let events = idl
        .get("events")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    let errors = idl
        .get("errors")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);

    println!("{} {}", bold("program"), program_id);
    println!("  {:<18} {}", dim("name"), name);
    println!("  {:<18} {}", dim("version"), version);
    println!("  {:<18} {}", dim("idl account"), idl_address);
    println!("  {:<18} {}", dim("instructions"), instructions);
    println!("  {:<18} {}", dim("account types"), accounts);
    println!("  {:<18} {}", dim("defined types"), types);
    if events > 0 {
        println!("  {:<18} {}", dim("events"), events);
    }
    if errors > 0 {
        println!("  {:<18} {}", dim("error codes"), errors);
    }

    // Show the first handful of names so the operator immediately sees
    // what they got. Full lists live in `--json`.
    print_name_preview("first instructions", idl.get("instructions"), colour);
    print_name_preview("first account types", idl.get("accounts"), colour);
}

fn print_name_preview(label: &str, list: Option<&Value>, colour: bool) {
    let dim = |s: &str| {
        if colour {
            s.dimmed().to_string()
        } else {
            s.to_string()
        }
    };
    let arr = match list.and_then(Value::as_array) {
        Some(a) if !a.is_empty() => a,
        _ => return,
    };
    let names: Vec<&str> = arr
        .iter()
        .take(6)
        .filter_map(|v| v.get("name").and_then(Value::as_str))
        .collect();
    if names.is_empty() {
        return;
    }
    let mut joined = names.join(", ");
    if arr.len() > names.len() {
        joined.push_str(&format!(", … (+{} more)", arr.len() - names.len()));
    }
    println!("  {:<18} {}", dim(label), joined);
}

fn print_not_found(program_id: &str, colour: bool) {
    let dim = |s: &str| {
        if colour {
            s.dimmed().to_string()
        } else {
            s.to_string()
        }
    };
    eprintln!(
        "no Anchor IDL on chain for {}.",
        if colour {
            program_id.bold().to_string()
        } else {
            program_id.to_string()
        }
    );
    eprintln!();
    eprintln!("most likely cause:");
    eprintln!("  • the program is not an Anchor program (native loaders, Phoenix, …)");
    eprintln!("  • the team hasn't run `anchor idl init` yet");
    eprintln!("  • you copied the wrong address (e.g. an Anchor binary's BPFLoader account)");
    eprintln!();
    eprintln!("{}", dim("the gateway proxies to mainnet RPC, not to your fork."));
}
