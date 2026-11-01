//! Services Ambry reads context from: remote MCP servers (Slack, Figma, Notion…) and the
//! Telegram bot. The list lives in `connections.json` in the data folder; tokens never do,
//! they go to the keychain.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use crate::write_atomic;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A remote MCP server over Streamable HTTP, signed in with OAuth.
    Mcp,
    /// Ambry's own Telegram bot, with a token from @BotFather.
    Telegram,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    /// Stable and unique, like `slack`; also the keychain account for its tokens.
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// The MCP server's endpoint; empty for Telegram.
    #[serde(default)]
    pub url: String,
    /// An SF Symbol for the settings list.
    #[serde(default)]
    pub symbol: String,
    /// Whether people turned it on. Signing in is separate: a token in the keychain.
    #[serde(default)]
    pub enabled: bool,
}

/// Services with official remote MCP servers, offered before anyone adds their own.
pub fn presets() -> Vec<Connection> {
    let mcp = |id: &str, name: &str, url: &str, symbol: &str| Connection {
        id: id.into(),
        name: name.into(),
        kind: Kind::Mcp,
        url: url.into(),
        symbol: symbol.into(),
        enabled: false,
    };
    vec![
        Connection {
            id: "telegram".into(),
            name: "Telegram".into(),
            kind: Kind::Telegram,
            url: String::new(),
            symbol: "paperplane".into(),
            enabled: false,
        },
        mcp("slack", "Slack", "https://mcp.slack.com/mcp", "number"),
        mcp("figma", "Figma", "https://mcp.figma.com/mcp", "pencil.and.outline"),
        mcp("notion", "Notion", "https://mcp.notion.com/mcp", "doc.text"),
        mcp(
            "github",
            "GitHub",
            "https://api.githubcopilot.com/mcp/",
            "chevron.left.forwardslash.chevron.right",
        ),
        mcp("linear", "Linear", "https://mcp.linear.app/mcp", "line.3.horizontal"),
    ]
}

/// The saved list, kept in `<data dir>/connections.json`.
pub struct Connections {
    path: PathBuf,
    pub list: Vec<Connection>,
}

impl Connections {
    /// Loads the list in `data_dir`, with any preset it does not have yet added at its place.
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("connections.json");
        let mut list: Vec<Connection> = fs::read_to_string(&path)
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        for (index, preset) in presets().into_iter().enumerate() {
            if !list.iter().any(|saved| saved.id == preset.id) {
                list.insert(index.min(list.len()), preset);
            }
        }
        Self { path, list }
    }

    pub fn save(&self) -> Result<()> {
        write_atomic(&self.path, serde_json::to_string_pretty(&self.list)?.as_bytes())
            .with_context(|| format!("saving {}", self.path.display()))
    }

    pub fn get(&self, id: &str) -> Option<&Connection> {
        self.list.iter().find(|connection| connection.id == id)
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        if let Some(connection) = self.list.iter_mut().find(|connection| connection.id == id) {
            connection.enabled = enabled;
        }
    }

    /// Adds a remote MCP server people found themselves. Returns its id.
    pub fn add_mcp(&mut self, name: &str, url: &str) -> Result<String> {
        let name = name.trim();
        let url = url.trim();
        if name.is_empty() {
            bail!("give the server a name");
        }
        if !(url.starts_with("https://") || url.starts_with("http://localhost")
            || url.starts_with("http://127.0.0.1"))
        {
            bail!("the address must start with https://");
        }
        let stem: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let stem = if stem.trim_matches('-').is_empty() {
            "server".to_string()
        } else {
            stem
        };
        let mut id = stem.clone();
        let mut n = 2;
        while self.get(&id).is_some() {
            id = format!("{stem}-{n}");
            n += 1;
        }
        self.list.push(Connection {
            id: id.clone(),
            name: name.into(),
            kind: Kind::Mcp,
            url: url.into(),
            symbol: "server.rack".into(),
            enabled: true,
        });
        Ok(id)
    }

    /// Removes a server people added; presets stay and can only be turned off.
    pub fn remove(&mut self, id: &str) {
        if presets().iter().any(|preset| preset.id == id) {
            self.set_enabled(id, false);
        } else {
            self.list.retain(|connection| connection.id != id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_presets_and_added_servers() {
        let dir = tempfile::tempdir().unwrap();
        let mut connections = Connections::load(dir.path());
        assert_eq!(connections.list.len(), presets().len());
        connections.set_enabled("slack", true);
        let id = connections.add_mcp("Our Wiki", "https://wiki.example.com/mcp").unwrap();
        assert_eq!(id, "our-wiki");
        assert_eq!(
            connections.add_mcp("Our Wiki", "https://x.example.com/mcp").unwrap(),
            "our-wiki-2"
        );
        assert!(connections.add_mcp("Plain", "http://example.com").is_err());
        connections.save().unwrap();

        let mut loaded = Connections::load(dir.path());
        assert!(loaded.get("slack").unwrap().enabled);
        assert_eq!(loaded.get("our-wiki").unwrap().url, "https://wiki.example.com/mcp");
        loaded.remove("our-wiki");
        loaded.remove("figma");
        assert!(loaded.get("our-wiki").is_none());
        assert!(loaded.get("figma").is_some());
    }
}
