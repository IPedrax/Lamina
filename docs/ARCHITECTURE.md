# Architecture

## One engine, many front ends

`lamina_core::Session` owns the scene (the objects on the plate). Every change goes through `Session::execute(name, params)`:

1. The command is found by id (`commands::ALL`, plus any set a crate registers, like the slicer's).
2. It runs under `catch_unwind`. A panic, an error, or a scene that fails `Scene::check` restores the scene as it was.
3. A successful change that is `undoable` becomes an undo step (200 kept; a step is an `Arc` of the old scene, so it costs a pointer).

`bridge::run` sits on top for agents (the MCP server and the AI chat). It adds `Access::Read`, which refuses any command that
changes the scene or its history, and the ones in `bridge::REPLACES` (`new`, `open`), which put another plate in the user's place.

## Mesh repair

`repair.rs` mends what files and the booleans get wrong. `repair` drops doubled faces and skins, makes the winding consistent
(`orient` walks each shell across edges two faces share, turns the faces that disagree with the larger side, then turns a shell with
negative volume outward unless a ray test finds it inside another shell, where it is a void), and then runs the booleans' passes:
T-junctions split, fins dropped, open vertices snapped within 50 microns, holes filled with a fan when they have 16 edges or fewer or
are flat and convex. A sound mesh comes back untouched. `problems` counts open edges, edges of three or more faces and flipped faces
without changing anything; `info` reports them and the window shows a warning on the object.

## 3MF

`zip.rs` reads and writes just enough ZIP for 3MF packages (stored and deflated entries, no ZIP64 or encryption), checking every
offset and size against the bytes there are and each entry's CRC, and inflating only up to a limit. `threemf.rs` follows the 3MF Core
Specification: `_rels/.rels` names the model part; objects are meshes or components placing other objects (in other parts too, with
the Production Extension's `path`, as slicers write them); each build item becomes a Lamina object, its components gathered into one
mesh and its transform kept (3MF's row-vector matrix is the transpose of `Transform`'s). Units are read; a mirroring transform is
baked into the mesh with its faces turned. On import the group keeps its layout and is centred on the plate.

## Projects

`project.rs` writes and reads `.lamina` files: JSON with `format: 1`, each object's id, name, transform and local mesh (base64 of
little-endian `f32` vertices and `u32` triangles, lossless and compact), and `print`, the front end's settings, which the core keeps
without reading (`Session::print`). The slicer's commands lay `print.profile` under the settings a slice is given, so `open` then
`slice` prints a project as it was set up. A newer `format` is refused; a key this version does not know is left out and listed in
what `open` returns. `open` and `new` start a new undo history, as BIMCraft's do. Parameters are strict (`deny_unknown_fields`), and every number is range-checked.

```
lamina-cli (CLI, MCP) ─┐
lamina-assistant (chat) ─┼─> bridge::run ─> Session::execute ─> commands (core + slicer) ─> Scene
desktop (lamina) ──────┘
```

## Crates

| Crate | Holds | Depends on |
|---|---|---|
| `lamina-core` | `Mesh`, `Transform`, STL and 3MF read/write, `.lamina` projects, mesh repair, `Scene`, `Session`, `bridge`, built-in commands | serde, miniz_oxide, quick-xml |
| `lamina-slicer` | `slice` (mesh to loops), `geom` (polygon booleans and offsets on `i_overlay`), `print` (walls, infill, G-code), `profile`, the `slice` and `slicer_profile` commands | core, i_overlay |
| `lamina-csg` | `boolean` over csgrs' BSP trees, crack repair, the soundness gate, the `union`, `difference` and `intersect` commands | core, csgrs |
| `lamina-assistant` | provider clients, sealed key storage, tool layer, turn loop | core, ureq, ring |
| `lamina` (apps/lamina) | the desktop window: 3D view, objects panel, settings, layer preview, assistant panel | core, slicer, csg, assistant, eframe, rfd |
| `lamina-cli` | `lamina-cli` binary, MCP server | core, slicer, csg |

A test in `lamina-core` reads every manifest and fails if any crate but `assistant` depends on an HTTP client or `ring`.

## The stack is Rust

The engine, the slicer, the AI chat, the CLI and the planned desktop are Rust. The desktop will be egui with eframe and wgpu, the
same stack as BIMCraft, so its camera and mesh pass can be lifted. There is no web view, no JavaScript and no Python in the app or
its build.

## The desktop

`apps/lamina` is egui 0.36 on eframe (wgpu), the same stack as BIMCraft. The window is thin: it reads `session.scene` and changes it
only through `Session::execute`, so the assistant, a button and a script do the same thing the same way.

- **3D view (`view3d.rs`).** An orbit camera (drag to orbit, right or middle drag to pan, wheel to zoom) and a small CPU
  rasterizer that draws the plate, its 10 mm grid and the objects into an egui texture, redrawn only when the scene, the selection,
  the camera or the size changes. Each pixel keeps the id of the object drawn there, which is the click-to-pick. BIMCraft draws on
  the GPU (`crates/ui-egui/src/gpu.rs`, rendering into a texture egui shows); that is the upgrade for meshes past a few hundred
  thousand triangles, and the CPU path stays as the fallback, as it does there.
- **Move gizmo.** One picked object carries three axis arrows and a square to slide it on the plate, painted over the picture at a
  constant size on screen. A drag that starts on one moves the object along that line on screen (or across the level plane under the
  pointer), drawn where it would go and snapped to half a millimetre unless Shift is held; on release the window runs one `move`, so
  the whole drag is one undo step. A drag that starts anywhere else still orbits.
- **Objects and settings.** Add a box, cylinder or sphere (placed beside what is on the plate, in one undo step, through
  `add_primitive {at}`), import an STL, move, rotate, scale, drop, delete, and with several picked, join, subtract or intersect.
  The print settings edit the profile the slice uses.
- **Preview (`preview.rs`).** Slicing reads the G-code back into layers of moves, so the preview shows exactly what the printer
  will be told: one layer, the one below it dim, travels optional, zoomed to what is printed. It says when the plate has changed since.
- **Assistant (`chat.rs`).** The ported chat, with provider, model and key; the turn runs in the frame on the window's session.
  The key comes from the provider's environment variable or the box. Ticking Remember keeps it in the settings file, sealed by
  `lamina_assistant::seal`: by default under a secret built into the program, which the panel says plainly is scrambling and not
  protection from anyone who has Lamina; with a passphrase, AES-256-GCM under PBKDF2 (600,000 rounds), typed once a session and
  stored nowhere. This is BIMCraft's design, ported with the crate.
- **Projects (`files.rs`).** The File menu runs `new`, `open` and `save`. The window keeps the project's path, and the scene and
  print settings as they were when it was opened or saved: the plate has changed when the scene is another `Arc` (so undoing back to
  the saved plate is no change), the settings when their JSON differs. New, Open and the close button ask before losing changes.
  The window copies its print settings into `Session::print` each frame, so a saved project and the assistant's `slice_check` use the
  settings shown. Files dropped on the window are opened or imported by extension.
- **Settings (`settings.rs`).** One `settings.json` in the user's settings folder (`%APPDATA%\Lamina`,
  `~/Library/Application Support/Lamina`, or `~/.config/lamina`; `LAMINA_SETTINGS` overrides): the print settings, the presets last
  chosen, the user's own presets, the recent projects and the sealed keys. Read hostilely, a part that does not read is left out and said; written whole,
  to a file only its owner can read, then moved over the old one, and only when something changed and nothing is being dragged.
