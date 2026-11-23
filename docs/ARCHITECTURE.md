# Architecture

## One engine, many front ends

`lamina_core::Session` owns the scene (the objects on the plate). Every change goes through `Session::execute(name, params)`:

1. The command is found by id (`commands::ALL`, plus any set a crate registers, like the slicer's).
2. It runs under `catch_unwind`. A panic, an error, or a scene that fails `Scene::check` restores the scene as it was.
3. A successful change that is `undoable` becomes an undo step (200 kept; a step is an `Arc` of the old scene, so it costs a pointer).

`Session::transaction(label, f)` runs several commands as one step, all or nothing, and `merge_steps(from, to)` joins the steps of
one name on top of the history: the chat names each reply's transaction after its turn and merges after each reply, so a turn is
one undo step however many replies it takes, and the window's group delete is one step.

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
| `lamina-core` | `Mesh`, `Transform`, STL and 3MF read/write, OBJ and glTF read, `.lamina` projects, mesh repair, sketches, `Scene`, `Session`, `bridge`, built-in commands | serde, miniz_oxide, quick-xml, i_overlay, i_triangle |
| `lamina-slicer` | `slice` (mesh to loops), `geom` (polygon booleans and offsets on `i_overlay`), `print` (walls, infill, G-code), `profile`, the `slice` and `slicer_profile` commands | core, i_overlay |
| `lamina-csg` | `boolean` over csgrs' BSP trees, crack repair, the soundness gate, the `union`, `difference` and `intersect` commands | core, csgrs |
| `lamina-assistant` | provider clients, sealed key storage, tool layer, turn loop | core, ureq, ring |
| `lamina-printer` | uploads to Moonraker and OctoPrint, the `send_to_printer` command | core, ureq |
| `lamina` (apps/lamina) | the desktop window: 3D view, objects panel, settings, layer preview, assistant panel, printer connection | core, slicer, csg, assistant, printer, eframe, rfd |
| `lamina-cli` | `lamina-cli` binary, MCP server | core, slicer, csg, printer |

A test in `lamina-core` reads every manifest and fails if any crate but `assistant` and `printer` depends on an HTTP client, or any but
`assistant` on `ring`.

## Printers

`lamina-printer` sends a G-code file to the printer the user names: Moonraker's `POST /server/files/upload` or OctoPrint's
`POST /api/files/local`, one multipart request each, with `print` set when the user asks it to start. The address is checked first
(http or https, a host, a port and a path, nothing else), the key goes only in `X-Api-Key`, no redirect is followed and the reply is
read up to 1 MB. `send_to_printer` reads a file, so the assistant (which refuses every command that touches a file) never runs it, and
the MCP server's session does not register it: starting a print is a physical act only the user takes. Tests run a fake printer on
this machine (`testing::printer`); nothing contacts a real one.

## The stack is Rust

The engine, the slicer, the AI chat, the CLI and the planned desktop are Rust. The desktop will be egui with eframe and wgpu, the
same stack as BIMCraft, so its camera and mesh pass can be lifted. There is no web view, no JavaScript and no Python in the app or
its build.

## The desktop

`apps/lamina` is egui 0.36 on eframe (wgpu), the same stack as BIMCraft. The window is thin: it reads `session.scene` and changes it
only through `Session::execute`, so the assistant, a button and a script do the same thing the same way.

- **Preview (`preview.rs`).** The G-code is read back into layers of moves, each extrusion with its kind from the `;TYPE:`
  comments. In 3D every layer up to the chosen one is drawn by `lamina_core::render::draw_paths`, each line as wide as it prints
  and shaded round, coloured by kind with a legend, lower layers a little darker; the camera steers as the 3D view's does. The
  chosen layer alone from above, with its travels, is the other view.
- **Long work (`work.rs`).** Slicing, the booleans and the cut, writing G-code and sending it run on a thread, as the chat's requests
  do, so the window keeps drawing; the status bar shows a spinner, the time so far and Cancel. A job runs on a sibling session that
  shares the scene, so it still goes through `Session::execute`, and its new scene comes back with `Session::adopt` as one undo
  step, refused if the plate changed meanwhile. While a job that changes the plate runs, the window's own commands wait. Cancel sets
  the job's flag (`lamina_core::cancel`, a thread-local the slicer checks each layer and the boolean's child wait checks every
  5 ms, ending the child) and the window forgets the job.

- **3D view (`view3d.rs`).** The camera and rasterizer are `lamina_core::render`, shared with the G-code thumbnails and the
  assistant's picture. An orbit camera (drag to orbit, right or middle drag to pan, wheel to zoom) and a small CPU
  rasterizer that draws the plate, its 10 mm grid and the objects into an egui texture, redrawn only when the scene, the selection,
  the camera or the size changes. Each pixel keeps the id of the object drawn there, which is the click-to-pick. BIMCraft draws on
  the GPU (`crates/ui-egui/src/gpu.rs`, rendering into a texture egui shows); that is the upgrade for meshes past a few hundred
  thousand triangles, and the CPU path stays as the fallback, as it does there.
