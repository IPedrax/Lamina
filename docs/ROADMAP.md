# Roadmap

What is built, what is missing, and how to build each missing piece in this codebase. Every feature follows the project's rules
(`CLAUDE.md`): new behaviour is a command with a test, input is hostile, the slicer is clean-room, and the app stays Rust.

Priorities:

- **P0**: needed before Lamina is trusted on a real printer.
- **P1**: what makes it pleasant to use every day.
- **P2**: later, or for fewer users.

## Done

- Command engine with rollback, undo, scripts, a catalog agents read, and read-only access for agents.
- Meshes, primitives (box, cylinder, geodesic sphere), transforms, STL in and out, placing a shape `at` a point.
- Slicer: layers, walls, solid and sparse infill, skirt, retraction, flow, time and filament estimates, Marlin-style G-code.
- Booleans (`union`, `difference`, `intersect`) on csgrs, with crack repair and a gate that refuses broken results.
- Printer and filament presets, built in and the user's own.
- CLI and MCP server.
- The AI chat, with three providers, skills, `slice_check`, and a key that can be remembered sealed.
- The desktop app: a 3D view with orbit, picking and a move gizmo; the objects' tools; print settings and presets; slicing with a layer
  preview; G-code export; the assistant panel; a settings file.
- **1.1 Project files.** `crates/core/src/project.rs`: `.lamina` JSON with `format: 1`, each object's id, name, transform and local
  mesh (base64 little-endian arrays), and the print settings and preset labels (`Session::print`, which the slicer lays under what a
  slice is given). `save`, `open` and `new` commands; `open` and `new` start a new history, are in `bridge::REPLACES` (refused to a
  read-only agent) and are refused to the window's assistant. Unknown keys are left out and listed, a newer format is refused. The
  CLI opens a `.lamina` given to `run`, `info` or `slice`.
- **0.1 Golden G-code tests.** `crates/slicer/tests/golden/`: a cube, a tube, a T with overhanging arms and a two-object project,
  each sliced and compared line by line (numbers to just under their written precision, so CI on three systems agrees);
  `LAMINA_BLESS=1` writes the new output to be read in the diff. The first run found spikes and doubled points in walls on round
  shapes (offsetting points in a straight line), fixed by simplifying loops before they are offset.
