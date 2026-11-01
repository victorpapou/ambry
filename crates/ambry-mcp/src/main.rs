//! `ambry-mcp`: one Ambry box as an MCP server over stdio, for Claude Code, Claude Desktop,
//! Codex or any other MCP client.
//!
//! ```sh
//! claude mcp add ambry -- ambry-mcp --box ~/Ambry/Inbox
//! ```
//!
//! `--box` takes the box's folder or just its name in `~/Ambry`. Without it the server uses
//! `$AMBRY_BOX`, then the Inbox.

mod server;

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

fn main() -> Result<()> {
    let root = box_folder()?;
    if !root.join(ambry_core::META_DIR).is_dir() {
        bail!(
            "{} is not an Ambry box; open it in Ambry once, or pass --box",
            root.display()
        );
    }
    let mut server = server::Server::new(root);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            // A batch, from clients on the 2025-03-26 protocol.
            Ok(Value::Array(messages)) => {
                let answers: Vec<Value> = messages
                    .iter()
                    .filter_map(|message| server.handle(message))
                    .collect();
                (!answers.is_empty()).then(|| Value::Array(answers))
            }
            Ok(message) => server.handle(&message),
            Err(err) => Some(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32700, "message": err.to_string() },
            })),
        };
        if let Some(answer) = answer {
            writeln!(stdout, "{answer}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn box_folder() -> Result<PathBuf> {
    let mut args = std::env::args().skip(1);
    let mut chosen = std::env::var_os("AMBRY_BOX").map(PathBuf::from);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--box" => chosen = Some(args.next().context("--box needs a folder")?.into()),
            "-h" | "--help" => {
                println!("usage: ambry-mcp [--box <folder or name>]");
                std::process::exit(0);
            }
            other => bail!("unknown argument {other}"),
        }
    }
    let root = ambry_core::boxes_root()?;
    Ok(match chosen {
        Some(path) if path.components().count() > 1 || path.is_absolute() => path,
        Some(name) => {
            let name = name.to_string_lossy().into_owned();
            ambry_core::list_boxes(&root)
                .into_iter()
                .find(|entry| entry.name == name)
                .map(|entry| entry.path)
                .unwrap_or_else(|| root.join(name))
        }
        None => root.join("Inbox"),
    })
}
