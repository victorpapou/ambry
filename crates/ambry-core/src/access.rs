//! What an agent may see of a box: an overview, its files and a text search across them.
//!
//! A box can only hand out what is in it: its own folder, and the files and folders people
//! dropped on its canvas. Anything else is refused, however the path is spelled.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::memory::{Memory, title};
use crate::{
    AGENTS_FILE, BoxDoc, CONTEXT_FILE, ItemKind, MEMORY_DIR, META_DIR, SKILLS_DIR, SOURCES_DIR,
};

/// The most text one read returns, in bytes.
pub const MAX_READ: usize = 256 * 1024;
/// A search reads files up to this size.
const MAX_SEARCH_FILE: u64 = 1024 * 1024;
/// How many files a search looks at, and how many lines it returns.
const MAX_SEARCH_FILES: usize = 5000;
pub const MAX_HITS: usize = 100;
/// How deep a search walks into a dropped folder.
const MAX_DEPTH: usize = 8;
/// Folders a search never walks into.
const SKIP_DIRS: [&str; 6] = ["node_modules", "target", ".git", "build", "dist", META_DIR];

/// CONTEXT.md: everything in the box in one page, for agents to read first.
pub fn context_markdown(root: &Path, doc: &BoxDoc) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}: context\n", doc.name);
    let _ = writeln!(
        out,
        "Ambry writes this file; edits here are overwritten. Rules for agents are in {AGENTS_FILE}.\n"
    );
    let _ = writeln!(out, "## Files in this box\n");
    if doc.items.is_empty() {
        let _ = writeln!(out, "Nothing yet.");
    }
    for item in &doc.items {
        let kind = match item.kind {
            ItemKind::Folder => "folder",
            ItemKind::File => "file",
        };
        let missing = if item.exists() { "" } else { " (missing)" };
        let _ = writeln!(
            out,
            "- {} ({kind}{missing}): `{}`",
            item.name(),
            item.path.display()
        );
    }
    let notes = Memory::new(root).list();
    let _ = writeln!(out, "\n## Memory\n");
    if notes.is_empty() {
        let _ = writeln!(out, "No notes yet. Agents save notes in {MEMORY_DIR}/.");
    }
    for note in notes {
        let _ = writeln!(out, "- `{MEMORY_DIR}/{}`: {}", note.path, note.title);
    }
    let skills = skills(root);
    if !skills.is_empty() {
        let _ = writeln!(out, "\n## Skills\n");
        for (name, summary) in skills {
            let _ = writeln!(out, "- `{SKILLS_DIR}/{name}/SKILL.md`: {summary}");
        }
    }
    let sources = files_under(&root.join(SOURCES_DIR), 2);
    if !sources.is_empty() {
        let _ = writeln!(out, "\n## Sources\n");
        for source in sources {
            if let Ok(relative) = source.strip_prefix(root) {
                let _ = writeln!(out, "- `{}`", relative.display());
            }
        }
    }
    out
}

/// Each skill's folder name and its description (from the front matter) or first line.
pub fn skills(root: &Path) -> Vec<(String, String)> {
    let Ok(entries) = fs::read_dir(root.join(SKILLS_DIR)) else {
        return Vec::new();
    };
    let mut skills: Vec<(String, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let text = fs::read_to_string(entry.path().join("SKILL.md")).ok()?;
            let summary = text
                .lines()
                .find_map(|line| line.strip_prefix("description:"))
                .map(|line| line.trim().trim_matches('"').to_string())
                .unwrap_or_else(|| title(&text));
            Some((entry.file_name().to_string_lossy().into_owned(), summary))
        })
        .collect();
    skills.sort();
    skills
}

/// What `box_overview` hands an agent first: the rules, the index and how to use memory.
pub fn overview(root: &Path, doc: &BoxDoc) -> String {
    let agents = fs::read_to_string(root.join(AGENTS_FILE)).unwrap_or_default();
    let mut out = String::new();
    let _ = writeln!(out, "Box \"{}\" at {}\n", doc.name, root.display());
    if !agents.trim().is_empty() {
        let _ = writeln!(out, "=== {AGENTS_FILE} ===\n{}\n", agents.trim_end());
    }
    let _ = writeln!(
        out,
        "=== {CONTEXT_FILE} ===\n{}",
        context_markdown(root, doc).trim_end()
    );
    out
}

