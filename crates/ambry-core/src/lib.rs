//! Box model and on-disk storage for Ambry.
//!
//! A box is a folder on disk that people and agents share. The items people drop on its canvas
//! are links to files and folders elsewhere; what agents need lives in the folder itself:
//!
//! ```text
//! <box>/
//!   AGENTS.md            rules and goal of the box, written by people
//!   CONTEXT.md           an index of everything in the box, kept up to date by Ambry
//!   memory/*.md          notes agents write and people correct
//!   skills/<n>/SKILL.md  procedures, "how we do things here"
//!   sources/<p>/*.md     snapshots of chats, threads and designs from connected services
//!   .ambry/box.json      name, icon, items and where they sit on the canvas
//!   .ambry/journal.jsonl every change an agent made, with a copy of what it replaced
//! ```
//!
//! Any agent can read a box without Ambry; `ambry-mcp` serves the same folder over MCP.

pub mod access;
pub mod connections;
pub mod memory;

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

/// `~/Library/Application Support/Ambry` on macOS: settings and caches, not boxes.
pub fn data_dir() -> Result<PathBuf> {
    Ok(dirs::data_dir()
        .context("no data directory on this system")?
        .join("Ambry"))
}

/// Where boxes live: `~/Ambry`, or `$AMBRY_HOME` when it is set.
pub fn boxes_root() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("AMBRY_HOME") {
        return Ok(PathBuf::from(home));
    }
    Ok(dirs::home_dir()
        .context("no home directory on this system")?
        .join("Ambry"))
}

/// The folder inside a box that only Ambry writes.
pub const META_DIR: &str = ".ambry";
pub const BOX_FILE: &str = "box.json";
pub const AGENTS_FILE: &str = "AGENTS.md";
pub const CONTEXT_FILE: &str = "CONTEXT.md";
pub const MEMORY_DIR: &str = "memory";
pub const SKILLS_DIR: &str = "skills";
pub const SOURCES_DIR: &str = "sources";

/// What a new box's AGENTS.md says until someone rewrites it.
fn agents_template(name: &str) -> String {
    format!(
        "# {name}\n\n\
         What this box is for, in a sentence or two.\n\n\
         ## Rules for agents\n\n\
         - Read CONTEXT.md first: it lists every file in this box.\n\
         - Save what you learn that the next session would need in memory/, one file per subject.\n\
         - Keep notes short and dated. Correct a note instead of adding a second one.\n\
         - Procedures live in skills/<name>/SKILL.md.\n"
    )
}

/// A saved box as the box switcher lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxEntry {
    /// The box's folder.
    pub path: PathBuf,
    pub name: String,
    pub icon: BoxIcon,
    /// How many items the box holds.
    pub items: usize,
}

/// Every box in `root`, by name.
pub fn list_boxes(root: &Path) -> Vec<BoxEntry> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut boxes: Vec<BoxEntry> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join(META_DIR).join(BOX_FILE).is_file())
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

/// A folder in `root` for a new box called `name` that no other box uses yet, named like the
/// box so people find it in Finder.
pub fn new_box_path(root: &Path, name: &str) -> PathBuf {
    let stem: String = name
        .trim()
        .chars()
        .map(|c| {
            if matches!(c, '/' | ':' | '\\') || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let stem = stem.trim_start_matches('.');
    let stem = if stem.is_empty() { "Box" } else { stem };
    let mut path = root.join(stem);
    let mut n = 2;
    while path.exists() {
        path = root.join(format!("{stem} {n}"));
        n += 1;
    }
    path
}

/// Moves boxes saved by earlier builds, one JSON file each in `old_dir`, into folders in
/// `root`. Each old file is renamed to `*.json.migrated` once its box is moved, so this runs
/// once. Returns the folders made.
pub fn migrate_legacy(old_dir: &Path, root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(old_dir) else {
        return Vec::new();
    };
    let mut made = Vec::new();
    for path in entries.flatten().map(|entry| entry.path()) {
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(json) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<BoxDoc>(&json) else {
            continue;
        };
        let folder = new_box_path(root, &doc.name);
        if Store::new(&folder).save(&doc).is_err() {
            continue;
        }
        let _ = fs::rename(&path, path.with_extension("json.migrated"));
        made.push(folder);
    }
    made
}

/// Reads and writes a box's folder.
pub struct Store {
    path: PathBuf,
}

impl Store {
    /// The box in folder `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The box called `name` in [`boxes_root`], after moving any boxes earlier builds saved.
    pub fn default_box(name: &str) -> Result<Self> {
        let root = boxes_root()?;
        if let Ok(old) = data_dir().map(|dir| dir.join("boxes")) {
            migrate_legacy(&old, &root);
        }
        let existing = list_boxes(&root)
            .into_iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.path);
        Ok(Self::new(existing.unwrap_or_else(|| root.join(name))))
    }

    /// The box's folder.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The folder this box sits in, with the other boxes.
    pub fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("."))
    }

    fn file(&self) -> PathBuf {
        self.path.join(META_DIR).join(BOX_FILE)
    }

    /// Loads the box, or returns a new empty one named `default_name` if none is saved yet.
    pub fn load_or_new(&self, default_name: &str) -> Result<BoxDoc> {
        let file = self.file();
        if !file.exists() {
            return Ok(BoxDoc::new(default_name));
        }
        let json =
            fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        serde_json::from_str(&json).with_context(|| format!("parsing {}", file.display()))
    }

    /// Saves the box, makes the folders agents expect if they are missing, and rewrites
    /// CONTEXT.md.
    pub fn save(&self, doc: &BoxDoc) -> Result<()> {
        let meta = self.path.join(META_DIR);
        fs::create_dir_all(&meta).with_context(|| format!("creating {}", meta.display()))?;
        for dir in [MEMORY_DIR, SKILLS_DIR] {
            fs::create_dir_all(self.path.join(dir))?;
        }
        let agents = self.path.join(AGENTS_FILE);
        if !agents.exists() {
            fs::write(&agents, agents_template(&doc.name))?;
        }
        write_atomic(&self.file(), serde_json::to_string_pretty(doc)?.as_bytes())?;
        self.refresh_context(doc)
    }

    /// Rewrites CONTEXT.md, after memory or skills changed.
    pub fn refresh_context(&self, doc: &BoxDoc) -> Result<()> {
        write_atomic(
            &self.path.join(CONTEXT_FILE),
            access::context_markdown(&self.path, doc).as_bytes(),
        )
    }
}

