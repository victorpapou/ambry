# ambry
Ambry is an open-source app to share the necessary context about your project with your agents

## Prototype

The current build is a clickable prototype of the box window: a dotted canvas you drop files and
folders onto, with a list view and the toolbar from the design. There is no agent yet.

What works:

- drag files and folders from Finder onto the canvas, or add them with **+**;
- move cards around, scroll (two fingers on the trackpad) to pan the canvas;
- double-click or press Enter to open a file, Backspace to remove it from the box;
- switch between canvas and list with the toolbar;
- pinch to zoom the canvas, as in Finder and Freeform;
- each box is a folder in `~/Ambry/<name>/` that agents can read: `AGENTS.md` for the rules,
  a generated `CONTEXT.md` listing everything in it, and `memory/` for the notes agents keep.

### Give a box to an agent

`ambry-mcp` serves one box over MCP. With Claude Code:

```sh
claude mcp add ambry -- /path/to/Ambry.app/Contents/MacOS/ambry-mcp --box Inbox
```

The agent gets `box_overview`, `read_item`, `search`, `memory_view`, `memory_read`,
`memory_write`, `memory_edit`, `memory_delete` and `skill_read`. It writes only in `memory/`,
and every change is kept in `.ambry/journal.jsonl` with a copy of what it replaced.

### Run on macOS

You need the Command Line Tools and Rust. Full Xcode is not required: GPUI's shaders are compiled
at startup (`runtime_shaders`).

```sh
xcode-select --install
curl https://sh.rustup.rs -sSf | sh
cargo run --release
```

The first build downloads and compiles GPUI and takes a few minutes.

## Layout

| Crate | What it does |
| --- | --- |
| `crates/ambry-core` | Box folders, memory, what agents may read, connections |
| `crates/ambry-mcp` | A box as an MCP server for agents |
| `crates/ambry-app` | GPUI window: canvas, list, title bar, toolbar |
