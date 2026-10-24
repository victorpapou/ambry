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
- the box is saved to `~/Library/Application Support/Ambry/boxes/inbox.json`.

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
| `crates/ambry-core` | Box model and JSON storage |
| `crates/ambry-app` | GPUI window: canvas, list, title bar, toolbar |
