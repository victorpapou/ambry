//! Box model and on-disk storage for Ambry.
//!
//! A box is a canvas of items that point at files and folders on disk.
//! The prototype keeps one box per JSON file; SQLite comes later.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// Horizontal distance between items dropped together.
pub const DROP_STEP_X: f32 = 120.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    File,
    Folder,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: u64,
    pub path: PathBuf,
    pub kind: ItemKind,
    /// Top-left corner of the card in canvas coordinates.
    pub x: f32,
    pub y: f32,
}

impl Item {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// Short label shown on the card, like ".md" or "folder".
    pub fn badge(&self) -> String {
        match self.kind {
            ItemKind::Folder => "folder".into(),
            ItemKind::File => self
                .path
                .extension()
                .map(|ext| format!(".{}", ext.to_string_lossy().to_lowercase()))
                .unwrap_or_else(|| "file".into()),
        }
    }

    pub fn is_image(&self) -> bool {
        matches!(
            self.badge().as_str(),
            ".png" | ".jpg" | ".jpeg" | ".gif" | ".webp" | ".bmp"
        )
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BoxDoc {
    pub name: String,
    pub items: Vec<Item>,
    next_id: u64,
}

impl BoxDoc {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Adds paths as items laid out in a row starting at `(x, y)`.
    /// Paths already in the box are skipped. Returns the ids of new items.
    pub fn add_paths(&mut self, paths: &[PathBuf], x: f32, y: f32) -> Vec<u64> {
        let mut added = Vec::new();
        for path in paths {
            if self.items.iter().any(|item| &item.path == path) {
                continue;
            }
            let kind = if path.is_dir() {
                ItemKind::Folder
            } else {
                ItemKind::File
            };
            self.next_id += 1;
            self.items.push(Item {
                id: self.next_id,
                path: path.clone(),
                kind,
                x: x + added.len() as f32 * DROP_STEP_X,
                y,
            });
            added.push(self.next_id);
        }
        added
    }

    pub fn item(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|item| item.id == id)
    }

    pub fn move_item(&mut self, id: u64, x: f32, y: f32) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.x = x;
            item.y = y;
        }
    }

    pub fn remove(&mut self, id: u64) {
        self.items.retain(|item| item.id != id);
    }
}

/// Reads and writes a box as pretty JSON.
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `~/Library/Application Support/Ambry/boxes/<file_name>.json` on macOS.
    pub fn in_data_dir(file_name: &str) -> Result<Self> {
        let dir = dirs::data_dir()
            .context("no data directory on this system")?
            .join("Ambry")
            .join("boxes");
        Ok(Self::new(dir.join(format!("{file_name}.json"))))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the box, or returns a new empty one named `default_name` if none is saved yet.
    pub fn load_or_new(&self, default_name: &str) -> Result<BoxDoc> {
        if !self.path.exists() {
            return Ok(BoxDoc::new(default_name));
        }
        let json = fs::read_to_string(&self.path)
            .with_context(|| format!("reading {}", self.path.display()))?;
        serde_json::from_str(&json).with_context(|| format!("parsing {}", self.path.display()))
    }

    pub fn save(&self, doc: &BoxDoc) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(doc)?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_paths_lays_out_a_row_and_skips_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ambry.md");
        fs::write(&file, "# hi").unwrap();
        let folder = dir.path().join("assets");
        fs::create_dir(&folder).unwrap();

        let mut doc = BoxDoc::new("announcement");
        let added = doc.add_paths(&[file.clone(), folder.clone()], 10.0, 20.0);
        assert_eq!(added, vec![1, 2]);
        assert_eq!(doc.item(1).unwrap().badge(), ".md");
        assert_eq!(doc.item(2).unwrap().kind, ItemKind::Folder);
        assert_eq!(doc.item(2).unwrap().x, 10.0 + DROP_STEP_X);

        assert!(doc.add_paths(&[file], 0.0, 0.0).is_empty());
        assert_eq!(doc.items.len(), 2);
    }

    #[test]
    fn store_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("boxes/inbox.json"));
        let mut doc = store.load_or_new("Inbox").unwrap();
        assert_eq!(doc.name, "Inbox");

        doc.add_paths(&[dir.path().join("photo.PNG")], 0.0, 0.0);
        doc.move_item(1, 50.0, 60.0);
        store.save(&doc).unwrap();

        let loaded = store.load_or_new("ignored").unwrap();
        assert_eq!(loaded, doc);
        assert!(loaded.item(1).unwrap().is_image());

        let mut loaded = loaded;
        loaded.remove(1);
        assert!(loaded.items.is_empty());
        // Ids keep growing after a removal.
        assert_eq!(loaded.add_paths(&[dir.path().join("b")], 0.0, 0.0), vec![2]);
    }
}