- **1.4 3MF import and export.** `crates/core/src/zip.rs` (stored and deflated entries over `miniz_oxide`, every offset checked,
  inflated up to a limit, CRC checked) and `threemf.rs` (the model part read with `quick-xml`: meshes, components, units, the
  Production Extension's `path` for objects in other parts, build items with their transforms). `import_mesh` reads a `.3mf` with
  each build item as its own object, laid out as in the file and centred on the plate; `export_mesh` writes one, positions for a
  printer whose zero is the plate's corner. Other slicers' settings in a package are left alone.
- **2.2 Mesh repair.** `crates/core/src/repair.rs` holds the passes the booleans used (T-junctions, skins, fins, snapping, hole
  filling), now shared, and a new one that makes winding consistent across shared edges and turns each shell outward unless it lies
  inside another (a void). `import_mesh` repairs on the way in and says what it did; `repair {id}` mends an object, as one undo step.
- **2.3 Report problems clearly.** `info` gives `open_edges`, `nonmanifold_edges` and `flipped_faces` for each object; the plate
  panel shows a warning sign with the problems on hover, and a Repair button for the picked object.
- **4.1 Supports.** `crates/slicer/src/support.rs`: where a layer leans past `support_angle` from vertical, all of it that is over
  air is held up; overhangs are carried down to the model or the plate (`support`: `off`, `everywhere`, `from_plate`), keep
  `support_xy_gap` from the walls and stop `support_z_gap` below the model, with `support_interface_layers` of denser lines on top.
  Sparse lines run one way so layers stack; the interface crosses them. A golden T is sliced with supports.
- **7.1 File menu and drag and drop.** New, Open, Open recent, Save, Save as (with Ctrl shortcuts), Import mesh, Export G-code and
  Quit; the title shows the project and a star for unsaved changes; New, Open and closing ask first when something would be lost;
  files dropped on the window are opened (`.lamina`) or imported (meshes). `apps/lamina/src/files.rs`.

## Order of work

The P0 items, in the order that unblocks the most:

1. Project files (1.1). Done.
2. Golden G-code tests and real test prints (0.1, 0.2). The golden tests are done; the test prints need hardware.
3. Mesh repair on import, and 3MF import (2.2, 2.3, 1.4). Done.
4. Supports and brim (4.1, 4.2). Without them, most real parts fail on the plate.
5. Connected infill and seam placement (4.3, 4.4), so prints are clean and do not string.
6. One undo step per chat turn (0.3), and continuous integration (8.1).
7. Send to Moonraker and OctoPrint (6.1, 6.2).

After those, the P1 items below, roughly top to bottom within each area.

---

## 0. Foundations

**0.2 Real test prints (P0).** How: export a 20 mm calibration cube, a 0.4 mm single-wall box and a bridge test with `lamina-cli slice`.
Print each on one Marlin and one Klipper printer. Measure with calipers. Write the results, with photos, in `docs/PRINT-TESTS.md`. Fix
what is wrong before adding features.

**0.3 One undo step per chat turn, and for group actions (P0).** Today every command the assistant runs is its own undo step, and so is
each object in a group delete. How: add `Session::transaction(label, f)` to `crates/core/src/engine.rs`, which runs several commands
and pushes one step holding the scene from before the first. BIMCraft's `engine.rs` has `transaction` and `merge_steps` to follow. Use
it in `crates/assistant/src/chat.rs` around each turn, and in the desktop for group delete and moving several objects. Test: a turn of
three commands is taken back by one `undo`.

**0.4 Booleans in a child process (P1).** csgrs recurses, and if it ever recursed without end the stack overflow would abort the whole app
instead of returning an error. How: run each boolean in a child process. Start the app's own binary with a hidden `--boolean-child`
flag, pass the two meshes on stdin as binary STL, and read the result from stdout, with a timeout. BIMCraft's `crates/dwg` does exactly
this (`in_child`, `child_main`) for its DWG reader. Test: a child that is killed returns an error, and the app carries on.

**0.5 Tilted cylinders in booleans (P1).** These are the last weak case: about 50% of tilted cylinder pairs are refused. How: a tilted
cap is only flat to f32 rounding, because the world mesh is stored as f32 after the transform. Give `lamina_csg::boolean` each
object's local mesh and its `Transform`. Merge flat patches in the local frame, where caps are exactly flat, then move the polygon
corners to world space in f64. Re-measure with `success_matrix` and `curved_score`, which are ignored tests in `crates/csg`.

**0.6 Fuzz the parsers (P1).** How: for each parser (STL, settings file, sealed key, G-code preview, command parameters), add a test
that feeds a few thousand random and mutated inputs from a seeded generator and asserts it never panics. No new dependency: the csg
crate's tests already have a small RNG to reuse.

## 1. Files and projects

**1.2 Autosave (P1).** How: every few minutes, when the scene changed, write the project to the settings folder. Offer it at the next
start if the app did not close cleanly. BIMCraft's `autosave.rs` has the pattern.

**1.3 OBJ and glTF import (P1).** How: lift `crates/core/src/mesh_in.rs` from BIMCraft (same author, MIT OR Apache-2.0). It already
reads OBJ and glTF from their public specifications, in millimetres with z up, with caps, and "knows no element". Dispatch on the file
extension in `import_mesh`. Test with small files written in the test, as BIMCraft's tests do.

**1.5 OBJ export (P2).** How: write each object's world mesh as plain text. (3MF export came with 1.4.)

## 2. Meshes

**2.1 Mirror (P1).** How: `Mesh::transformed` must reverse each triangle's winding when the transform's determinant is negative. Add
a `det()` to `Transform`, then a `mirror {id, axis}` command in `crates/core/src/commands.rs`. Test: a mirrored mesh stays watertight
with positive volume.

**2.4 Simplify heavy meshes (P2).** Imported scans can have millions of triangles. How: quadric edge collapse (Garland and Heckbert's
public algorithm) in `crates/core/src/simplify.rs`, behind a `simplify {id, triangles}` command. The `meshopt` crate is C++ through
FFI, so this would be our own implementation.

## 3. Modelling

**3.1 Rotate and scale handles (P1).** How: in `apps/lamina/src/view3d.rs`, next to the move gizmo. Draw a ring per axis, projected as a
polyline of points around the object's centre. A drag on a ring turns by the change in angle of the pointer around the projected
centre, snapped to 5 degrees, and on release runs `rotate`. Add a corner handle for uniform scale: the ratio of the pointer's distances
from the centre, run as `scale` on release. Test: the same drag-and-assert tests as the move gizmo.

**3.2 Move several objects (P1).** Needs 0.3. How: the gizmo stands at the middle of the selection's bounds, and on release one
transaction moves each object.

**3.3 Lay flat on a face (P1).** How: make `Picture` keep a triangle index per pixel beside the object id. A "lay flat" click then
gives a face, whose world normal `n` is turned to point down (axis `n x (0,0,-1)`, angle `acos(-n.z)`). Add a core command
`lay_flat {id, face}` that rotates and drops the object to the plate.

**3.4 Arrange (P1).** How: an `arrange {gap?}` command in core. Take each object's footprint rectangle, sort by height, and pack them
in rows (a shelf packer) inside the plate, centred. Test: no two footprints overlap and all stay on the plate.

**3.5 Duplicate (P1).** How: `duplicate {id, count?}` in core. It clones the object with a new id (the mesh `Arc` is shared, so it
costs nothing), then runs `arrange` on the copies.

**3.6 Cut with a plane (P1).** How: `cut {id, z}` first, a general plane later. Make two booleans with a box larger than the object,
one above the plane and one below, and replace the object with both halves. This reuses `lamina-csg`, so the halves are closed.

**3.7 Absolute position and size fields (P1).** How: in the plate panel, show the object's position and size and let them be typed.
On enter, run `move` by the difference, or `scale` with `by` set to the ratio. Add a lock for keeping proportions.

**3.8 More shapes (P1).** How: in `crates/core/src/mesh.rs` and `add_primitive`. A cone or frustum is the cylinder with a second radius.
Add a torus (two angles), an n-sided prism or pyramid, a tube built directly (no boolean needed), and a box with rounded edges, built
directly as a rounded box rather than a fillet. Test each for watertightness and volume, as the existing primitives are tested.

**3.9 Sketch and extrude (P1).** A flat outline lifted into a solid covers most mechanical parts. How: an
`extrude {outline: [[x,y]...], holes?, height}` command. Clean the outline with `i_overlay` (already a slicer dependency; move the
shared part to core, or depend on it from core) and triangulate caps with holes using `spade` (MIT or Apache-2.0) or `earcutr`. Then
join the caps with side walls. Add helpers for common outlines: rounded rectangle, slot, circle with holes.

**3.10 Text (P2).** How: read glyph outlines from a font with `ttf-parser` (BIMCraft uses it, with IBM Plex under the OFL, which may be
bundled with its licence). Flatten the curves to polylines and extrude with 3.9. Engrave or emboss with a boolean.

**3.11 Fillets and chamfers (P2).** A general mesh fillet is research-level work. How: offer them where they are tractable. Rounded
and chamfered boxes are built directly (3.8). Chamfer the bottom edge of any object with a boolean against a wedge ring. Say plainly in
the AI's skills what is not possible.

**3.12 Parametric recipes (P1).** These let the AI change "make the hole 6 mm" instead of rebuilding. How: add an optional `recipe` to
an object. It is a list of the commands that made it (primitives, transforms, booleans, extrudes) with their parameters. Add
`build {recipe}`, which runs them and keeps the recipe, and `edit {id, step, params}`, which changes one step and rebuilds. Store the
recipe in the project file. The modelling skill teaches the AI to edit recipes rather than redo work.

**3.13 Measure (P1).** How: keep the depth buffer in `Picture`, so a click gives a world point (unproject with `Camera::ray`). Measure
between two clicked points in the window, and add a core `measure {a, b}` for agents.

## 4. Slicer

**4.1b Tree supports (P2).** Branches that grow from the plate toward each overhang, using less plastic than the straight columns of 4.1.

**4.2 Brim and raft (P0 for brim).** How: a brim is outward offsets of each object's first-layer outer contours, touching the part,
`brim_width / line width` loops. It is the skirt code with distance 0, per object. A raft is a few layers of coarse lines under the
object, with the object lifted by the raft's height plus a gap. Fields: `brim_width`, `raft_layers`.

**4.3 Connected infill (P0).** Today every infill line is its own stroke, with a retraction between long travels. How: after clipping,
join each line's end to the next line's start with a short path along the region's boundary. Use it only when it stays inside the
region, checked by clipping it with `geom::clip_lines`. Travel and retract only when no such path exists. Test: retractions on a
filled square drop from one per line to a handful.

**4.4 Seam placement (P0).** How: in `print.rs`, choose where each wall loop starts with a `seam` field. `aligned` starts nearest the
previous layer's seam. `rear` uses the point with the greatest y. `sharpest` uses the most concave corner. Today the loop starts
nearest the head.

**4.5 More infill patterns (P1).** How: an `infill_pattern` field.
- Grid: both directions on every layer.
- Triangles: three directions.
- Gyroid: run marching squares on `sin x cos y + sin y cos z + sin z cos x = 0` at the layer's z, scaled to the density, and clip the
  contours to the region.
- Honeycomb: offset hexagon polylines.

The gyroid is strong in every direction and the most asked for.

**4.6 Bridging (P1).** How: find solid infill that lies over air (the solid region not covered by the layer below). Print it with
`bridge_speed`, `bridge_flow` and the fan on. Choose the direction by trying angles and keeping the one with the shortest lines across
the gap.

**4.7 Gap fill and thin walls (P1).** Today a wall thinner than a line vanishes. How: what the wall insets leave unprinted, wider than
a quarter line, is printed as single lines along its centre. Find the centreline with a straight skeleton or a Voronoi diagram:
`boostvoronoi` is BSL-1.0 and permissive, BIMCraft has a straight skeleton (`skeleton.rs`). Vary the flow by width. Full
variable-width walls (as in the public Arachne paper) are P2.

**4.8 Cooling (P1).** How: a `min_layer_time` field. When a layer would print faster than that, slow its moves to meet it. Add a fan
ramp over the first layers and full fan on bridges.

**4.9 Travel moves (P1).** How:
- Combing: route a travel inside the region instead of across a gap, so it needs no retraction.
- Z-hop: lift on retraction, with a `z_hop` field.
- Wipe: move along the last wall while retracting.

Test: the number of retractions and the travel distance on a two-object plate.

**4.10 Better time estimates (P1).** How: printer fields for maximum acceleration and junction deviation, Marlin's or Klipper's.
Estimate each move with a trapezoid speed profile instead of distance over speed. Compare with the printer's real time from 0.2.

**4.11 Vase mode (P1).** How: one wall, no top, and z raised a little along each point of the loop, so it never stops. It is cheap
to add and popular.

**4.12 Firmware details (P1).**
- Klipper: object markers (`EXCLUDE_OBJECT` lines) so one failed object can be cancelled.
- Thumbnails: comment blocks of a small PNG drawn by the existing CPU rasterizer, which Klipper's front ends and some firmwares show.
- Pressure advance: `M900` (Marlin), `SET_PRESSURE_ADVANCE` (Klipper) or `M572` (RepRapFirmware), from a filament field.
- RepRapFirmware: `G10` firmware retraction.

**4.13 Feature types in the G-code (P1).** How: write a comment before each kind of move (outer wall, inner wall, solid, sparse,
skirt, support). The 3D preview colours by it (7.3), and the AI can count them.

**4.14 Per-object settings (P1).** How: an optional map of profile fields on each object, set by `set_object_settings {id, profile}`.
The slicer lays it over the plate's profile for that object only: infill, walls, layers. Modifier meshes (settings for a region inside
a part) are P2.

