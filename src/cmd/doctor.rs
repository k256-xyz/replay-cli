//! `k256-replay doctor` — single pre-flight catching the "why isn't this
//! working" causes that bite first-time users (and CI runners).
//!
//! Checks are intentionally additive: a failing one prints an actionable
//! hint but doesn't abort the remaining ones. We exit non-zero only when
//! *any* check failed, so `k256-replay doctor && ./recipes/02-…` is a
//! valid CI gate.
//!
//! Same shape as the prereq logic that lives in
//! `examples/recipes/00-prereqs.sh`; both share the same intent. The
//! script stays for users who want to read what the check does; this
//! command is the operator-facing entry point.

use anyhow::Result;
use owo_colors::OwoColorize;
use serde_json::Value;

use crate::client::ReplayClient;

pub fn run(client: &ReplayClient, endpoint: &str, key: &str, colour: bool) -> Result<()> {
    let ok = |s: &str| {
        if colour {
            format!("  {} {s}", "✓".green())
        } else {
            format!("  ✓ {s}")
        }
    };
    let fail = |s: &str| {
        if colour {
            format!("  {} {s}", "✗".red())
        } else {
            format!("  ✗ {s}")
        }
    };
    let warn = |s: &str| {
        if colour {
            format!("  {} {s}", "!".yellow())
        } else {
            format!("  ! {s}")
        }
    };
    let hint = |s: &str| {
        if colour {
            format!("      {}", s.dimmed())
        } else {
            format!("      {s}")
        }
    };
    let section = |s: &str| {
        if colour {
            format!("{}", s.cyan().bold())
        } else {
            s.to_string()
        }
    };

    let mut failures = 0usize;

    println!("{}", section("environment"));
    // 1) PATH binaries that the recipes depend on (and many manual
    //    workflows). These are user-side; the CLI itself doesn't need
    //    them, but `examples/recipes/*` do.
    for bin in ["solana", "solana-keygen", "jq", "curl"] {
        if let Some(p) = which(bin) {
            println!("{}", ok(&format!("{bin:<14} {}", p)));
        } else {
            failures += 1;
            println!("{}", fail(&format!("{bin:<14} not found on PATH")));
            println!(
                "{}",
                hint(match bin {
                    "solana" | "solana-keygen" =>
                        "install: https://docs.anza.xyz/cli/install/  (needed by patch / RPC verify)",
                    "jq" => "install: brew install jq  (needed by recipes 01-04 for parsing JSON)",
                    "curl" => "install: install curl from your package manager",
                    _ => "",
                })
            );
        }
    }

    println!();
    println!("{}", section("CLI configuration"));

    // 2) Endpoint sanity — should be the managed gateway, not a box IP.
    println!("{}", ok(&format!("endpoint       {endpoint}")));
    if !endpoint.contains("api-replay.k256.xyz") {
        println!(
            "{}",
            warn(&format!("endpoint is not the managed gateway"))
        );
        println!(
            "{}",
            hint("set REPLAY_ENDPOINT=https://api-replay.k256.xyz or pass --endpoint")
        );
    }

    // 3) API key — must be set + must start with `rpl_live_`.
    if key.is_empty() {
        failures += 1;
        println!("{}", fail("REPLAY_API_KEY    unset"));
        println!(
            "{}",
            hint("grab it from https://app.k256.xyz/app/replay/<server-id>/access")
        );
    } else if !key.starts_with("rpl_") {
        failures += 1;
        println!("{}", fail(&format!("REPLAY_API_KEY    looks malformed (\"{}…\")", &key[..key.len().min(8)])));
        println!(
            "{}",
            hint("Replay bearers start with `rpl_live_…` — copy it from the Access page")
        );
    } else {
        let redacted = format!(
            "{}…{}",
            &key[..key.len().min(12)],
            &key[key.len().saturating_sub(4)..]
        );
        println!("{}", ok(&format!("REPLAY_API_KEY    {redacted}")));
    }

    println!();
    println!("{}", section("gateway connectivity"));

    // 4) /status round-trip. This combines auth + endpoint + box reachability.
    match client.status_json() {
        Ok(v) => {
            let phase = v.get("phase").and_then(Value::as_str).unwrap_or("?");
            let slot = v.get("current_slot").map(|s| s.to_string()).unwrap_or_else(|| "?".into());
            println!("{}", ok(&format!("/status            phase={phase}  slot={slot}")));

            // 4a) Disk-pressure surface — only on orchestrators that
            //     publish it. Warn if pressure is high; fail if a write
            //     would already be refused.
            if let Some(disk) = v.get("disk").filter(|d| !d.is_null()) {
                let pct = disk.get("pct_used").and_then(Value::as_f64).unwrap_or(0.0);
                let refuse = disk
                    .get("refuse_new_checkpoint")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if refuse {
                    failures += 1;
                    println!(
                        "{}",
                        fail(&format!(
                            "host disk          {:.1}% used — /checkpoint and /boot will 507",
                            pct
                        ))
                    );
                    println!(
                        "{}",
                        hint("free space: `k256-replay checkpoint prune --keep-latest <N> --yes`")
                    );
                } else if pct > 80.0 {
                    println!(
                        "{}",
                        warn(&format!("host disk          {:.1}% used (advisory)", pct))
                    );
                    println!(
                        "{}",
                        hint("nothing's broken yet — consider pruning before pressure forces a refusal")
                    );
                } else {
                    println!("{}", ok(&format!("host disk          {:.1}% used", pct)));
                }
            }

            // 4b) Phase sanity — if the box is dead the rest of the recipes
            //     can't work.
            if phase == "dead" {
                failures += 1;
                println!("{}", fail("box phase is `dead` — needs /boot before any workflow"));
                println!(
                    "{}",
                    hint("from the dashboard: pick a snapshot and click `Boot validator`")
                );
            } else if phase == "idle" {
                println!("{}", warn("box phase is `idle` — no validator running; /boot first"));
            }
        }
        Err(e) => {
            failures += 1;
            println!("{}", fail(&format!("/status            {e}")));
            println!(
                "{}",
                hint("if you see 401: REPLAY_API_KEY doesn't match the box. 5xx: see the dashboard.")
            );
        }
    }

    println!();
    if failures == 0 {
        let msg = "all checks passed. The recipes in examples/recipes/ are good to go.";
        println!("{}", if colour { msg.green().bold().to_string() } else { msg.to_string() });
        Ok(())
    } else {
        let msg = format!("{failures} check(s) failed. Fix the items above before running recipes.");
        eprintln!("{}", if colour { msg.red().bold().to_string() } else { msg });
        std::process::exit(1);
    }
}

/// Resolve a binary name against PATH. Same semantics as the shell
/// `command -v` builtin — returns the absolute path or None.
fn which(name: &str) -> Option<String> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(name);
            if candidate.is_file()
                && std::fs::metadata(&candidate)
                    .map(|m| {
                        use std::os::unix::fs::PermissionsExt;
                        m.permissions().mode() & 0o111 != 0
                    })
                    .unwrap_or(false)
            {
                Some(candidate.to_string_lossy().into_owned())
            } else {
                None
            }
        })
    })
}
