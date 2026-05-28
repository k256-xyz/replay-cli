//! Shared rendering helpers: tables, colors, formatting.
//!
//! All ANSI colouring goes through `paint` so it disables itself
//! automatically when stdout isn't a TTY or `NO_COLOR` is set. The
//! `replay diff` table renderer in `cmd::diff` consumes these helpers.

use std::io::IsTerminal;

use comfy_table::{
    presets::UTF8_BORDERS_ONLY, ContentArrangement, Table,
};
use humansize::{format_size, DECIMAL};
use owo_colors::OwoColorize;

use crate::types::{AccountDiff, AccountSnapshot, Execution, Fixture, FixtureStats};

/// Detect whether colour escapes are safe. Respects `NO_COLOR` per
/// <https://no-color.org/> and falls back to plain text whenever stdout
/// isn't a terminal (so `replay diff … | less` looks clean).
pub fn use_colour() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    std::io::stdout().is_terminal()
}

/// Short base58 in the same 4…4 style as the web State diff cell.
pub fn short_pubkey(s: &str) -> String {
    if s.len() <= 9 {
        s.to_string()
    } else {
        format!("{}…{}", &s[..4], &s[s.len() - 4..])
    }
}

/// Insert a thousands separator into a decimal digit string.
pub fn group_u64_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// Try to parse a decimal-string u64 and render with thousands; fall
/// back to the raw string if parsing fails so we never crash on data
/// we don't fully understand.
pub fn fmt_u64_str(s: &str) -> String {
    if s.bytes().all(|b| b.is_ascii_digit()) {
        group_u64_str(s)
    } else {
        s.to_string()
    }
}

fn fmt_lamports_diff(pre: Option<&AccountSnapshot>, post: Option<&AccountSnapshot>) -> String {
    match (pre, post) {
        (None, None) => "—".to_string(),
        (Some(a), Some(b)) if a.lamports == b.lamports => fmt_u64_str(&a.lamports),
        (Some(a), Some(b)) => format!("{} → {}", fmt_u64_str(&a.lamports), fmt_u64_str(&b.lamports)),
        (None, Some(b)) => format!("→ {}", fmt_u64_str(&b.lamports)),
        (Some(a), None) => format!("{} →", fmt_u64_str(&a.lamports)),
    }
}

fn fmt_owner_diff(pre: Option<&AccountSnapshot>, post: Option<&AccountSnapshot>) -> String {
    match (pre, post) {
        (None, None) => "—".to_string(),
        (Some(a), Some(b)) if a.owner == b.owner => short_pubkey(&a.owner),
        (Some(a), Some(b)) => format!("{} → {}", short_pubkey(&a.owner), short_pubkey(&b.owner)),
        (None, Some(b)) => short_pubkey(&b.owner),
        (Some(a), None) => short_pubkey(&a.owner),
    }
}

fn fmt_data(row: &AccountDiff) -> String {
    match row.data_comparison.as_str() {
        "not_available" => "not captured".to_string(),
        "unknown_too_large" => "unknown (large)".to_string(),
        "equal" => "same".to_string(),
        "length_changed" => {
            let pre = row.pre.as_ref().map(|s| s.data_len).unwrap_or(0);
            let post = row.post.as_ref().map(|s| s.data_len).unwrap_or(0);
            format!("len {pre} → {post}")
        }
        "changed" => {
            let len = row.post.as_ref().map(|s| s.data_len).unwrap_or(0);
            let trunc = if row.data.omitted_reason.as_deref() == Some("fixture_truncated") {
                " · truncated"
            } else if row.data.omitted_reason.as_deref() == Some("too_large") {
                " · hash"
            } else {
                ""
            };
            format!("len {len}{trunc}")
        }
        other => other.to_string(),
    }
}

fn fmt_role_chips(row: &AccountDiff) -> String {
    let mut chips: Vec<&str> = Vec::with_capacity(5);
    if row.fee_payer {
        chips.push("fee_payer");
    }
    if row.signer {
        chips.push("signer");
    }
    if row.declared_writable {
        chips.push("writable");
    } else {
        chips.push("readonly");
    }
    if row.invoked {
        chips.push("invoked");
    }
    if row.instruction_account {
        chips.push("instruction");
    }
    chips.join(" ")
}

fn classify(row: &AccountDiff) -> &'static str {
    if !row.declared_writable {
        "role-only"
    } else {
        match row.changed {
            Some(true) => "changed",
            Some(false) => "unchanged",
            None => "unknown",
        }
    }
}

fn paint_label(label: &str, colour: bool) -> String {
    if !colour {
        return label.to_string();
    }
    match label {
        "changed" => label.green().bold().to_string(),
        "unchanged" => label.dimmed().to_string(),
        "unknown" => label.yellow().bold().to_string(),
        "role-only" => label.dimmed().italic().to_string(),
        _ => label.to_string(),
    }
}

