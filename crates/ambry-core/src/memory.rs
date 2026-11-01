//! A box's memory: Markdown notes in `memory/` that agents write and people correct.
//!
//! Every change goes through here so it lands in `.ambry/journal.jsonl` with a copy of what it
//! replaced in `.ambry/history/`. That is what lets the app show "changed by an agent" and undo
//! it.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{MEMORY_DIR, META_DIR, write_atomic};

/// The largest note an agent may write, in bytes.
pub const MAX_NOTE: usize = 64 * 1024;

/// A note as [`Memory::list`] describes it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Note {
    /// Relative to `memory/`, with `/` between folders, like `deploy.md` or `team/rules.md`.
    pub path: String,
    pub bytes: u64,
    /// The first heading or line, so an agent can tell notes apart without reading them.
    pub title: String,
}

/// What happened to a note.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Create,
    Write,
    Edit,
    Delete,
}

/// One line of `.ambry/journal.jsonl`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Change {
    /// Seconds since 1970.
    pub time: u64,
    /// Who made it: `"agent:<client name>"` or `"person"`.
    pub actor: String,
    pub action: Action,
    /// Relative to the box's folder, like `memory/deploy.md`.
    pub path: String,
    /// What the file held before, relative to the box's folder; none for a new file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

/// The memory of the box in folder `root`.
pub struct Memory {
    root: PathBuf,
}

impl Memory {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn dir(&self) -> PathBuf {
        self.root.join(MEMORY_DIR)
    }

    /// Every note, by path.
    pub fn list(&self) -> Vec<Note> {
        let mut notes = Vec::new();
        collect(&self.dir(), "", &mut notes);
        notes.sort_by(|a, b| a.path.cmp(&b.path));
        notes
    }

    pub fn read(&self, note: &str) -> Result<String> {
        let path = self.resolve(note)?;
        fs::read_to_string(&path).with_context(|| format!("no note {note}"))
    }

    /// Creates `note` or replaces all of it.
    pub fn write(&self, note: &str, text: &str, actor: &str) -> Result<()> {
        if text.len() > MAX_NOTE {
            bail!("a note holds at most {MAX_NOTE} bytes; split it into several notes");
        }
        let path = self.resolve(note)?;
        let existed = path.exists();
        let before = self.keep_copy(note, &path)?;
        write_atomic(&path, text.as_bytes())?;
        self.log(Change {
            time: now(),
            actor: actor.into(),
            action: if existed { Action::Write } else { Action::Create },
            path: format!("{MEMORY_DIR}/{note}"),
            before,
        })
    }

    /// Replaces the one place `old` appears in `note` with `new`.
    pub fn edit(&self, note: &str, old: &str, new: &str, actor: &str) -> Result<()> {
        let text = self.read(note)?;
        if old.is_empty() {
            bail!("old_text is empty");
        }
        match text.matches(old).count() {
            0 => bail!("old_text does not appear in {note}; read the note and quote it exactly"),
            1 => {}
            n => bail!("old_text appears {n} times in {note}; quote more of it so it is unique"),
        }
        let text = text.replacen(old, new, 1);
        if text.len() > MAX_NOTE {
            bail!("a note holds at most {MAX_NOTE} bytes; split it into several notes");
        }
        let path = self.resolve(note)?;
        let before = self.keep_copy(note, &path)?;
        write_atomic(&path, text.as_bytes())?;
        self.log(Change {
            time: now(),
            actor: actor.into(),
            action: Action::Edit,
            path: format!("{MEMORY_DIR}/{note}"),
            before,
        })
    }

    pub fn delete(&self, note: &str, actor: &str) -> Result<()> {
        let path = self.resolve(note)?;
        if !path.is_file() {
            bail!("no note {note}");
        }
        let before = self.keep_copy(note, &path)?;
        fs::remove_file(&path)?;
        self.log(Change {
            time: now(),
            actor: actor.into(),
            action: Action::Delete,
            path: format!("{MEMORY_DIR}/{note}"),
            before,
        })
    }

