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
  Only three crates reach the network: the AI chat (`crates/assistant`), the printer connection (`crates/printer`, to the one
  address the user typed) and the updater (`crates/update`, HTTPS to `api.github.com` and GitHub's release download hosts only,
  and a dependency of `apps/lamina` alone). The `network_boundary` test in `lamina-core` checks every manifest. The one other socket is the
  window's control port for agents (`apps/lamina/src/control.rs`, and `apps/lamina-cli/src/window.rs` that asks it): 127.0.0.1
  only, with a token, never another address.
- **Clean-room slicing.** PrusaSlicer, OrcaSlicer, Bambu Studio and Cura are AGPL or LGPL: read their documentation and behave
  alike, never copy their code. Prefer permissively licensed crates (MIT or Apache-2.0), and check the licence before adding one.
- **Units.** Millimetres, x east, y north, z up. The scene's origin is the middle of the plate, z = 0 is the plate's surface. A
  printer profile maps that to the machine's own coordinates.
- **Test first, and look at the result.** Mesh logic is tested through its output (volumes, layer counts, G-code lines). Leave one
  runnable check behind any new branch, loop or parser, and give a new parser a fuzz test with `lamina_core::fuzz`.

## Before you finish

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo i18n check
```

CI (`.github/workflows/ci.yml`) runs the same on Linux, Windows and macOS for every push and pull request (format and translations on
Linux only). A change to a string the window shows needs `cargo i18n update` and its translations (docs/TRANSLATING.md).

## Module map

- `crates/core`: `mesh.rs` (triangle mesh, primitives with ISO metric threads, `Transform`), `stl.rs` (read and write), `scene.rs`
  (objects on the plate, and `Modifier`: support enforcers and blockers, which print nothing),
  `engine.rs` (`Session`: execute, undo, transactions, `adopt` for work done on a thread, script, catalog, the project's print settings), `commands.rs` (the built-in commands),
  `project.rs` (`.lamina` files: `save`, `open`, `new`), `repair.rs` (mesh repair shared by import and the booleans, `tidy_flat` that takes out the vertices a
  boolean left in flat faces and along straight edges, and the problems `info` reports), `simplify.rs` (quadric edge collapse for heavy meshes, the `simplify` command), `threemf.rs` and `zip.rs` (3MF in and out), `mesh_in.rs` (OBJ and glTF in, lifted from BIMCraft), `sketch.rs` (outlines extruded into solids), `text.rs` (lettering from the bundled IBM Plex Mono, under the OFL), `render.rs` (the orbit camera and CPU rasterizer
  the 3D view, the thumbnails and the assistant's picture share, `draw_paths` for the 3D G-code preview, and a PNG writer), `base64.rs`, `fuzz.rs` (the seeded generator every crate's fuzz tests use), `cancel.rs` (the flag a worker thread's slice or
  boolean stops on).
  `bridge.rs` (what an agent may ask: execute, script, catalog; read-only access; `REPLACES`, the commands that replace the plate;
  the control port's protocol, token and bridge file, and `settings_file`, the settings folder the desktop and the CLI share).
- `crates/slicer`: `slice.rs` (mesh to closed loops per layer), `geom.rs` (polygon booleans, offsets, line clipping on i_overlay),
  `print.rs` (walls, skins, infill, skirt, tool changes and the purge tower, one object at a time, the G-code writer), `infill.rs` (sparse infill patterns), `gap.rs` (gap fill and thin walls), `walls.rs` (walls of varying width for thin parts,
  `wall_generator: "variable"`), `time.rs` (the print time planner), `support.rs` (overhangs,
  supports and their interface, planned for the whole plate with its enforcers and blockers, `plan_with`), `profile.rs` (settings, `LIMITS` and `ABOUT`, checks, `[name]` placeholders), `expr.rs` (`{expr}` in the start and end G-code), `import.rs` (PrusaSlicer and OrcaSlicer profiles through a key table), `presets.rs`
  (built-in printers and filaments, each setting only its own fields; `apply` and `capture`), `commands.rs`
  (`slice`, `slice_check`, `slicer_presets`, `slicer_profile`; an app adds them with `session.register(lamina_slicer::COMMANDS)`).
  `tests/golden/`: models whose G-code is compared line by line; a slicer change re-blesses them with `LAMINA_BLESS=1` and the diff
  is read before it is committed.
- `crates/csg`: `boolean(op, a, b)`, the only code that knows csgrs, and the `union`, `difference` and `intersect` commands; `split.rs`
  has `cut` (any axis, with dowels or pins placed on the cut face by the slicer's sections) and `split_to_fit`. A result is
  returned only if `sound` accepts it (closed, possible volume and bounds): refuse, never return a broken mesh. It builds the BSP
  trees itself (`build`, never on a sliver's plane) rather than with csgrs' `Node::build`. The crack repair it
  runs is `lamina_core::repair::close_with`. `child.rs`: the apps run each boolean in a copy of themselves (`--boolean-child`), so a
  crash in csgrs is an error, not the end of the app; every app's `main` calls `child_main` first. `vendor/core2` is a
  stand-in for a yanked crate csgrs asks for (see the root `Cargo.toml`).
- `crates/assistant`: the AI chat, ported from BIMCraft. `skills/*.md` are the notes the model reads with `read_skill`
  (modelling, designing for FDM, slicer settings, objects, heights and extruders, supports, materials, calibration, troubleshooting, working from a picture), written for Lamina's own commands: a test checks that every
  command they name exists. A command a skill promises must exist, and a new command the model needs gets a line in the skill. Notes: [crates/assistant/CLAUDE.md](crates/assistant/CLAUDE.md). With
  `crates/update` the only crates that may use `ring`, and with `crates/printer` and `crates/update` the only ones that may use `ureq`
  (a test in `lamina-core` checks every manifest).
- `crates/printer`: sending G-code to Moonraker, OctoPrint, PrusaLink (`prusalink.rs`, with an API key or a user name and password by
  HTTP digest, `digest.rs`) or a Duet (`duet.rs` on its own, `dsf.rs` run by a single-board computer) on the user's network, or
  streamed to a printer on a USB cable (`serial.rs`, over `Read + Write`, the port from `serial2`) (`send`, the `send_to_printer` command), what
  it is doing (`status`, the window's watch, not a command so no agent can point it elsewhere), and a
  fake printer for tests (`testing`, a feature). With the assistant and the updater, the only crates that may use `ureq`.
- `crates/update` (`lamina-update`): the updater, lifted from BIMCraft's. `lib.rs` (the latest release from GitHub's API, its tag
  compared with semver, the asset for this install chosen by the names in `release.yml`, the download capped and checked against
  GitHub's SHA-256 digest), `net.rs` (ureq, HTTPS only, redirects followed by hand and only to GitHub's release hosts), `detect.rs`
  (which install this is: the MSI's copy in Program Files, an AppImage, `Lamina.app`), `install.rs` (Windows: a copy of the program
  runs the MSI after the window exits, `--finish-update`), `appimage.rs` and `macos.rs` (the new file or bundle tried with
  `--version`, then renamed in), `updater.rs` (the state the window draws, work on threads), `testing` (a feature: `Fake`, a
  table of answers). Notes: [crates/update/CLAUDE.md](crates/update/CLAUDE.md). Only `apps/lamina` depends on it.
- `crates/i18n` (`lamina-i18n`): the window's translations, lifted from BIMCraft's. `lib.rs` (`translated`, the lookup in the
  compiled tables, and `ui_msg!`, which marks English chosen indirectly and is the literal itself, so the slicer can mark its
  setting help with no other dependency), `catalog.rs` (the PO reader, merge and the compiler `apps/lamina/build.rs` runs, the
  `catalog` feature), `catalog/extract.rs` and `main.rs` (`cargo i18n extract|update|check|stats`, the `tooling` feature, the only
  one with syn). The catalogs are `translations/ui/*.po`; how to translate: [docs/TRANSLATING.md](docs/TRANSLATING.md).
- `apps/lamina`: the desktop app (`lamina`): `lib.rs` (the window: toolbar, plate panel, settings, tabs), `files.rs` (the File menu,
  recent projects, dropped files, the question before unsaved changes are lost), `autosave.rs` (the plate written every two minutes
  and offered back after an unclean exit), `view3d.rs` (the 3D view on
  `lamina_core::render`, picking, the gizmo and the tools, and Blender's G, R and S as `Modal`), `gpu.rs` and `gpu.wgsl` (the same picture drawn on the graphics card with
  wgpu: the scene uploaded once for each scene `Arc`, ids and depth read back only for a click; the processor draws when there is no
  card, it fails, or View says so; from BIMCraft), `preview.rs` (layers read back from the G-code with each extrusion's kind, drawn in 3D up to the
  chosen layer and coloured by kind, or one layer from above), `chat.rs` (the assistant panel, with Remember and a passphrase for the key), `printer.rs` (the printer connection and the Send
  buttons), `work.rs` (slicing, booleans, export and sending on a thread, with a spinner and Cancel), `comfort.rs` (shortcuts,
  Blender's view keys, the right-click menu, the Edit menu with the undo history, the theme), `control.rs` (the control port: agents work on the
  window's plate through `lamina-cli mcp --connect`; from BIMCraft), `icon.rs` (the app icon, drawn, and its PNG, ICO and ICNS files), `startup.rs` (`lamina.log` beside the settings, panics
  and failed starts, and another graphics backend after a start that died; from BIMCraft), `updates.rs` (the update banner, the box
  that holds the window while one installs, and the File menu's Check for updates at start and Check for updates now; `main`
  attaches the updater, so a window in a test has `updates: None`; from BIMCraft), `settings.rs` (the settings file: print settings,
  presets, sealed keys, the update check's switch, the language; read hostilely, written whole and owner-only), `lang.rs` (the
  interface's language: `t`, `tf` and `tc` look up the catalogs `build.rs` compiles, English the key and the fallback; set each
  frame on the window's thread, so a test's window is English; View, Language picks it, `main` takes the system's at the first
  start; from BIMCraft). Every string the window shows goes through `t`, `tf` or `tc`, or `ui_msg!` where a table or enum holds
  it; command errors, G-code, the log and what agents read stay English. Thin: it changes the scene only through `Session::execute`. UI behaviour is tested with
  egui_kittest; `picture_of_the_window` (ignored, needs a GPU) renders it to a PNG to look at.
- `apps/lamina-cli`: the `lamina-cli` binary, `mcp.rs` (the MCP server) and `window.rs` (`mcp --connect`: the tools run on an
  open window's plate through its control port).
- `tools/notices` (`lamina-notices`): checks every dependency's licence (a test runs it on the whole tree) and writes the
  THIRD-PARTY-NOTICES.txt each release carries.
- `packaging/` and `.github/workflows/release.yml`: the AppImage and tarball, the MSI and the DMG, built and published on each tag
  (lifted from BIMCraft's). `assets/app-icon/` is written by `apps/lamina/src/icon.rs` (`LAMINA_BLESS=1`; a test checks they match).
- `docs/`: `PRINT-TESTS.md` (the three test prints of roadmap 0.2: scripts in `print-tests/`, commands, tables to fill in; checked by
  `crates/slicer/tests/print_tests.rs`), `USER-GUIDE.md` (for people using the window; its table of settings is checked by `crates/slicer/tests/guide.rs`,
  its pictures in `images/` drawn by `docs_pictures`), `ARCHITECTURE.md` (what is ported from BIMCraft and what is not), `RESEARCH.md` (slicers, editors, crates, licence flags),
  `ROADMAP.md` (the order of work and the known limits).
