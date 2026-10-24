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

/// The glyph and color a box shows next to its name, like a shortcut in Shortcuts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxIcon {
    /// An SF Symbol name.
    pub symbol: String,
    /// One of [`BoxIcon::COLORS`].
    pub color: String,
}

impl BoxIcon {
    /// The colors Shortcuts offers for a shortcut's icon, in its order.
    pub const COLORS: [&str; 15] = [
        "red", "coral", "orange", "yellow", "green", "teal", "sky", "blue", "indigo", "violet",
        "lavender", "pink", "gray", "sage", "taupe",
    ];
    /// SF Symbols a box's icon can show, in named groups like Shortcuts' glyph picker.
    pub const SYMBOL_GROUPS: &[(&str, &[&str])] = &[
        (
            "Objects",
            &[
                "tray",
                "tray.full",
                "archivebox",
                "shippingbox",
                "folder",
                "doc",
                "doc.text",
                "book",
                "books.vertical",
                "bookmark",
                "newspaper",
                "paperclip",
                "pencil",
                "paintbrush",
                "scissors",
                "briefcase",
                "bag",
                "cart",
                "creditcard",
                "gift",
                "key",
                "lock",
                "lightbulb",
                "lamp.desk",
                "camera",
                "photo",
                "film",
                "music.note",
                "mic",
                "puzzlepiece",
                "hammer",
                "wrench.and.screwdriver",
                "graduationcap",
                "backpack",
                "umbrella",
                "tshirt",
                "eyeglasses",
                "crown",
                "trophy",
                "flag",
                "tag",
                "pin",
                "calendar",
                "clock",
                "alarm",
                "hourglass",
                "envelope",
                "paperplane",
                "phone",
                "bubble.left",
                "megaphone",
                "bell",
            ],
        ),
        (
            "People",
            &[
                "person",
                "person.2",
                "person.3",
                "figure.walk",
                "figure.run",
                "figure.wave",
                "figure.yoga",
                "figure.hiking",
                "hand.raised",
                "hand.thumbsup",
                "heart",
                "face.smiling",
                "brain.head.profile",
                "eye",
                "ear",
            ],
        ),
        (
            "Symbols",
            &[
                "star",
                "sparkles",
                "bolt",
                "flame",
                "drop",
                "sun.max",
                "moon",
                "cloud",
                "snowflake",
                "checkmark.circle",
                "xmark.circle",
                "exclamationmark.triangle",
                "questionmark.circle",
                "info.circle",
                "plus.circle",
                "number",
                "at",
                "link",
                "globe",
                "infinity",
                "square.grid.2x2",
                "chart.bar",
                "chart.pie",
                "waveform",
                "wand.and.stars",
                "atom",
                "function",
            ],
        ),
        (
            "Maps",
            &[
                "car",
                "bus",
                "tram",
                "bicycle",
                "fuelpump",
                "binoculars",
                "map",
                "location",
                "mappin",
                "house",
                "building.2",
                "airplane",
                "ferry",
                "signpost.right",
                "globe.americas",
            ],
        ),
        (
            "Devices",
            &[
                "desktopcomputer",
                "laptopcomputer",
                "keyboard",
                "printer",
                "server.rack",
                "tv",
                "gamecontroller",
                "headphones",
                "applewatch",
                "iphone",
                "ipad",
                "airpods",
                "homepod",
                "display",
                "externaldrive",
                "cpu",
            ],
        ),
        (
            "Nature",
            &[
                "leaf",
                "tree",
                "mountain.2",
                "pawprint",
                "bird",
                "fish",
                "tortoise",
                "ladybug",
                "carrot",
                "cup.and.saucer",
                "fork.knife",
                "wineglass",
                "birthday.cake",
            ],
        ),
        (
            "Health",
            &[
                "cross.case",
                "pills",
                "bandage",
                "bed.double",
                "dumbbell",
                "stethoscope",
                "lungs",
                "figure.mind.and.body",
            ],
        ),
    ];
}

impl Default for BoxIcon {
    fn default() -> Self {
        Self {
            symbol: "tray".into(),
            color: "blue".into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BoxDoc {
    pub name: String,
    #[serde(default)]
    pub icon: BoxIcon,
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

/// `~/Library/Application Support/Ambry` on macOS.
pub fn data_dir() -> Result<PathBuf> {
    Ok(dirs::data_dir()
        .context("no data directory on this system")?
        .join("Ambry"))
}

/// A saved box as the box switcher lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxEntry {
    pub path: PathBuf,
    pub name: String,
    pub icon: BoxIcon,
    /// How many items the box holds.
    pub items: usize,
}

/// Every box saved in `dir`, by name.
pub fn list_boxes(dir: &Path) -> Vec<BoxEntry> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut boxes: Vec<BoxEntry> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| {
            let doc = Store::new(&path).load_or_new("").ok()?;
            Some(BoxEntry {
                path,
                items: doc.items.len(),
                name: doc.name,
                icon: doc.icon,
            })
        })
        .collect();
    boxes.sort_by_key(|entry| entry.name.to_lowercase());
    boxes
}

/// A file in `dir` for a new box called `name` that no other box uses yet.
pub fn new_box_path(dir: &Path, name: &str) -> PathBuf {
    let stem: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let stem = if stem.is_empty() {
        "box".to_string()
    } else {
        stem
    };
    let mut path = dir.join(format!("{stem}.json"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem}-{n}.json"));
        n += 1;
    }
    path
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
        let dir = data_dir()?.join("boxes");
        Ok(Self::new(dir.join(format!("{file_name}.json"))))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The folder this box is saved in, with the other boxes.
    pub fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("."))
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
    fn lists_boxes_and_picks_free_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut work = BoxDoc::new("Work");
        work.icon.symbol = "briefcase".into();
        Store::new(dir.path().join("work.json"))
            .save(&work)
            .unwrap();
        Store::new(dir.path().join("inbox.json"))
            .save(&BoxDoc::new("Inbox"))
            .unwrap();

        let boxes = list_boxes(dir.path());
        let names: Vec<_> = boxes.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["Inbox", "Work"]);
        assert_eq!(boxes[1].icon.symbol, "briefcase");
        assert_eq!(boxes[0].icon, BoxIcon::default());

        assert_eq!(
            new_box_path(dir.path(), "New Box"),
            dir.path().join("new-box.json")
        );
        assert_eq!(
            new_box_path(dir.path(), "Work"),
            dir.path().join("work-2.json")
        );
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
