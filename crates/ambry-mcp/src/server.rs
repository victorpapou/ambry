//! The MCP side: JSON-RPC messages in, answers out. Transport-free, so tests drive it directly.

use std::path::PathBuf;

use ambry_core::access::{self, Read};
use ambry_core::memory::Memory;
use ambry_core::{BoxDoc, Store};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

/// Protocol versions this server speaks, newest first.
const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "This server is one Ambry box: files a person collected for a \
project, plus a memory you share with them. Call box_overview first; it returns the box's \
rules (AGENTS.md) and an index of every file, note and skill. Read files with read_item and \
find text with search. When you learn something the next session would need (a decision, a \
preference, how something is done), save it with memory_write as a short Markdown note, one \
note per subject, and fix an outdated note with memory_edit instead of adding another. The \
person sees and can undo every change you make to memory.";

pub struct Server {
    store: Store,
    /// Who is connected, from `initialize`, for the journal.
    client: String,
}

impl Server {
    pub fn new(root: PathBuf) -> Self {
        Self {
            store: Store::new(root),
            client: "agent".into(),
        }
    }

    fn root(&self) -> PathBuf {
        self.store.path().to_path_buf()
    }

    /// The box as it is now: the app may have changed it since the last call.
    fn doc(&self) -> BoxDoc {
        let fallback = self
            .store
            .path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.store
            .load_or_new(&fallback)
            .unwrap_or_else(|_| BoxDoc::new(fallback))
    }

    /// Answers one message; notifications get no answer.
    pub fn handle(&mut self, message: &Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let params = message.get("params").cloned().unwrap_or(json!({}));
        let id = id?;
        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => Ok(self.call(&params)),
            _ => {
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("no method {method}") },
                }));
            }
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err::<Value, anyhow::Error>(err) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32603, "message": err.to_string() },
            }),
        })
    }

    fn initialize(&mut self, params: &Value) -> Value {
        if let Some(name) = params.pointer("/clientInfo/name").and_then(Value::as_str) {
            self.client = format!("agent:{name}");
        }
        let asked = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(VERSIONS[0]);
        let version = VERSIONS
            .iter()
            .find(|version| **version == asked)
            .unwrap_or(&VERSIONS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "ambry", "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS,
        })
    }

    fn call(&self, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        match self.run(name, &args) {
            Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
            Err(err) => json!({
                "content": [{ "type": "text", "text": format!("{err:#}") }],
                "isError": true,
            }),
        }
    }

    fn run(&self, name: &str, args: &Value) -> Result<String> {
        let root = self.root();
        let memory = Memory::new(&root);
        let text = |key: &str| -> Result<String> {
            args.get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| anyhow!("missing argument {key}"))
        };
        match name {
            "box_overview" => Ok(access::overview(&root, &self.doc())),
            "read_item" => {
                let read = access::read(&root, &self.doc(), &text("path")?)?;
                Ok(match read {
                    Read::Text {
                        path,
                        text,
                        truncated,
                    } => {
                        let more = if truncated {
                            format!("\n\n[cut at {} KB]", access::MAX_READ / 1024)
                        } else {
                            String::new()
                        };
                        format!("{}\n\n{text}{more}", path.display())
                    }
                    Read::Folder { path, entries } => {
                        format!("{}/\n{}", path.display(), entries.join("\n"))
                    }
                    Read::Binary { path, bytes } => format!(
                        "{} is not text ({bytes} bytes). Ask the person to describe it.",
                        path.display()
                    ),
                })
            }
            "search" => {
                let hits = access::search(&root, &self.doc(), &text("query")?);
                if hits.is_empty() {
                    return Ok("No matches.".into());
                }
                let mut out: Vec<String> = hits
                    .iter()
                    .map(|hit| format!("{}:{}: {}", hit.path.display(), hit.line, hit.text))
                    .collect();
                if hits.len() >= access::MAX_HITS {
                    out.push(format!("[first {} matches only]", access::MAX_HITS));
                }
                Ok(out.join("\n"))
            }
            "memory_view" => {
                let notes = memory.list();
                if notes.is_empty() {
                    return Ok("No notes yet.".into());
                }
                Ok(notes
                    .iter()
                    .map(|note| format!("{} ({} bytes): {}", note.path, note.bytes, note.title))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            "memory_read" => memory.read(&text("path")?),
            "memory_write" => {
                let path = text("path")?;
                memory.write(&path, &text("content")?, &self.client)?;
                self.refresh();
                Ok(format!("Saved memory/{path}."))
            }
            "memory_edit" => {
                let path = text("path")?;
                memory.edit(&path, &text("old_text")?, &text("new_text")?, &self.client)?;
                self.refresh();
                Ok(format!("Edited memory/{path}."))
            }
            "memory_delete" => {
                let path = text("path")?;
                memory.delete(&path, &self.client)?;
                self.refresh();
                Ok(format!("Deleted memory/{path}. The person can restore it."))
            }
            "skill_read" => {
                let skill = text("name")?;
                if skill.contains(['/', '\\']) || skill.starts_with('.') {
                    return Err(anyhow!("{skill:?} is not a skill name"));
                }
                std::fs::read_to_string(root.join("skills").join(&skill).join("SKILL.md"))
                    .map_err(|_| anyhow!("no skill {skill}; box_overview lists them"))
            }
            _ => Err(anyhow!("no tool {name}")),
        }
    }

    /// Keeps CONTEXT.md listing the notes.
    fn refresh(&self) {
        let _ = self.store.refresh_context(&self.doc());
    }
}