- **Gizmo.** Rings turn the picked objects about an axis (by the angle the pointer sweeps round the middle, snapped to 5 degrees) and
  a corner square scales them; on release one transaction runs `rotate` or `scale` on each. The arrows and plate square move them:
  the picked objects (one or several, at the middle of their bounds) carry three axis arrows and a square to slide
  them on the plate, painted over the picture at a
  constant size on screen. A drag that starts on one moves the object along that line on screen (or across the level plane under the
  pointer), drawn where it would go and snapped to half a millimetre unless Shift is held; on release the window runs one `move`
  (several in one transaction), so the whole drag is one undo step. The panel also shows the picked object's position and size as
  fields to type into. A drag that starts anywhere else still orbits.
- **Tools.** Move (the gizmo), Lay flat and Measure, chosen in the toolbar. The picture keeps each pixel's object, triangle and
  depth, so a click gives the object, the face (which `lay_flat` turns down) or the point (unprojected from the depth; two measure).
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
  {printer, filament, process}`, `slicer_presets`). Processes set only the layers and the strength: printers by plate and firmware (Marlin, Klipper with `PRINT_START`, a centred delta) and
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

| | Before the repair work | With the repair | Flat faces found in each object's own frame |
|---|---|---|---|
| Upright shapes, all pairs and operations | 307 of 400 (77%) | 199 of 200 | 199 of 200 |
| Tilted shapes | 276 of 400 (69%) | 180 of 200 (90%) | 198 of 200 (99%) |
| Curved pairs only, union and difference | 66 of 160 | 144 of 160 (90%) | 159 of 160 |

Tilted cylinders were the weak spot (crossing each other 50%, joined to a box 71%): a tilted cap, stored as f32 after the transform, is
flat only to f32's rounding and could not be merged into one polygon. `boolean_placed` takes each object's own mesh and transform,
merges flat patches in the object's frame, where the cap is exactly flat, and moves the corners where they stand in f64, flat to
f64's rounding, far inside csgrs' 1e-8. Upright, every cell is 100% but cylinder with sphere union (95%); tilted, every cell but sphere
with sphere union (2 of 4). What is refused is refused with advice; nothing broken is returned. If csgrs ever recursed without end on some input, the stack overflow would abort the process: so the desktop and the CLI run
each boolean in a copy of themselves (`child.rs`, `--boolean-child`, ported from BIMCraft's DWG reader), and a child that aborts or
runs past two minutes is an error the app carries on from. Tests, and apps that do not ask, run booleans in the process.

csgrs 0.20.1 also needs `core2`, every release of which is yanked, so `vendor/core2` stands in for it over `std::io` (the root
`Cargo.toml` patches it in); remove it when csgrs stops asking.

## Slicing pipeline

`slice::layers` cuts a closed mesh at the middle of each layer. Segments are chained by the vertex pair of the edge they cross, not
by float equality, so adjacent triangles agree exactly. Loops wind counter-clockwise for material and clockwise for holes.
`geom::region` simplifies each loop (Douglas and Peucker, 5 microns) before anything is offset: a mesh's faces leave points in a
straight line, and i_overlay's offset turns a corner of no turn into doubled points and spikes.
`print::plan` then insets the region for each wall, finds top and bottom skins by intersecting the infill areas of the layers above
and below, and fills with lines anchored to a global grid so layers and objects align. `connect` joins each line's end to the next
line's start, straight or along the boundary both lie on, when that path is short and clipping shows it stays inside the region. `Writer` turns strokes into moves, with
retraction on long travels and a flow model from line width, layer height and filament diameter.

`support::plan` works on the layer regions of one object. A layer whose part beyond the layer below grown by `height x
tan(support_angle)` is not empty is too steep there; what is held up is all of that layer over air near the steep part, so a slope gets
one area and not a comb of thin strips. Overhangs are carried down, clipped by the model (support rests on it), or with `from_plate`
kept only where each layer's column continues to the plate. The printed area keeps an xy gap from the walls, a z gap of whole layers
below the model, and loses what is narrower than a line; its top layers are a denser interface.

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
| `crates/core/src/mesh_in.rs` (OBJ and glTF readers, "knows no element") | `core/mesh_in.rs` | lifted with its reader tests; `import_mesh` makes objects of what it reads |
| `crates/core/src/autosave.rs` | `apps/lamina/src/autosave.rs` | simpler: one window, one file |
| `crates/dwg` (`in_child`, `child_main`, `run_child`) | `crates/csg/src/child.rs` | the same pattern, for booleans |
| `crates/ui-egui/src/orbit.rs`, `camera.rs`, `gpu.rs`, `gpu.wgsl`, `retained.rs` (wgpu 3D view, orbit/pan/zoom, picking) | `apps/lamina/src/view3d.rs`, own and smaller | the same interaction, drawn on the CPU; the GPU pass is the upgrade |
| `crates/ui-egui` panels, `assistant.rs` (chat palette) | `apps/lamina/src/chat.rs` | rewritten small on the same chat, with the same Remember and passphrase; no Co-Work |
| `crates/core/src/view3d.rs`, `view3d/gpu.rs` | not reused | BIM-specific (levels, elements, section boxes) |

The assistant's only coupling to the core was `Session`, `bridge::run`, `Access` and `find`, which is why porting it took a
trimmed bridge and a rewritten tool list rather than a rewrite.

## Decisions to revisit

- **Booleans.** `manifold3d` is the robust mesh boolean, but it builds C++ with cmake. `csgrs` is pure Rust and its robustness is
  unverified. Decide before the CSG commands. See [RESEARCH.md](RESEARCH.md).
- **Licence.** MIT today. BIMCraft is MIT OR Apache-2.0; dual-license if you want the patent grant.
