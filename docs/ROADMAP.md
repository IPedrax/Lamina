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
- **0.3 One undo step per chat turn, and for group actions.** `Session::transaction(label, f)` runs several commands as one undo
  step, all or nothing, and `merge_steps` joins steps of one name (as BIMCraft's engine does). The chat runs each reply's calls in
  a transaction named for its turn and merges the turn's steps, so one undo takes back a whole turn over all its replies. The
  window's group delete (the Delete key, and Delete all for several picked objects) is one step.
- **8.1 Continuous integration.** `.github/workflows/ci.yml` (adapted from BIMCraft's) runs `cargo fmt --check`, clippy with
  `-D warnings` and `cargo test --workspace` on Linux, Windows and macOS for every push and pull request, with the build cache saved
  from `main`. The ignored tests stay manual. Not yet seen running: pushing was refused overnight (see `QUESTIONS.md`).
- **0.4 Booleans in a child process.** `crates/csg/src/child.rs`, ported from BIMCraft's `crates/dwg`: the desktop and the CLI
  run each boolean in a copy of themselves started with `--boolean-child`, the meshes on its standard input (lossless arrays) and the
  result on its standard output, stopped after two minutes. A child that aborts or hangs is an error and the app carries on.
  Tested with the real `lamina-cli` as the child.
- **0.5 Tilted cylinders in booleans.** `lamina_csg::boolean_placed` takes each object's local mesh and transform, merges flat
  patches in the local frame and moves corners to world space in f64; the commands and the child process pass both. Measured with
  the same random shapes: tilted pairs 180 of 200 before, 198 now; curved pairs 144 of 160 before, 159 now.
- **0.6 Fuzz the parsers.** `lamina_core::fuzz` (a seeded xorshift and mutations: flipped bits, odd bytes, cuts, repeats,
  splices) feeds thousands of hostile inputs to STL, projects, ZIP and 3MF, base64, every core command's parameters (no panic inside,
  the scene still valid), slicer profiles and G-code placeholders, the three providers' replies, sealed keys, the G-code preview
  and the settings file. None panicked.
- **1.1 Project files.** `crates/core/src/project.rs`: `.lamina` JSON with `format: 1`, each object's id, name, transform and local
  mesh (base64 little-endian arrays), and the print settings and preset labels (`Session::print`, which the slicer lays under what a
  slice is given). `save`, `open` and `new` commands; `open` and `new` start a new history, are in `bridge::REPLACES` (refused to a
  read-only agent) and are refused to the window's assistant. Unknown keys are left out and listed, a newer format is refused. The
  CLI opens a `.lamina` given to `run`, `info` or `slice`.
- **0.1 Golden G-code tests.** `crates/slicer/tests/golden/`: a cube, a tube, a T with overhanging arms and a two-object project,
  each sliced and compared line by line (numbers to just under their written precision, so CI on three systems agrees);
  `LAMINA_BLESS=1` writes the new output to be read in the diff. The first run found spikes and doubled points in walls on round
  shapes (offsetting points in a straight line), fixed by simplifying loops before they are offset.
- **1.2 Autosave.** Every two minutes while the plate has unsaved changes, the window writes the project to `autosave.lamina`
  beside the settings file, on a thread. A clean exit or a save removes it; at the next start a file still there is offered back
  (Bring it back, as an unsaved project, or Discard it). `apps/lamina/src/autosave.rs`, a simpler cousin of BIMCraft's.
- **1.3 OBJ and glTF import.** `crates/core/src/mesh_in.rs`, lifted from BIMCraft with its tests: OBJ (with `units` and `up`,
  millimetres and z up by default) and glTF 2.0 (`.gltf` and `.glb`, metres and y up as the specification says), each object of the
  file a Lamina object, laid out as in the file and centred on the plate, repaired on the way in. A material library or buffer is
  read only from the file's folder or below. What the reader leaves out is listed in `left_out`. A sheet is no longer closed into a
  solid of no volume by the repair.
- **1.4 3MF import and export.** `crates/core/src/zip.rs` (stored and deflated entries over `miniz_oxide`, every offset checked,
  inflated up to a limit, CRC checked) and `threemf.rs` (the model part read with `quick-xml`: meshes, components, units, the
  Production Extension's `path` for objects in other parts, build items with their transforms). `import_mesh` reads a `.3mf` with
  each build item as its own object, laid out as in the file and centred on the plate; `export_mesh` writes one, positions for a
  printer whose zero is the plate's corner. Other slicers' settings in a package are left alone.
- **2.1 Mirror.** `Transform::det`, `Mesh::transformed` turns the winding when a transform mirrors, and `mirror {id, axis}`
  mirrors an object about its centre (a Mirror button in the window). Projects keep mirrored transforms; booleans take mirrored
  objects the right way out (corners reversed, normals turned).
- **2.2 Mesh repair.** `crates/core/src/repair.rs` holds the passes the booleans used (T-junctions, skins, fins, snapping, hole
  filling), now shared, and a new one that makes winding consistent across shared edges and turns each shell outward unless it lies
  inside another (a void). `import_mesh` repairs on the way in and says what it did; `repair {id}` mends an object, as one undo step.
- **2.3 Report problems clearly.** `info` gives `open_edges`, `nonmanifold_edges` and `flipped_faces` for each object; the plate
  panel shows a warning sign with the problems on hover, and a Repair button for the picked object.
- **3.1 Rotate and scale handles.** The gizmo has a ring per axis (a circle about the middle, square to the axis) and a corner
  square. A drag on a ring turns by the angle the pointer sweeps round the middle on screen, the way the axis is seen, snapped to 5
  degrees; a drag on the square scales by how much farther the pointer is from the middle, snapped to 0.05. The picked objects are
  drawn changing, and on release one transaction runs `rotate` or `scale` on each.
- **3.2 Move several objects.** The gizmo stands at the middle of the picked objects' bounds, draws them all moving, and on
  release one transaction moves each, so one undo takes the drag back.
- **3.7 Absolute position and size fields.** The picked object's footprint middle and bottom, and its size, as fields; a value
  typed (or dragged and let go) runs `move` by the difference or `scale` by the ratio, with Keep proportions.
- **3.3 Lay flat on a face** and **3.13 Measure.** The 3D view's picture keeps each pixel's triangle and depth beside its object.
  With the Lay flat tool a click gives a face, and `lay_flat {id, face}` turns its normal straight down (`Transform::rotate_axis`,
  Rodrigues' formula) and sets the object on the plate; with the Measure tool two clicks give two points (unprojected from the
  depth), drawn joined, and `measure {a, b}` gives the distance, also for agents.
- **3.4 Arrange** and **3.5 Duplicate.** `arrange {ids?, gap?, bed?}` lays footprints out in rows (a shelf packer, deepest first),
  centred on the plate, and says whether they fit; `duplicate {id, count?, arrange?}` adds copies sharing the mesh and lays the
  plate out. Arrange and Duplicate buttons in the window.
- **3.6 Cut with a plane.** `cut {id, z, keep?}` in `lamina-csg`: a difference and an intersection with a box larger than the object
  above the plane; the lower part stays, the upper is turned onto its cut face and set beside it, both closed. A general plane is
  for later. A Cut control in the window.
- **3.8 More shapes.** `add_primitive` kinds `cone` (and frustum, with `radius_top`), `prism`, `pyramid`, `tube` (built directly),
  `torus` and `rounded_box` (each face's grid pushed out from the box shrunk by the radius, so edges are quarter cylinders and
  corners eighths of a sphere). Each is tested closed with its volume; the window has a More shapes menu.
- **3.9 Sketch and extrude.** `crates/core/src/sketch.rs` and `extrude {height, outline | shape, holes?, round_holes?}`: the
  outline cleaned by `i_overlay` (now a core dependency too), each shape's cap triangulated by `i_triangle` (MIT OR Apache-2.0, the
  same author's, on the same `i_overlay`), walls along the caps' boundary edges, each shape welded on its own so touching shapes stay
  manifold. Helpers: `rounded_rect`, `slot`, `circle`, and round holes by diameter. A plate with holes in the window's More shapes.
- **3.12 Parametric recipes.** An object's optional `recipe` (the commands that made it). `build {recipe}` runs the steps on an
  empty plate of their own (`Session::sibling`, the same commands; no file, replace, undo or nested build), which must end with one
  object, and keeps the recipe; `edit {id, step, params}` merges new parameters into one step and rebuilds the part where it
  stands. Projects keep recipes, `info` shows them, and the modelling skill teaches editing a step rather than redoing work.
- **4.1 Supports.** `crates/slicer/src/support.rs`: where a layer leans past `support_angle` from vertical, all of it that is over
  air is held up; overhangs are carried down to the model or the plate (`support`: `off`, `everywhere`, `from_plate`), keep
  `support_xy_gap` from the walls and stop `support_z_gap` below the model, with `support_interface_layers` of denser lines on top.
  Sparse lines run one way so layers stack; the interface crosses them. A golden T is sliced with supports.
- **4.2 Brim and raft.** `brim_width`: loops outward from the outer walls of everything on the first layer, touching it, merged
  where objects are close; the skirt moves out past it. `raft_layers`: a coarse base, crossing middle layers and a solid top, 2 mm
  beyond the footprint, with the objects lifted 0.15 mm above it. Golden cases for both.
- **4.3 Connected infill.** After clipping, each infill, support and raft line's end is joined to the next line's start by the
  straight path, or else the way along the boundary both ends lie on, when it is within three line spacings and clipping shows it
  stays inside the region. Retractions in the golden files fell by about 40 %; a 20 mm cube retracts about twice a layer.
- **4.4 Seam placement.** `seam`: `aligned` (the default: each loop starts nearest the seam below that is nearest it, the first
  layer at the rear), `rear` (greatest y, the leftmost of ties), `sharpest` (the sharpest inside corner, else the sharpest corner,
  rearmost of near ties) or `nearest` (where the head is, as before). Skirt and brim loops follow the same rule.
- **4.5 More infill patterns.** `infill_pattern` (`crates/slicer/src/infill.rs`): lines, grid (two directions each layer), triangles
  (three at 60 degrees, a triangular lattice), gyroid (marching squares on Schoen's surface at the layer's height, its period set so
  the share filled matches the density) and honeycomb (a hexagon tiling, each edge once). Each is tested to fill within 15 % of the
  density asked for; the new patterns are joined only along the boundary. A golden gyroid cube.
- **4.6 Bridging.** The solid infill not over the layer below (without slivers) is a bridge: lines at the angle, in steps of 15
  degrees, whose ends rest on the layer below the most (a point a line's width past each end is tested), else the shortest; printed
  at `bridge_speed` with `bridge_flow` and `bridge_fan`, the fan set back after. A golden span between two pillars.
- **4.7 Gap fill and thin walls.** `crates/slicer/src/gap.rs`: what the walls (each a band a line wide round its path) and the
  infill (where at least a line wide) leave, wider than a quarter line, is printed along its middle: the gap's outline split at its
  two points farthest apart, one side walked in half-line steps, each centre halfway to the other side, the width there setting
  the flow (in runs of similar width). A fin thinner than a line now prints. The centre is a fair approximation for strips, not a
  straight skeleton; walls of varying width (Arachne) are P2. A golden fin.
- **4.8 Cooling.** `min_layer_time` (5 s): a layer whose moves would take less is slowed in proportion, no move below
  `min_print_speed`. The fan is off on the first layer and comes up in equal steps to `fan_speed` by `fan_full_layer`; bridges
  have their own fan (4.6).
- **4.9 Travel moves.** Combing (`combing`, on): a travel whose straight path stays inside the object's region on that layer needs
  no retraction, however long (routing round holes is for later). `z_hop` lifts on each retraction and lowers before the next
  extrusion; `wipe_distance` spreads the retraction over a move along the stroke just printed. Retractions in the golden files
  fell by about half again (a 20 mm cube now retracts once a layer).
- **4.10 Better time estimates.** `crates/slicer/src/time.rs`: each move a trapezoid (or a triangle) at `max_acceleration`, corners
  as the junction deviation allows (Marlin's documented formula; Klipper's square corner velocity converts), a backward pass so
  every move can stop and a forward pass so every speed is reachable. The Klipper presets set 3000 mm/s2. Not yet compared with a
  real print (0.2).
- **4.11 Vase mode.** `vase`: the bottom layers as usual, then each layer's outer wall alone, its points rising in proportion to
  the way round from the last layer's height to its own (`G1 X Y Z E`), so z never stops; no infill, no top, no layer-change
  retraction. A warning with several objects. A golden square vase.
- **4.12 Firmware details.** `firmware` (marlin, klipper, reprap): Klipper gets `EXCLUDE_OBJECT_DEFINE` (each object's box) and
  `EXCLUDE_OBJECT_START`/`END` round each object's moves on each layer, the skirt and brim outside them. `pressure_advance` (a
  filament field) as `M900 K`, `SET_PRESSURE_ADVANCE ADVANCE=` or `M572 D0 S`. `firmware_retraction` uses `G10` and `G11`.
  `thumbnail`: a PNG of the plate drawn by `lamina_core::render`, base64 in a `; thumbnail begin` comment block; the Klipper
  presets set 300 pixels.
- **4.13 Feature types in the G-code.** Every stroke has a kind, written as `;TYPE:outer-wall` (inner-wall, solid-infill,
  sparse-infill, skirt, brim, support, support-interface, raft) before it, again after each layer change. `slice_check` returns
  the filament of each kind (`features`), so the assistant can count them.
- **4.14 Per-object settings.** An object's optional `settings` (kept in projects, shown by `info`), set by
  `set_object_settings {id, profile}` with the fields an object may change (walls, skins, infill and its pattern, seam, speeds,
  supports); `print::gcode_each` slices each object with its settings over the plate's. Layer heights stay plate-wide, as objects
  share layers. A small "This object's settings" panel in the window. Modifier meshes are P2.
- **5.1 Process presets.** A third kind in `presets.rs`, setting only the layers and the strength (`PROCESS_FIELDS`): 0.12 mm Fine,
  0.20 mm Standard, 0.28 mm Draft and Strong. `slice` and `slice_check` take `process` (after the printer and the filament, before
  `profile`); `slicer_presets` lists them; the window has a Process picker and Save process, kept in the settings file and the
  project.
- **5.2 All settings in the window.** Every number of the profile has its limits in one table, `profile::LIMITS`, which
  `Profile::check` reads (a test checks every number is in it) and the window reads for each field's range, shown on hover. The
  print settings panel has groups for speeds, retraction and travel, the skirt, cooling, support details and the machine, and a
  start and end G-code editor that lists the placeholders and says when one is unknown.
- **6.1 Send to Moonraker** and **6.2 Send to OctoPrint.** `crates/printer`: one multipart upload to the address the user typed
  (`/server/files/upload` with `root` and `print`, or `/api/files/local` with `select` and `print` and the key in `X-Api-Key`),
  no redirects, the reply read up to 1 MB, the address checked (http or https, no credentials, query or fragment). The
  `send_to_printer` command reads a G-code file, so the assistant never runs it; `lamina-cli run` has it, the MCP server does not.
  The window's Printer connection keeps the kind and address, and the key only when Remember is ticked, sealed as the
  assistant's is; Send to printer and Send and print slice the plate and upload it. Tested against a fake printer on this machine.
  The second crate allowed on the network (`network_boundary` in `lamina-core`).
- **7.1 File menu and drag and drop.** New, Open, Open recent, Save, Save as (with Ctrl shortcuts), Import mesh, Export G-code and
  Quit; the title shows the project and a star for unsaved changes; New, Open and closing ask first when something would be lost;
  files dropped on the window are opened (`.lamina`) or imported (meshes). `apps/lamina/src/files.rs`.
- **7.2 The window's work on a thread.** Slicing, booleans and the cut, writing G-code and sending it run on a thread
  (`apps/lamina/src/work.rs`), with a spinner, the seconds so far and Cancel on the status bar. A job runs on a sibling session and
  its scene comes back as one undo step (`Session::adopt`), refused if the plate changed meanwhile; the window's own commands wait
  for a job that changes the plate. Cancel sets a flag (`lamina_core::cancel`) the slicer checks each layer and a boolean's child
  wait checks, ending the child.
- **7.3 3D G-code preview.** The preview reads each extrusion's kind from the `;TYPE:` comments and draws every layer up to the
  chosen one in 3D (`lamina_core::render::draw_paths`: each line as wide as it prints, shaded round, depth-tested), coloured by kind
  with a legend; the layer view from above stays one click away. Triangles and lines are now cut at the near plane, so the plate
  seen from close by is drawn up to the eye in the 3D view too.
- **7.5 Comfort.** Shortcuts: G, L and M pick the move, lay flat and measure tools (rotating is the gizmo's rings, in the move tool),
  R turns the picked objects 90 degrees, F frames them, A arranges, Ctrl+D duplicates, Ctrl+A picks all. A right-click menu on
  objects in the 3D view and the plate list. Edit, History lists the undo and redo steps (`Session::history`). Every setting has a
  tooltip from `profile::ABOUT` (a sentence for each field, checked by a test) and its range. A light theme, or the system's, in the
  View menu, kept in the settings file. Vase mode is in the window now.
- **7.7 Packaging and releases.** `packaging/` and `.github/workflows/release.yml`, lifted from BIMCraft's: on a `v*` tag, an
  MSI (WiX 5, installed and removed again in the job), an AppImage and a tarball for x86_64 and aarch64 Linux, and a universal
  DMG with the CLI zipped beside it (signed and notarized when the Apple secrets are set, else ad hoc), all tested first and
  published with SHA256SUMS. Builds take `LAMINA_KEY_SECRET` from a repository secret and fail without it. The icon is drawn in
  Rust (`apps/lamina/src/icon.rs`) and written to `assets/app-icon/` as PNG, ICO and ICNS; the window uses it too. `lamina
  --version` lets the packages check the program starts. The updater is 7.9.
- **7.8 Logging and crash safety.** `apps/lamina/src/startup.rs`, lifted from BIMCraft's: `lamina.log` beside the settings file
  (each start, the graphics adapter, panics with a backtrace, a failed start, and each error the status bar shows), a message box
  on Windows when the window cannot open, and after a start that died, the next tries another graphics backend (DirectX 12,
  Vulkan, OpenGL on Windows), keeping the one that stays up.
- **8.3 User guide.** `docs/USER-GUIDE.md`: installing, the window, the first print, arranging and changing parts, presets of your
  own, the assistant and its key, sending to a printer, projects and autosave, where Lamina keeps things, and a table of every
  setting made from `profile::ABOUT`, `LIMITS` and the defaults (`crates/slicer/tests/guide.rs` checks it; `LAMINA_BLESS=1`
  writes it again). Its pictures are drawn by Lamina's CPU renderer (`docs_pictures`, ignored); a picture of the whole window
  still needs a machine with a graphics card (docs/QUESTIONS.md).
- **8.4 Licence and third-party notice.** MIT stays until the owner decides otherwise (docs/QUESTIONS.md). `tools/notices`
  (`lamina-notices`) reads `cargo metadata`, refuses any crate whose licence offers no permissive choice (a test runs it on the
  whole tree, every platform's crates), and writes THIRD-PARTY-NOTICES.txt: every crate with its licence and repository, then each
  distinct licence text once with the crates that ship it. Each release job writes it, and the MSI, the AppImage, the tarball and
  the DMG carry it. Written in Rust rather than with `cargo about`, so the release needs no tool installed.
- **9.1 A picture the model can see.** The assistant's `preview {azimuth?, elevation?}` tool draws the plate with
  `lamina_core::render` (640 by 448 pixels, framed on the objects) and the model gets the PNG: inside the tool result for
  Anthropic, in a user message after the results for OpenAI and Gemini. Tested on the fake network, each provider's request
  carrying its image part. The picture skill uses it to compare a part with the user's photo, and the chat panel gains Attach (and
  takes a picture dropped on the window), so a photo can go with a message.

## Order of work

The P0 items, in the order that unblocks the most:

1. Project files (1.1). Done.
2. Golden G-code tests and real test prints (0.1, 0.2). The golden tests are done; the test prints need hardware.
3. Mesh repair on import, and 3MF import (2.2, 2.3, 1.4). Done.
4. Supports and brim (4.1, 4.2). Done.
5. Connected infill and seam placement (4.3, 4.4). Done.
6. One undo step per chat turn and continuous integration (0.3, 8.1). Done.
7. Send to Moonraker and OctoPrint (6.1, 6.2). Done.

After those, the P1 items below, roughly top to bottom within each area.

---

## 0. Foundations

**0.2 Real test prints (P0).** How: export a 20 mm calibration cube, a 0.4 mm single-wall box and a bridge test with `lamina-cli slice`.
Print each on one Marlin and one Klipper printer. Measure with calipers. Write the results, with photos, in `docs/PRINT-TESTS.md`. Fix
what is wrong before adding features.

## 1. Files and projects

**1.5 OBJ export (P2).** How: write each object's world mesh as plain text. (3MF export came with 1.4.)

## 2. Meshes

**2.4 Simplify heavy meshes (P2).** Imported scans can have millions of triangles. How: quadric edge collapse (Garland and Heckbert's
public algorithm) in `crates/core/src/simplify.rs`, behind a `simplify {id, triangles}` command. The `meshopt` crate is C++ through
FFI, so this would be our own implementation.

## 3. Modelling

**3.10 Text (P2).** How: read glyph outlines from a font with `ttf-parser` (BIMCraft uses it, with IBM Plex under the OFL, which may be
bundled with its licence). Flatten the curves to polylines and extrude with 3.9. Engrave or emboss with a boolean.

**3.11 Fillets and chamfers (P2).** A general mesh fillet is research-level work. How: offer them where they are tractable. Rounded
and chamfered boxes are built directly (3.8). Chamfer the bottom edge of any object with a boolean against a wedge ring. Say plainly in
the AI's skills what is not possible.

## 4. Slicer

**4.1b Tree supports (P2).** Branches that grow from the plate toward each overhang, using less plastic than the straight columns of 4.1.

**4.15 Adaptive layer height (P2).** How: choose each layer's height from the steepest surface it crosses, thin where faces are close to
flat. `slice::layers` takes a function from z to height instead of one number.

**4.16 Ironing, fuzzy skin, arc fitting (P2).**
- Ironing: a low-flow pass over top surfaces.
- Fuzzy skin: random offsets on the outer wall's points.
- Arc fitting: replace runs of points on a circle with `G2`/`G3` within a tolerance, when the firmware supports it.

**4.17 Multi-material and sequential printing (P2).** These need tool changes, a purge tower, an extruder per object, and for
sequential printing a check of the print head's clearance.

## 5. Profiles and settings

**5.3 Preset inheritance (P2).** How: a user preset may name another in `inherits`, and its fields are laid over that one's. Resolve
the chain with a depth limit, and refuse cycles.

**5.4 Import other slicers' profiles (P2).** How: read an OrcaSlicer JSON preset or a PrusaSlicer `.ini` the user chooses, and map the
keys we understand to our fields through a table we write. Report the keys left out. Never bundle their profile files, whose licence
is unclear.

**5.5 Expressions in start G-code (P2).** How: `{expr}` with arithmetic, comparisons and `if`, beside today's `[name]`. Use a small
recursive-descent evaluator in `profile.rs`, with a length limit.

## 6. Printers

**6.3 Printer status (P2).** How: poll Moonraker's or OctoPrint's job endpoints for progress and temperatures, and show them in the
window.

**6.4 More printers (P2).** PrusaLink (`PUT /api/v1/files/...`), Duet (`rr_upload` or DSF), Bambu over the LAN (MQTT and FTPS, now
limited by Bambu's own authorization), and USB serial with line numbers and resends. Each is its own module in `crates/printer`.

## 7. Desktop

**7.4 GPU viewport (P2).** The CPU view slows past a few hundred thousand triangles. How: render the scene with wgpu into a texture
egui shows (`register_native_texture`), as BIMCraft's `crates/ui-egui/src/gpu.rs` does, and keep the CPU path as the fallback.

**7.6 Translations (P2).** How: lift BIMCraft's `crates/i18n` (PO files compiled in, English fallback) and pass every interface string
through it.

**7.9 Updater (P2).** How: lift BIMCraft's `crates/update`, which checks the releases and installs the MSI, the AppImage or the
DMG, each checked against the SHA-256 digest GitHub gives the asset. The release names in `release.yml` are what it would look for.

## 8. Quality and infrastructure

**8.2 Benchmarks (P2).** How: ignored tests that time slicing a 100,000-triangle mesh and a boolean of two detailed shapes, printed so
regressions are seen.

## 9. The assistant

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

- Walls are constant width; a thin feature gets one gap-fill line, not walls of varying width. Supports are straight columns, not trees.
- The skirt is an outline of the first layer, not a hull. The time estimate has not been checked against a real print.
- A mesh whose holes are too big to fill without guessing stays open, and prints with parts missing, with a warning. STL, 3MF, OBJ and glTF in; STL and 3MF out.
- Booleans refuse about 1 in 100 random pairs (the rest of a sphere-with-sphere union mostly), and a boolean of a boolean's result
  works on its world mesh, so its tilted faces are flat only to f32.
- Nothing has been printed on real hardware.
