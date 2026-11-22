# Lamina: instructions for agents

Lamina is a 3D slicer and modelling tool with a built-in AI chat, a CLI and an MCP server, in pure Rust. It reuses the design of
[BIMCraft](https://github.com/IPedrax/BIMCraft) (same author, MIT OR Apache-2.0): one command engine that every front end drives.

## Rules

- **Never crash.** No `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, `unreachable!` or `unsafe` outside tests (the workspace
  lints deny them). Files, parameters and anything an AI sends are hostile: check that numbers are finite and in range, and cap sizes
  (`MAX_TRIANGLES`, `MAX_COORD`, `MAX_FILE`, `MAX_OBJECTS`). `Session::execute` runs every command under `catch_unwind` and restores the
  scene on failure.
- **Everything is a command** (`crates/core/src/commands.rs`, plus the sets other crates `Session::register`). The CLI, scripts, MCP
  server, AI chat and desktop all go through `Session::execute`, so new behaviour is a new command or parameter, with a test. Each
  command's `params` text is written for agents: they read it to call the command.
- **Strict in, kind out.** Command parameters use `#[serde(deny_unknown_fields)]`: a misspelled key is an error.
- **The core has no UI and no network.** `lamina-core` must not depend on a UI toolkit or an HTTP client, so front ends stay swappable.
  Only the desktop's AI chat will reach the network, through its own crate.
- **Clean-room slicing.** PrusaSlicer, OrcaSlicer, Bambu Studio and Cura are AGPL or LGPL: read their documentation and behave
  alike, never copy their code. Prefer permissively licensed crates (MIT or Apache-2.0), and check the licence before adding one.
- **Units.** Millimetres, x east, y north, z up. The scene's origin is the middle of the plate, z = 0 is the plate's surface. A
  printer profile maps that to the machine's own coordinates.
- **Test first, and look at the result.** Mesh logic is tested through its output (volumes, layer counts, G-code lines). Leave one
  runnable check behind any new branch, loop or parser.

## Before you finish

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Module map

- `crates/core`: `mesh.rs` (triangle mesh, primitives, `Transform`), `stl.rs` (read and write), `scene.rs` (objects on the plate),
  `engine.rs` (`Session`: execute, undo, script, catalog), `commands.rs` (the built-in commands).
  `bridge.rs` (what an agent may ask: execute, script, catalog; read-only access).
- `crates/slicer`: `slice.rs` (mesh to closed loops per layer), `geom.rs` (polygon booleans, offsets, line clipping on i_overlay),
  `print.rs` (walls, skins, infill, skirt, the G-code writer), `profile.rs` (settings, checks, `[name]` placeholders), `commands.rs`
  (`slice`, `slicer_profile`; an app adds them with `session.register(lamina_slicer::COMMANDS)`).
- `crates/assistant`: the AI chat, ported from BIMCraft. Notes: [crates/assistant/CLAUDE.md](crates/assistant/CLAUDE.md). The only
  crate that may use `ureq` or `ring` (a test in `lamina-core` checks every manifest).
- `apps/lamina-cli`: the `lamina` binary and `mcp.rs`, the MCP server.
- `docs/`: `ARCHITECTURE.md` (what is ported from BIMCraft and what is not), `RESEARCH.md` (slicers, editors, crates, licence flags),
  `ROADMAP.md` (the order of work and the known limits).