fn tools() -> Value {
    let read_only = json!({ "readOnlyHint": true, "openWorldHint": false });
    let writes = json!({ "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false });
    let path = json!({
        "type": "string",
        "description": "Note path inside memory/, like deploy.md or team/rules.md",
    });
    json!([
        {
            "name": "box_overview",
            "title": "Box overview",
            "description": "The box's rules (AGENTS.md) and an index of its files, memory notes, skills and sources. Call this first.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_only,
        },
        {
            "name": "read_item",
            "title": "Read a file",
            "description": "Reads a file or lists a folder in the box: an absolute path from the index, or a path relative to the box like AGENTS.md or sources/slack/general.md. Text is cut at 256 KB.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            },
            "annotations": read_only,
        },
        {
            "name": "search",
            "title": "Search the box",
            "description": "Finds lines containing the text, ignoring case, in the box's files, folders, memory, skills and sources. Returns path:line: text.",
            "inputSchema": {
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"],
            },
            "annotations": read_only,
        },
        {
            "name": "memory_view",
            "title": "List memory",
            "description": "Lists the notes in the box's memory with their titles.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_only,
        },
        {
            "name": "memory_read",
            "title": "Read a note",
            "description": "Reads one memory note.",
            "inputSchema": { "type": "object", "properties": { "path": path }, "required": ["path"] },
            "annotations": read_only,
        },
        {
            "name": "memory_write",
            "title": "Write a note",
            "description": "Creates a memory note or replaces all of it. Markdown, one subject per note, at most 64 KB. Start with a # heading that says what the note is about.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": path, "content": { "type": "string" } },
                "required": ["path", "content"],
            },
            "annotations": writes,
        },
        {
            "name": "memory_edit",
            "title": "Edit a note",
            "description": "Replaces old_text, which must appear exactly once, with new_text in a memory note.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": path,
                    "old_text": { "type": "string" },
                    "new_text": { "type": "string" },
                },
                "required": ["path", "old_text", "new_text"],
            },
            "annotations": writes,
        },
        {
            "name": "memory_delete",
            "title": "Delete a note",
            "description": "Deletes a memory note that is wrong or no longer needed. A copy is kept so the person can restore it.",
            "inputSchema": { "type": "object", "properties": { "path": path }, "required": ["path"] },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "openWorldHint": false },
        },
        {
            "name": "skill_read",
            "title": "Read a skill",
            "description": "Reads skills/<name>/SKILL.md, a procedure for this box.",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"],
            },
            "annotations": read_only,
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(server: &mut Server, id: u64, method: &str, params: Value) -> Value {
        server
            .handle(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .unwrap()
    }

    fn tool_text(answer: &Value) -> &str {
        answer["result"]["content"][0]["text"].as_str().unwrap()
    }

    #[test]
    fn speaks_mcp_and_keeps_memory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("brief.md");
        std::fs::write(&file, "# Brief\nShip on Friday\n").unwrap();
        let root = dir.path().join("Launch");
        let mut doc = BoxDoc::new("Launch");
        doc.add_paths(&[file.clone()], 0., 0.);
        Store::new(&root).save(&doc).unwrap();

        let mut server = Server::new(root.clone());
        let init = call(
            &mut server,
            1,
            "initialize",
            json!({ "protocolVersion": "2025-03-26", "clientInfo": { "name": "claude-code" } }),
        );
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(
            server
                .handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
                .is_none()
        );

        let list = call(&mut server, 2, "tools/list", json!({}));
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 9);

        let overview = call(
            &mut server,
            3,
            "tools/call",
            json!({ "name": "box_overview" }),
        );
        assert!(tool_text(&overview).contains("brief.md"));

        let read = call(
            &mut server,
            4,
            "tools/call",
            json!({ "name": "read_item", "arguments": { "path": file } }),
        );
        assert!(tool_text(&read).contains("Ship on Friday"));

        let wrote = call(
            &mut server,
            5,
            "tools/call",
            json!({ "name": "memory_write", "arguments": { "path": "launch.md", "content": "# Launch\nFriday\n" } }),
        );
        assert!(wrote["result"].get("isError").is_none(), "{wrote}");
        let journal = Memory::new(&root).journal();
        assert_eq!(journal[0].actor, "agent:claude-code");
        assert!(
            std::fs::read_to_string(root.join("CONTEXT.md"))
                .unwrap()
                .contains("memory/launch.md")
        );

        let escape = call(
            &mut server,
            6,
            "tools/call",
            json!({ "name": "memory_write", "arguments": { "path": "../AGENTS.md", "content": "x" } }),
        );
        assert_eq!(escape["result"]["isError"], true);

        let found = call(
            &mut server,
            7,
            "tools/call",
            json!({ "name": "search", "arguments": { "query": "friday" } }),
        );
        assert!(tool_text(&found).contains("launch.md:2"));

        let missing = call(&mut server, 8, "resources/list", json!({}));
        assert_eq!(missing["error"]["code"], -32601);
    }
}
