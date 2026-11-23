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
- **Rust all the way.** The app, the engine, the viewport and the tooling are Rust: the desktop is egui with eframe and wgpu, as
  BIMCraft's is. No web view, Electron, Tauri, JavaScript, Node or Python in the app or its build (Python is fine for one-off
  checks in `tmp`, never as a dependency). Skills for the AI are plain Markdown compiled in; a Three.js, WebGPU or Python skill
  from elsewhere is read for its ideas and never used as code.
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
  `engine.rs` (`Session`: execute, undo, script, catalog, the project's print settings), `commands.rs` (the built-in commands),
  `project.rs` (`.lamina` files: `save`, `open`, `new`), `repair.rs` (mesh repair shared by import and the booleans, and the
  problems `info` reports), `threemf.rs` and `zip.rs` (3MF in and out), `base64.rs`.
  `bridge.rs` (what an agent may ask: execute, script, catalog; read-only access; `REPLACES`, the commands that replace the plate).
- `crates/slicer`: `slice.rs` (mesh to closed loops per layer), `geom.rs` (polygon booleans, offsets, line clipping on i_overlay),
  `print.rs` (walls, skins, infill, skirt, the G-code writer), `support.rs` (overhangs, supports and their interface), `profile.rs` (settings, checks, `[name]` placeholders), `presets.rs`
  (built-in printers and filaments, each setting only its own fields; `apply` and `capture`), `commands.rs`
  (`slice`, `slice_check`, `slicer_presets`, `slicer_profile`; an app adds them with `session.register(lamina_slicer::COMMANDS)`).
  `tests/golden/`: models whose G-code is compared line by line; a slicer change re-blesses them with `LAMINA_BLESS=1` and the diff
  is read before it is committed.
- `crates/csg`: `boolean(op, a, b)`, the only code that knows csgrs, and the `union`, `difference` and `intersect` commands. A result is
  returned only if `sound` accepts it (closed, possible volume and bounds): refuse, never return a broken mesh. The crack repair it
  runs is `lamina_core::repair::close_with`. `vendor/core2` is a
  stand-in for a yanked crate csgrs asks for (see the root `Cargo.toml`).
- `crates/assistant`: the AI chat, ported from BIMCraft. `skills/*.md` are the notes the model reads with `read_skill`
  (modelling, designing for FDM, slicer settings, working from a picture), written for Lamina's own commands: a test checks that every
  command they name exists. A command a skill promises must exist, and a new command the model needs gets a line in the skill. Notes: [crates/assistant/CLAUDE.md](crates/assistant/CLAUDE.md). The only
  crate that may use `ureq` or `ring` (a test in `lamina-core` checks every manifest).
- `apps/lamina`: the desktop app (`lamina`): `lib.rs` (the window: toolbar, plate panel, settings, tabs), `files.rs` (the File menu,
  recent projects, dropped files, the question before unsaved changes are lost), `view3d.rs` (orbit camera,
  a CPU rasterizer that keeps an object id per pixel for picking, and the move gizmo), `preview.rs` (layers read back from the G-code, one drawn at a
  time), `chat.rs` (the assistant panel, with Remember and a passphrase for the key), `settings.rs` (the settings file: print settings,
  presets, sealed keys; read hostilely, written whole and owner-only). Thin: it changes the scene only through `Session::execute`. UI behaviour is tested with
  egui_kittest; `picture_of_the_window` (ignored, needs a GPU) renders it to a PNG to look at.
- `apps/lamina-cli`: the `lamina-cli` binary and `mcp.rs`, the MCP server.
- `docs/`: `ARCHITECTURE.md` (what is ported from BIMCraft and what is not), `RESEARCH.md` (slicers, editors, crates, licence flags),
  `ROADMAP.md` (the order of work and the known limits).