**4.15 Adaptive layer height (P2).** How: choose each layer's height from the steepest surface it crosses, thin where faces are close to
flat. `slice::layers` takes a function from z to height instead of one number.

**4.16 Ironing, fuzzy skin, arc fitting (P2).**
- Ironing: a low-flow pass over top surfaces.
- Fuzzy skin: random offsets on the outer wall's points.
- Arc fitting: replace runs of points on a circle with `G2`/`G3` within a tolerance, when the firmware supports it.

**4.17 Multi-material and sequential printing (P2).** These need tool changes, a purge tower, an extruder per object, and for
sequential printing a check of the print head's clearance.

## 5. Profiles and settings

**5.1 Process presets (P1).** How: a third kind in `crates/slicer/src/presets.rs`: "0.12 mm Fine", "0.20 mm Standard", "0.28 mm
Draft", "Strong" (more walls and infill). Each sets only layer and strength fields, so the three kinds combine.

**5.2 All settings in the window (P1).** How: the print settings panel shows only some fields. Add collapsible groups for speeds,
retraction, skirt and brim, cooling, and a start and end G-code editor that lists the placeholders. Each field shows its limits from
`Profile::check`.

**5.3 Preset inheritance (P2).** How: a user preset may name another in `inherits`, and its fields are laid over that one's. Resolve
the chain with a depth limit, and refuse cycles.