    /// The journal, oldest first.
    pub fn journal(&self) -> Vec<Change> {
        let Ok(text) = fs::read_to_string(self.root.join(META_DIR).join("journal.jsonl")) else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// Puts back what `change` replaced: the old text, or no file for a note it created.
    pub fn undo(&self, change: &Change) -> Result<()> {
        let path = self.root.join(&change.path);
        if !path.starts_with(self.dir()) {
            bail!("{} is not in memory/", change.path);
        }
        match &change.before {
            Some(before) => {
                let text = fs::read(self.root.join(before))
                    .with_context(|| format!("the copy {before} is gone"))?;
                write_atomic(&path, &text)
            }
            None => {
                if path.exists() {
                    fs::remove_file(&path)?;
                }
                Ok(())
            }
        }
    }

    /// The file for `note`, refusing anything outside `memory/` and anything but Markdown.
    fn resolve(&self, note: &str) -> Result<PathBuf> {
        let relative = Path::new(note);
        if note.is_empty()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            bail!("{note:?} is not a path inside memory/; use a relative path like deploy.md");
        }
        if relative.extension().is_none_or(|ext| ext != "md") {
            bail!("notes are Markdown files; name it {note}.md");
        }
        if relative
            .components()
            .any(|part| part.as_os_str().to_string_lossy().starts_with('.'))
        {
            bail!("note names may not start with a dot");
        }
        Ok(self.dir().join(relative))
    }

    /// Copies the file at `path` into the history before it changes, and returns where.
    fn keep_copy(&self, note: &str, path: &Path) -> Result<Option<String>> {
        if !path.is_file() {
            return Ok(None);
        }
        let name = format!("{}-{}", now_nanos(), note.replace('/', "_"));
        let relative = format!("{META_DIR}/history/{name}");
        let copy = self.root.join(&relative);
        fs::create_dir_all(copy.parent().unwrap())?;
        fs::copy(path, &copy).with_context(|| format!("copying {} aside", path.display()))?;
        Ok(Some(relative))
    }

    fn log(&self, change: Change) -> Result<()> {
        use std::io::Write as _;
        let path = self.root.join(META_DIR).join("journal.jsonl");
        fs::create_dir_all(path.parent().unwrap())?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(file, "{}", serde_json::to_string(&change)?)?;
        Ok(())
    }
}

fn collect(dir: &Path, prefix: &str, notes: &mut Vec<Note>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let relative = format!("{prefix}{name}");
        if path.is_dir() {
            collect(&path, &format!("{relative}/"), notes);
        } else if name.ends_with(".md") {
            let text = fs::read_to_string(&path).unwrap_or_default();
            notes.push(Note {
                path: relative,
                bytes: entry.metadata().map(|meta| meta.len()).unwrap_or(0),
                title: title(&text),
            });
        }
    }
}

/// The first non-empty line, without a heading's hashes.
pub fn title(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && *line != "---")
        .unwrap_or("");
    let line = line.trim_start_matches('#').trim();
    line.chars().take(100).collect()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .unwrap_or(0)
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_edits_and_lists_notes() {
        let dir = tempfile::tempdir().unwrap();
        let memory = Memory::new(dir.path());
        memory
            .write("deploy.md", "# Deploy\n\nship: make ship\n", "agent:test")
            .unwrap();
        memory
            .write("team/rules.md", "Rules\n- be kind\n", "person")
            .unwrap();
        let notes = memory.list();
        assert_eq!(
            notes.iter().map(|note| note.path.as_str()).collect::<Vec<_>>(),
            ["deploy.md", "team/rules.md"]
        );
        assert_eq!(notes[0].title, "Deploy");

        memory
            .edit("deploy.md", "make ship", "make release", "agent:test")
            .unwrap();
        assert!(memory.read("deploy.md").unwrap().contains("make release"));
        assert!(memory.edit("deploy.md", "nowhere", "x", "agent:test").is_err());

        let journal = memory.journal();
        assert_eq!(journal.len(), 3);
        assert_eq!(journal[0].action, Action::Create);
        assert_eq!(journal[2].action, Action::Edit);
        assert_eq!(journal[2].path, "memory/deploy.md");

        // Undo puts back the text before the edit.
        memory.undo(&journal[2]).unwrap();
        assert!(memory.read("deploy.md").unwrap().contains("make ship"));
        // Undoing a creation removes the note.
        memory.undo(&journal[0]).unwrap();
        assert!(memory.read("deploy.md").is_err());
    }

    #[test]
    fn refuses_paths_outside_memory() {
        let dir = tempfile::tempdir().unwrap();
        let memory = Memory::new(dir.path());
        for bad in ["../AGENTS.md", "/etc/passwd.md", "notes.txt", ".hidden.md", "", "a/../../b.md"] {
            assert!(memory.write(bad, "x", "agent").is_err(), "{bad} was allowed");
        }
        assert!(!dir.path().join("AGENTS.md").exists());
    }

    #[test]
    fn deletes_with_a_copy_kept() {
        let dir = tempfile::tempdir().unwrap();
        let memory = Memory::new(dir.path());
        memory.write("a.md", "keep me", "agent").unwrap();
        memory.delete("a.md", "agent").unwrap();
        assert!(memory.list().is_empty());
        let change = memory.journal().pop().unwrap();
        memory.undo(&change).unwrap();
        assert_eq!(memory.read("a.md").unwrap(), "keep me");
    }
}