/// Header line: `4cLKd…pAyp · slot 422,097,261 · CU 59,737 · fee 5,000 λ`
pub fn header_line(f: &Fixture) -> String {
    let parts: Vec<String> = vec![
        short_pubkey(&f.signature),
        format!("slot {}", fmt_u64_str(&f.slot)),
        match f.execution.compute_units_consumed.as_deref() {
            Some(cu) => format!("CU {}", fmt_u64_str(cu)),
            None => "CU —".to_string(),
        },
        match f.execution.fee_lamports.as_deref() {
            Some(fee) => format!("fee {} λ", fmt_u64_str(fee)),
            None => "fee —".to_string(),
        },
    ];
    parts.join("   ")
}

pub fn execution_line(ex: &Execution, accounts: &[AccountDiff], colour: bool) -> String {
    let (mut changed, mut unchanged, mut unknown, mut role_only) = (0u32, 0u32, 0u32, 0u32);
    for row in accounts {
        match classify(row) {
            "changed" => changed += 1,
            "unchanged" => unchanged += 1,
            "unknown" => unknown += 1,
            _ => role_only += 1,
        }
    }
    let result = match ex.result.as_str() {
        "success" => if colour { "SUCCESS".green().bold().to_string() } else { "SUCCESS".to_string() },
        "executed_error" => if colour { "EXECUTED ERROR".red().bold().to_string() } else { "EXECUTED ERROR".to_string() },
        "load_error" => if colour { "LOAD ERROR".yellow().bold().to_string() } else { "LOAD ERROR".to_string() },
        other => other.to_uppercase(),
    };
    let status = ex.state_status.replace('_', " ").to_uppercase();
    let mut parts = vec![
        result,
        if colour { status.dimmed().to_string() } else { status },
        format!("changed {changed}"),
        format!("unchanged {unchanged}"),
    ];
    if unknown > 0 {
        parts.push(format!("unknown {unknown}"));
    }
    if role_only > 0 {
        parts.push(format!("role-only {role_only}"));
    }
    if let Some(err) = ex.error.as_deref() {
        parts.push(if colour {
            format!("err: {}", err.red())
        } else {
            format!("err: {err}")
        });
    }
    parts.join("   ")
}

pub fn diff_table(accounts: &[AccountDiff], colour: bool) -> Table {
    let mut t = Table::new();
    t.load_preset(UTF8_BORDERS_ONLY)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["#", "Account / roles", "Change", "Lamports", "Owner", "Data"]);

    for row in accounts {
        let label = classify(row);
        let label_painted = paint_label(label, colour);
        let pubkey_line = if colour {
            format!("{}\n{}", short_pubkey(&row.pubkey).bold(), fmt_role_chips(row).dimmed())
        } else {
            format!("{}\n{}", short_pubkey(&row.pubkey), fmt_role_chips(row))
        };
        t.add_row(vec![
            row.index.to_string(),
            pubkey_line,
            label_painted,
            fmt_lamports_diff(row.pre.as_ref(), row.post.as_ref()),
            fmt_owner_diff(row.pre.as_ref(), row.post.as_ref()),
            fmt_data(row),
        ]);
    }
    t
}

/// `cache    1,192 fixtures   3 slots   67 MB / 5 GB   evicted 0`
/// (the cap is the validator's `TX_FIXTURE_CACHE_MAX_BYTES`. Older
/// binaries shipped 1 GiB; current/post-rebuild ship 5 GiB.)
pub fn fmt_stats_line(s: &FixtureStats, colour: bool) -> String {
    let bytes = format_size(s.bytes_used, DECIMAL);
    let cap = format_size(s.bytes_cap, DECIMAL);
    let mut parts = vec![
        format!("{} fixtures", group_u64_str(&s.fixture_count.to_string())),
        format!("{} slots", s.slot_count),
        format!("{} / {}", bytes, cap),
    ];
    if let (Some(o), Some(n)) = (s.oldest_slot.as_deref(), s.newest_slot.as_deref()) {
        parts.push(format!("slots {o}..{n}"));
    }
    // Process-lifetime counters — only show when non-zero so the line
    // stays scannable on a healthy box.
    if s.evicted_count > 0 {
        parts.push(format!("evicted {}", s.evicted_count));
    }
    if s.oversized_count > 0 {
        parts.push(format!("oversized {}", s.oversized_count));
    }
    if s.cleared_count > 0 {
        parts.push(format!("cleared {}", s.cleared_count));
    }
    let label = if colour { "cache".dimmed().to_string() } else { "cache".to_string() };
    format!("{label}   {}", parts.join("   "))
}