/// Writes `bytes` to a temporary file next to `path` and moves it over `path`, so readers
/// never see half a file.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
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
        Store::new(dir.path().join("Work")).save(&work).unwrap();
        Store::new(dir.path().join("Inbox"))
            .save(&BoxDoc::new("Inbox"))
            .unwrap();
        // A folder that is not a box is left out.
        fs::create_dir(dir.path().join("Photos")).unwrap();

        let boxes = list_boxes(dir.path());
        let names: Vec<_> = boxes.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["Inbox", "Work"]);
        assert_eq!(boxes[1].icon.symbol, "briefcase");
        assert_eq!(boxes[0].icon, BoxIcon::default());

        assert_eq!(
            new_box_path(dir.path(), "New Box"),
            dir.path().join("New Box")
        );
        assert_eq!(new_box_path(dir.path(), "Work"), dir.path().join("Work 2"));
        assert_eq!(new_box_path(dir.path(), "a/b"), dir.path().join("a-b"));
        assert_eq!(new_box_path(dir.path(), "..."), dir.path().join("Box"));
    }

    #[test]
    fn store_round_trips_and_lays_out_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("Inbox"));
        let mut doc = store.load_or_new("Inbox").unwrap();
        assert_eq!(doc.name, "Inbox");

        doc.add_paths(&[dir.path().join("photo.PNG")], 0.0, 0.0);
        doc.move_item(1, 50.0, 60.0);
        store.save(&doc).unwrap();

        let root = store.path();
        assert!(root.join(".ambry/box.json").is_file());
        assert!(root.join("memory").is_dir());
        assert!(root.join("skills").is_dir());
        assert!(
            fs::read_to_string(root.join("AGENTS.md"))
                .unwrap()
                .starts_with("# Inbox")
        );
        assert!(
            fs::read_to_string(root.join("CONTEXT.md"))
                .unwrap()
                .contains("photo.PNG")
        );

        // AGENTS.md belongs to people once it exists.
        fs::write(root.join("AGENTS.md"), "mine").unwrap();
        store.save(&doc).unwrap();
        assert_eq!(fs::read_to_string(root.join("AGENTS.md")).unwrap(), "mine");

        let loaded = store.load_or_new("ignored").unwrap();
        assert_eq!(loaded, doc);
        assert!(loaded.item(1).unwrap().is_image());

        let mut loaded = loaded;
        loaded.remove(1);
        assert!(loaded.items.is_empty());
        // Ids keep growing after a removal.
        assert_eq!(loaded.add_paths(&[dir.path().join("b")], 0.0, 0.0), vec![2]);
    }

    #[test]
    fn migrates_json_boxes_once() {
        let old = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut doc = BoxDoc::new("Launch");
        doc.add_paths(&[old.path().join("plan.md")], 0.0, 0.0);
        fs::write(
            old.path().join("launch.json"),
            serde_json::to_string(&doc).unwrap(),
        )
        .unwrap();

        let made = migrate_legacy(old.path(), root.path());
        assert_eq!(made, vec![root.path().join("Launch")]);
        assert_eq!(Store::new(&made[0]).load_or_new("").unwrap(), doc);
        assert!(old.path().join("launch.json.migrated").exists());
        assert!(migrate_legacy(old.path(), root.path()).is_empty());
    }
}
