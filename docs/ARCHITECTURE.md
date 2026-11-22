# Architecture

## One engine, many front ends

`lamina_core::Session` owns the scene (the objects on the plate). Every change goes through `Session::execute(name, params)`:

1. The command is found by id (`commands::ALL`, plus any set a crate registers, like the slicer's).
2. It runs under `catch_unwind`. A panic, an error, or a scene that fails `Scene::check` restores the scene as it was.
3. A successful change that is `undoable` becomes an undo step (200 kept; a step is an `Arc` of the old scene, so it costs a pointer).

`bridge::run` sits on top for agents (the MCP server and the AI chat). It adds `Access::Read`, which refuses any command that
changes the scene or its history. Parameters are strict (`deny_unknown_fields`), and every number is range-checked.

```
lamina-cli (CLI, MCP) ─┐
lamina-assistant (chat) ─┼─> bridge::run ─> Session::execute ─> commands (core + slicer) ─> Scene
desktop (planned)  ────┘
```

## Crates

| Crate | Holds | Depends on |
|---|---|---|
| `lamina-core` | `Mesh`, `Transform`, STL read/write, `Scene`, `Session`, `bridge`, built-in commands | serde |
| `lamina-slicer` | `slice` (mesh to loops), `geom` (polygon booleans and offsets on `i_overlay`), `print` (walls, infill, G-code), `profile`, the `slice` and `slicer_profile` commands | core, i_overlay |
| `lamina-assistant` | provider clients, sealed key storage, tool layer, turn loop | core, ureq, ring |
| `lamina-cli` | `lamina` binary, MCP server | core, slicer |

A test in `lamina-core` reads every manifest and fails if any crate but `assistant` depends on an HTTP client or `ring`.

## The stack is Rust

The engine, the slicer, the AI chat, the CLI and the planned desktop are Rust. The desktop will be egui with eframe and wgpu, the
same stack as BIMCraft, so its camera and mesh pass can be lifted. There is no web view, no JavaScript and no Python in the app or
its build.

## Skills for the app's AI

The in-app assistant is a model behind a provider API, so what it knows about modelling for a printer has to come from its prompt
and its tools. Rather than one long system prompt, Lamina ships short skills: Markdown files in `crates/assistant/skills/`
that the model reads on demand with the `read_skill` tool, the way Claude's own skills work. The list costs a few lines per
request, a skill's text only when the model asks for it.

| Skill | Teaches |
|---|---|
| `modelling-with-lamina` | the frame (mm, plate centre, z up), the commands, what is missing (no booleans), the look, make, check, report order |
| `design-for-fdm` | wall thickness, overhangs, orientation, holes and fits, size limits, tied to what Lamina's slicer does and does not do |
| `slicing-with-lamina` | every setting with sensible ranges, `slice_check`, how to read the result |
| `work-from-a-picture` | blockout from a photo with honest limits |

`slice_check` exists for the AI: it runs the whole slice and writes nothing, so the model can verify a part (fits the plate, how long,
any warning) while the file-writing `slice` stays refused to it.

### Where the ideas came from

The skills are original text. These were read for workflow ideas only; none of their text or code is used, and several have no
clear licence (see [RESEARCH.md](RESEARCH.md)).

- **OmniSkill's `img2threejs`:** build in passes, blockout first, check against the picture, say what is guessed, quote the cost
  before a long job. That became `work-from-a-picture`. Its Three.js, WebGPU and Python code is not usable in a Rust app.
- **Community OpenSCAD and CadQuery skills:** infer and list the assumptions instead of interviewing, run arithmetic checks before
  looking at a render (bounding box, wall thickness, fits the bed), say when a fit is a declared default and not a measurement,
  prefer modifying an existing mesh to rebuilding it. These shaped the order in `modelling-with-lamina`.

## Slicing pipeline

`slice::layers` cuts a closed mesh at the middle of each layer. Segments are chained by the vertex pair of the edge they cross, not
by float equality, so adjacent triangles agree exactly. Loops wind counter-clockwise for material and clockwise for holes.
`print::plan` then insets the region for each wall, finds top and bottom skins by intersecting the infill areas of the layers above
and below, and fills with lines anchored to a global grid so layers and objects align. `Writer` turns strokes into moves, with
retraction on long travels and a flow model from line width, layer height and filament diameter.

## What is ported from BIMCraft

BIMCraft is `/mnt/ai/RevCraft` (github.com/IPedrax/BIMCraft), same author, MIT OR Apache-2.0.

| From BIMCraft | In Lamina | State |
|---|---|---|
| `crates/core/src/engine.rs`, `commands.rs` (command catalog, rollback, undo, script) | `core/engine.rs`, `commands.rs` | rewritten smaller: one `Session`, static command sets |
| `crates/core/src/bridge.rs` (read-only agents) | `core/bridge.rs` | trimmed to Hello, Execute, Script, Catalog |
| `crates/assistant` (3 providers, sealed key, attachments, turn loop) | `crates/assistant` | ported, 53 tests. Dropped Co-Work and saved chats |
| `apps/bimcraft-cli/src/mcp.rs` | `apps/lamina-cli/src/mcp.rs` | ported, own session only |
| `crates/core/src/mesh_in.rs` (OBJ and glTF readers, "knows no element") | not yet | next: lift as `core/obj.rs`, `core/gltf.rs` |
| `crates/ui-egui/src/orbit.rs`, `camera.rs`, `gpu.rs`, `gpu.wgsl`, `retained.rs` (wgpu 3D view, orbit/pan/zoom, picking) | not yet | next: the desktop viewport. These are welded to BIMCraft's `Scene` and `Model`, so lift the camera and the mesh pass, not the files |
| `crates/ui-egui` panels, `assistant.rs` (chat palette) | not yet | the chat window is the small piece to lift first |
| `crates/core/src/view3d.rs`, `view3d/gpu.rs` | not reused | BIM-specific (levels, elements, section boxes) |

The assistant's only coupling to the core was `Session`, `bridge::run`, `Access` and `find`, which is why porting it took a
trimmed bridge and a rewritten tool list rather than a rewrite.

## Decisions to revisit

- **Booleans.** `manifold3d` is the robust mesh boolean, but it builds C++ with cmake. `csgrs` is pure Rust and its robustness is
  unverified. Decide before the CSG commands. See [RESEARCH.md](RESEARCH.md).
- **Undo per chat turn.** The chat runs each command as its own undo step. BIMCraft merges a turn into one step
  (`Session::transaction`, `merge_steps`); Lamina's core needs the same.
- **Licence.** MIT today. BIMCraft is MIT OR Apache-2.0; dual-license if you want the patent grant.