**5.4 Import other slicers' profiles (P2).** How: read an OrcaSlicer JSON preset or a PrusaSlicer `.ini` the user chooses, and map the
keys we understand to our fields through a table we write. Report the keys left out. Never bundle their profile files, whose licence
is unclear.

**5.5 Expressions in start G-code (P2).** How: `{expr}` with arithmetic, comparisons and `if`, beside today's `[name]`. Use a small
recursive-descent evaluator in `profile.rs`, with a length limit.

## 6. Printers

**6.1 Send to Moonraker, for Klipper (P0).** How: a new crate `crates/printer` that the desktop and the CLI use. It does
`POST /server/files/upload` (multipart, `print=true`) to the address the user typed, and nothing else. It is the second crate that
reaches the network, so add it to the allow list of the `network_boundary` test in `crates/core/src/lib.rs`. Its request goes only to
the configured host. Desktop: a "Send to printer" button and a printer address in the settings.

**6.2 Send to OctoPrint (P0).** How: the same crate. `POST /api/files/local` (multipart, `print=true`) with the key in `X-Api-Key`.
The key is sealed like the assistant's.

**6.3 Printer status (P2).** How: poll Moonraker's or OctoPrint's job endpoints for progress and temperatures, and show them in the
window.

**6.4 More printers (P2).** PrusaLink (`PUT /api/v1/files/...`), Duet (`rr_upload` or DSF), Bambu over the LAN (MQTT and FTPS, now
limited by Bambu's own authorization), and USB serial with line numbers and resends. Each is its own module in `crates/printer`.

## 7. Desktop

**7.2 Status of the window's work (P1).** How: slicing and booleans can take seconds. Run them on a thread, as the chat already runs
its requests, with a spinner and Cancel. The UI stays responsive.

**7.3 3D G-code preview (P1).** How: draw the extrusion moves up to the chosen layer as lines in the 3D view, with depth. The CPU
rasterizer already draws depth-tested lines (`Raster::line`). Colour them by feature type from 4.13, and keep the 2D layer view.

**7.4 GPU viewport (P2).** The CPU view slows past a few hundred thousand triangles. How: render the scene with wgpu into a texture
egui shows (`register_native_texture`), as BIMCraft's `crates/ui-egui/src/gpu.rs` does, and keep the CPU path as the fallback.

**7.5 Comfort (P1).** Keyboard shortcuts (G to move, R to rotate, F to frame), a right-click menu on objects, an undo history list,
tooltips on every field, and a light theme.

**7.6 Translations (P2).** How: lift BIMCraft's `crates/i18n` (PO files compiled in, English fallback) and pass every interface string
through it.

**7.7 Packaging and releases (P1).** How: lift BIMCraft's `packaging/` and release workflow to build an AppImage, an MSI and a DMG on
each tag. Set `LAMINA_KEY_SECRET` as a CI secret, so a release's sealed keys are not under the public development secret. An updater
(BIMCraft's `crates/update`) is P2.

**7.8 Logging and crash safety (P1).** How: a log file in the settings folder, and a fallback when the graphics adapter fails at
start. BIMCraft's `apps/bimcraft/src/startup.rs` has both.

## 8. Quality and infrastructure

**8.1 Continuous integration (P0).** How: `.github/workflows/ci.yml` runs `cargo fmt --check`,
`cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` on Linux, Windows and macOS, on every push and pull
request. The ignored tests (`success_matrix`, `curved_score`, `picture_of_the_window`) stay manual.

**8.2 Benchmarks (P2).** How: ignored tests that time slicing a 100,000-triangle mesh and a boolean of two detailed shapes, printed so
regressions are seen.

**8.3 User guide (P1).** How: `docs/USER-GUIDE.md` with screenshots from `picture_of_the_window`. Cover the first print, presets, the
assistant and its key, and what each setting does.

**8.4 Licence (P1).** Decide whether to stay MIT or move to MIT OR Apache-2.0 as BIMCraft is (Apache adds a patent grant). Generate a
full third-party notice for releases, for example with `cargo about`.

## 9. The assistant

**9.1 A picture the model can see (P1).** It lets the AI compare its part with a photo, as the OpenSCAD and CadQuery skills do. How:
add a `preview {azimuth?, elevation?}` tool in `crates/assistant/src/tools.rs`. It draws the scene with the desktop's CPU rasterizer,
so move `view3d::draw` and `Camera` into core or a small shared crate, and returns a PNG. Anthropic accepts images in tool results.
For OpenAI and Gemini, send the image in a follow-up user message. Test with the fake network: the request carries an image part.

**9.2 More skills (P1).** Troubleshooting (stringing, warping, layer shift), calibration (flow, temperature, retraction), materials,
and supports once 4.1 exists. Each is a Markdown file in `crates/assistant/skills/`, and the existing test checks every command it
names.

**9.3 Local models (P1).** These keep everything on the machine. How: an "OpenAI-compatible" provider with a base URL limited to
`localhost` or `127.0.0.1` (Ollama, LM Studio). Add it to `net::allowed` by address, not by name, with a test that any other host is
refused.

**9.4 Agents on the open window (P1).** How: let an external agent drive the window's plate through `lamina-cli mcp --connect`. Lift
BIMCraft's control port: `bridge.rs` request parsing with a token, `apps/bimcraft-cli/src/window.rs`, and the window's listener. It is
local only, the token is in a file beside the settings, and read-only is an option.

**9.5 Saved chats (P2).** How: keep each project's conversation beside it. BIMCraft's `saved.rs` was removed from the port for lack of
project files and can return after 1.1.

**9.6 Streaming replies (P2).** How: read each provider's server-sent events, so text appears as it is written. It is a change to
`net.rs` and the three provider parsers.

## Known limits today

- Walls are constant width, so thin features vanish instead of getting gap fill. Supports are straight columns, not trees. No bridging.
- Infill lines are separate strokes. The skirt is an outline of the first layer, not a hull. Time is distance over speed, without acceleration.
- A mesh whose holes are too big to fill without guessing stays open, and prints with parts missing, with a warning. STL and 3MF only (OBJ and glTF are 1.3).
- Booleans refuse about half of tilted cylinder pairs.
- Nothing has been printed on real hardware.
