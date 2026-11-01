# Ambry for agents

Ambry is a native Mac app: each project is a box, a folder of context that people and agents
share. Rust workspace, UI on GPUI with native AppKit views.

## Crates

| Crate | What it does |
| --- | --- |
| `crates/ambry-core` | Box model and storage: a box is a folder (`.ambry/box.json`, `AGENTS.md`, `CONTEXT.md`, `memory/`, `skills/`, `sources/`); memory with a journal (`memory.rs`); what agents may read and search (`access.rs`); connected services (`connections.rs`) |
| `crates/ambry-mcp` | `ambry-mcp`, one box as an MCP server over stdio |
| `crates/ambry-app` | The GPUI window (`box_view.rs`) that hosts the native views |
| `crates/ambry-macos` | Native AppKit views: icon canvas (`icon_canvas.rs`), columns, toolbar, settings, Quick Look |

## Rules

- Controls are native AppKit, never drawn look-alikes. A native view joins GPUI's layout
  through `ambry-app/src/native_view.rs` and `ambry_macos::NativeView`.
- Agents write only in a box's `memory/`, through `ambry_core::memory`, so every change is
  journaled and can be undone.
- UI changes are checked on a real Mac before they are called done.

## Checks

```sh
cargo test -p ambry-core -p ambry-mcp
# The native views type-check on Linux too:
rustup target add aarch64-apple-darwin
cargo check -p ambry-macos --target aarch64-apple-darwin
```

`ambry-app` builds only on macOS (GPUI needs the SDK): `sh scripts/bundle.sh`.