/// What a read found.
#[derive(Debug, PartialEq)]
pub enum Read {
    Text {
        path: PathBuf,
        text: String,
        truncated: bool,
    },
    Folder {
        path: PathBuf,
        entries: Vec<String>,
    },
    /// A file that is not text, like a picture: its size, so the agent knows it is there.
    Binary {
        path: PathBuf,
        bytes: u64,
    },
}

/// Reads `requested`: a path relative to the box's folder, or an absolute path to a dropped
/// file or to something inside a dropped folder.
pub fn read(root: &Path, doc: &BoxDoc, requested: &str) -> Result<Read> {
    let path = allowed(root, doc, requested)?;
    if path.is_dir() {
        let mut entries: Vec<String> = fs::read_dir(&path)?
            .flatten()
            .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
            .map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if entry.path().is_dir() {
                    format!("{name}/")
                } else {
                    name
                }
            })
            .collect();
        entries.sort();
        return Ok(Read::Folder { path, entries });
    }
    let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let truncated = bytes.len() > MAX_READ;
    let head = &bytes[..bytes.len().min(MAX_READ)];
    match std::str::from_utf8(head) {
        Ok(text) => Ok(Read::Text {
            path,
            text: text.to_string(),
            truncated,
        }),
        // A cut through a multi-byte character at the limit is still text.
        Err(err) if truncated && err.valid_up_to() + 4 > head.len() => Ok(Read::Text {
            path,
            text: String::from_utf8_lossy(&head[..err.valid_up_to()]).into_owned(),
            truncated,
        }),
        Err(_) => Ok(Read::Binary {
            path,
            bytes: bytes.len() as u64,
        }),
    }
}

/// `requested` as a real path, if the box holds it.
fn allowed(root: &Path, doc: &BoxDoc, requested: &str) -> Result<PathBuf> {
    let requested_path = Path::new(requested);
    let joined = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        root.join(requested_path)
    };
    let Ok(real) = joined.canonicalize() else {
        bail!("{requested} does not exist");
    };
    let root_real = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let in_meta = real.starts_with(root_real.join(META_DIR));
    if real.starts_with(&root_real) && !in_meta {
        return Ok(real);
    }
    for item in &doc.items {
        let Ok(item_real) = item.path.canonicalize() else {
            continue;
        };
        let inside = match item.kind {
            ItemKind::File => real == item_real,
            ItemKind::Folder => real.starts_with(&item_real),
        };
        if inside {
            return Ok(real);
        }
    }
    bail!("{requested} is not in this box")
}

/// A line that matched a search.
#[derive(Debug, PartialEq)]
pub struct Hit {
    pub path: PathBuf,
    /// Counted from 1.
    pub line: usize,
    pub text: String,
}

/// Every line, in the box's files, memory, skills and sources, that contains `query`,
/// ignoring case.
pub fn search(root: &Path, doc: &BoxDoc, query: &str) -> Vec<Hit> {
    let needle = query.to_lowercase();
    if needle.trim().is_empty() {
        return Vec::new();
    }
    let mut files: Vec<PathBuf> = vec![root.join(AGENTS_FILE)];
    for dir in [MEMORY_DIR, SKILLS_DIR, SOURCES_DIR] {
        files.extend(files_under(&root.join(dir), MAX_DEPTH));
    }
    for item in &doc.items {
        match item.kind {
            ItemKind::File => files.push(item.path.clone()),
            ItemKind::Folder => files.extend(files_under(&item.path, MAX_DEPTH)),
        }
    }
    files.truncate(MAX_SEARCH_FILES);
    let mut hits = Vec::new();
    for path in files {
        if fs::metadata(&path).map_or(true, |meta| meta.len() > MAX_SEARCH_FILE) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if line.to_lowercase().contains(&needle) {
                hits.push(Hit {
                    path: path.clone(),
                    line: index + 1,
                    text: line.trim().chars().take(200).collect(),
                });
                if hits.len() >= MAX_HITS {
                    return hits;
                }
            }
        }
    }
    hits
}

