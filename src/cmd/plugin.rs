//! `k256-replay plugin {list,upload,delete}`
//!
//! Custom Geyser plugin management — wraps the orchestrator's
//! `/plugin/{list,upload,delete}` routes. Plugins must be
//! ABI-compatible with the running validator: `x86_64-unknown-linux-gnu`,
//! glibc 2.39, bound to `0.0.0.0:10000` for external traffic. The
//! orchestrator rewrites the uploaded config's `libpath` to where the
//! `.so` was just written, so the operator doesn't need to know the
//! guest's filesystem layout.
//!
//! On a successful upload, pass the returned `config_path` as the
//! `geyser` field on the next `/boot` to make the validator load it.

use anyhow::{anyhow, Result};
use comfy_table::{presets::UTF8_BORDERS_ONLY, ContentArrangement, Table};
use humansize::{format_size, DECIMAL};
use owo_colors::OwoColorize;

use crate::client::ReplayClient;
use crate::types::PluginUploadResponse;
use crate::PluginAction;

pub fn run(client: &ReplayClient, action: PluginAction, colour: bool) -> Result<()> {
    match action {
        PluginAction::List { json } => list(client, json, colour),
        PluginAction::Upload { lib, config } => upload(client, &lib, &config, colour),
        PluginAction::Delete { id, yes } => delete(client, &id, yes, colour),
    }
}

fn list(client: &ReplayClient, json: bool, colour: bool) -> Result<()> {
    let resp = client.plugin_list()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }
    let dim = |s: &str| {
        if colour {
            s.dimmed().to_string()
        } else {
            s.to_string()
        }
    };
    let bold = |s: &str| {
        if colour {
            s.bold().to_string()
        } else {
            s.to_string()
        }
    };

    if let Some(active) = resp.active.as_ref() {
        let kind_paint = if colour {
            match active.kind.as_str() {
                "bundled" => active.kind.dimmed().to_string(),
                "uploaded" => active.kind.cyan().to_string(),
                _ => active.kind.yellow().to_string(),
            }
        } else {
            active.kind.clone()
        };
        println!(
            "active   {} ({})   {}",
            bold(&active.name),
            kind_paint,
            dim(&active.config_path)
        );
    } else {
        println!("active   {}", dim("(no plugin loaded)"));
    }

    if resp.plugins.is_empty() {
        println!();
        let hint = "upload a custom plugin: k256-replay plugin upload --lib X.so --config X.json";
        println!("{}", dim(hint));
        return Ok(());
    }

    let mut t = Table::new();
    t.load_preset(UTF8_BORDERS_ONLY)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["id", "name", "size", "uploaded", "config_path"]);
    for p in &resp.plugins {
        t.add_row(vec![
            p.id.clone(),
            p.name.clone(),
            format_size(p.size_bytes, DECIMAL),
            p.uploaded_unix.to_string(),
            p.config_path.clone(),
        ]);
    }
    println!();
    println!("{t}");
    Ok(())
}

fn upload(
    client: &ReplayClient,
    lib: &std::path::Path,
    config: &std::path::Path,
    colour: bool,
) -> Result<()> {
    if !lib.exists() {
        return Err(anyhow!("--lib path does not exist: {}", lib.display()));
    }
    if !config.exists() {
        return Err(anyhow!("--config path does not exist: {}", config.display()));
    }
    let resp: PluginUploadResponse = client.plugin_upload(lib, config)?;
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
    println!("uploaded  id {}   name {}", bold(&resp.id), bold(&resp.name));
    println!("  config_path  {}", resp.config_path);
    println!("  lib_path     {}", dim(&resp.lib_path));
    println!();
    let hint = format!(
        "boot with this plugin: POST /boot {{ snapshot_slot: N, geyser: \"{}\" }}",
        resp.config_path
    );
    println!("{}", dim(&hint));
    println!(
        "{}",
        dim("(/boot is console-side because it needs catalog auth; CLI doesn't wrap it)")
    );
    Ok(())
}

fn delete(client: &ReplayClient, id: &str, yes: bool, colour: bool) -> Result<()> {
    if !yes {
        return Err(anyhow!(
            "refusing to delete without --yes (removes the upload's `.so` + `config.json`)"
        ));
    }
    let resp = client.plugin_delete(id)?;
    let dim = |s: &str| {
        if colour {
            s.dimmed().to_string()
        } else {
            s.to_string()
        }
    };
    if resp.deleted {
        println!("deleted   id {id}");
    } else {
        println!(
            "delete returned deleted=false{}",
            resp.message
                .as_deref()
                .map(|m| format!("   {}", dim(m)))
                .unwrap_or_default()
        );
    }
    Ok(())
}