- **Presets.** Built in, in `lamina-slicer` (`presets.rs`), so the CLI, the MCP server and the assistant name them too (`slice
  {printer, filament}`, `slicer_presets`): printers by plate and firmware (Marlin, Klipper with `PRINT_START`, a centred delta) and
  filaments (PLA, PETG, ABS, ASA, TPU). A printer sets only the machine's fields, a filament only temperatures and fan, so they combine
  and leave the print settings alone. The values are typical figures of our own, not any maker's profile. The window adds presets of
  the user's own, saved from the settings shown.

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

## Booleans

`lamina-csg` is the only crate that names csgrs, so another engine can replace it behind `boolean(op, a, b)`. csgrs is a BSP library
(Evan Wallace's csg.js algorithm in Rust). How it is used:

1. **Flat faces go in whole.** Triangles that share an edge and lie exactly in one axis-square plane (a box's face, an upright
   cylinder's cap) go to the BSP as one polygon, because every inner edge is a place to split and a cap fanned from its centre is a ring
   of slivers. Only exactly flat patches: csgrs splits a polygon that is off its own plane by more than 1e-8 forever, until the stack
   overflows, which aborts the process. (Moving tilted corners onto their plane was tried and measured worse.)
2. **The whole shapes go through the BSP.** csgrs' own `union` and `difference` set aside faces whose boxes miss the other shape's
   box, and a shape wholly inside the other then has nothing to be clipped against. `combine` uses its public `Node` directly.
3. **The result is repaired** (`lamina_core::repair::close_with`, shared with import), in passes until it is closed or nothing changes: vertices are welded (a point within a micron of one
   already there is that one); triangles are split at vertices lying on their edges (T-junctions); pairs of identical triangles facing
   opposite ways, skins of no thickness where the shapes' surfaces coincide, are dropped; fins under 0.01 mm2 on edges three faces share
   are dropped; vertices on open edges within 50 microns are snapped together, which closes the slivers along the intersection curve;
   and holes are filled with a fan when they have 16 edges or fewer, or are flat and convex (a face the BSP lost). Every step iterates in
   a fixed order, so the same input is repaired the same way every run.
4. **A result is gated.** `sound` accepts only a closed solid whose volume is possible for inclusion and exclusion and whose bounds are
   inside what the operation allows (within the 50 micron snap). Anything else is an error that tells the user or the model what to try.

The sphere primitive is geodesic (a subdivided icosahedron, poles on z): a UV sphere's thin fans at the poles were what failed most.

Measured on random pairs with round-number offsets (`success_matrix`, 200 cases each way; `curved_score` takes the curved pairs alone):

| | Before the repair work | Now |
|---|---|---|
| Upright shapes, all pairs and operations | 307 of 400 (77%) | 199 of 200 |
| Tilted shapes | 276 of 400 (69%) | 180 of 200 (90%) |
| Curved pairs only, union and difference | 66 of 160 | 144 of 160 (90%) |

Upright, every cell is 100% but cylinder with sphere union (95%). The weak spot left is tilted cylinders: crossing each other (50%) or
joined to a box (71%), because a tilted cap cannot be merged into one flat polygon. Those are refused with advice; nothing broken is
returned. One risk remains: if csgrs ever recursed without end on some input, the stack overflow would abort the app rather than return
an error. It has not happened in thousands of measured booleans since merging was limited to exactly flat faces, but only running
booleans in a child process would make it impossible.

csgrs 0.20.1 also needs `core2`, every release of which is yanked, so `vendor/core2` stands in for it over `std::io` (the root
`Cargo.toml` patches it in); remove it when csgrs stops asking.

## Slicing pipeline

`slice::layers` cuts a closed mesh at the middle of each layer. Segments are chained by the vertex pair of the edge they cross, not
by float equality, so adjacent triangles agree exactly. Loops wind counter-clockwise for material and clockwise for holes.
`geom::region` simplifies each loop (Douglas and Peucker, 5 microns) before anything is offset: a mesh's faces leave points in a
straight line, and i_overlay's offset turns a corner of no turn into doubled points and spikes.
`print::plan` then insets the region for each wall, finds top and bottom skins by intersecting the infill areas of the layers above
and below, and fills with lines anchored to a global grid so layers and objects align. `Writer` turns strokes into moves, with
retraction on long travels and a flow model from line width, layer height and filament diameter.

Golden files (`crates/slicer/tests/golden/`) hold the G-code of a few small models. A change to the slicer that changes them fails
the test until they are re-blessed (`LAMINA_BLESS=1`), so every change in output is seen in a diff and judged a fix or a regression.

## What is ported from BIMCraft

BIMCraft is `/mnt/ai/RevCraft` (github.com/IPedrax/BIMCraft), same author, MIT OR Apache-2.0.

| From BIMCraft | In Lamina | State |
|---|---|---|
| `crates/core/src/engine.rs`, `commands.rs` (command catalog, rollback, undo, script) | `core/engine.rs`, `commands.rs` | rewritten smaller: one `Session`, static command sets |
| `crates/core/src/bridge.rs` (read-only agents) | `core/bridge.rs` | trimmed to Hello, Execute, Script, Catalog |
| `crates/assistant` (3 providers, sealed key, attachments, turn loop) | `crates/assistant` | ported, 53 tests. Dropped Co-Work and saved chats |
| `apps/bimcraft-cli/src/mcp.rs` | `apps/lamina-cli/src/mcp.rs` | ported, own session only |
| `crates/core/src/mesh_in.rs` (OBJ and glTF readers, "knows no element") | not yet | next: lift as `core/obj.rs`, `core/gltf.rs` |
| `crates/ui-egui/src/orbit.rs`, `camera.rs`, `gpu.rs`, `gpu.wgsl`, `retained.rs` (wgpu 3D view, orbit/pan/zoom, picking) | `apps/lamina/src/view3d.rs`, own and smaller | the same interaction, drawn on the CPU; the GPU pass is the upgrade |
| `crates/ui-egui` panels, `assistant.rs` (chat palette) | `apps/lamina/src/chat.rs` | rewritten small on the same chat, with the same Remember and passphrase; no Co-Work |
| `crates/core/src/view3d.rs`, `view3d/gpu.rs` | not reused | BIM-specific (levels, elements, section boxes) |

The assistant's only coupling to the core was `Session`, `bridge::run`, `Access` and `find`, which is why porting it took a
trimmed bridge and a rewritten tool list rather than a rewrite.

## Decisions to revisit

- **Booleans.** `manifold3d` is the robust mesh boolean, but it builds C++ with cmake. `csgrs` is pure Rust and its robustness is
  unverified. Decide before the CSG commands. See [RESEARCH.md](RESEARCH.md).
- **Undo per chat turn.** The chat runs each command as its own undo step. BIMCraft merges a turn into one step
  (`Session::transaction`, `merge_steps`); Lamina's core needs the same.
- **Licence.** MIT today. BIMCraft is MIT OR Apache-2.0; dual-license if you want the patent grant.