/// Files under `dir`, hidden ones and build folders left out, sorted.
fn files_under(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(dir, depth, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth > 0 {
                walk(&entry.path(), depth - 1, out);
            }
        } else if kind.is_file() {
            out.push(entry.path());
            if out.len() >= MAX_SEARCH_FILES {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    fn sample() -> (tempfile::TempDir, PathBuf, BoxDoc) {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir_all(outside.join("assets")).unwrap();
        fs::write(outside.join("brief.md"), "# Brief\nLaunch on Friday\n").unwrap();
        fs::write(outside.join("assets/notes.txt"), "the launch slogan\n").unwrap();
        fs::write(
            outside.join("assets/logo.png"),
            [0x89, 0x50, 0xff, 0xfe, 0x00],
        )
        .unwrap();
        fs::write(dir.path().join("secret.txt"), "launch codes").unwrap();
        let root = dir.path().join("Box");
        let mut doc = BoxDoc::new("Launch");
        doc.add_paths(&[outside.join("brief.md"), outside.join("assets")], 0., 0.);
        Store::new(&root).save(&doc).unwrap();
        Memory::new(&root)
            .write(
                "decisions.md",
                "# Decisions\nLaunch moved to Friday\n",
                "agent",
            )
            .unwrap();
        (dir, root, doc)
    }

    #[test]
    fn reads_only_what_the_box_holds() {
        let (dir, root, doc) = sample();
        let outside = dir.path().join("outside");
        let brief = outside.join("brief.md");
        assert!(matches!(
            read(&root, &doc, brief.to_str().unwrap()).unwrap(),
            Read::Text { ref text, .. } if text.contains("Friday")
        ));
        assert!(matches!(
            read(&root, &doc, "memory/decisions.md").unwrap(),
            Read::Text { .. }
        ));
        assert!(matches!(
            read(&root, &doc, outside.join("assets").to_str().unwrap()).unwrap(),
            Read::Folder { ref entries, .. } if entries == &["logo.png", "notes.txt"]
        ));
        assert!(matches!(
            read(
                &root,
                &doc,
                outside.join("assets/logo.png").to_str().unwrap()
            )
            .unwrap(),
            Read::Binary { bytes: 5, .. }
        ));
        for bad in [
            dir.path().join("secret.txt").to_string_lossy().into_owned(),
            "../secret.txt".to_string(),
            "assets/../../secret.txt".to_string(),
            ".ambry/box.json".to_string(),
        ] {
            assert!(read(&root, &doc, &bad).is_err(), "{bad} was readable");
        }
    }

    #[test]
    fn searches_files_and_memory() {
        let (_dir, root, doc) = sample();
        let hits = search(&root, &doc, "LAUNCH");
        let names: Vec<String> = hits
            .iter()
            .map(|hit| hit.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"decisions.md".to_string()));
        assert!(names.contains(&"brief.md".to_string()));
        assert!(names.contains(&"notes.txt".to_string()));
        assert!(!names.contains(&"secret.txt".to_string()));
    }

    #[test]
    fn context_lists_files_memory_and_skills() {
        let (_dir, root, doc) = sample();
        fs::create_dir_all(root.join("skills/release")).unwrap();
        fs::write(
            root.join("skills/release/SKILL.md"),
            "---\nname: release\ndescription: How we ship a release\n---\n",
        )
        .unwrap();
        let text = context_markdown(&root, &doc);
        assert!(text.contains("brief.md (file)"));
        assert!(text.contains("`memory/decisions.md`: Decisions"));
        assert!(text.contains("`skills/release/SKILL.md`: How we ship a release"));
        assert!(overview(&root, &doc).contains("Rules for agents"));
    }
}
