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
- **1.5 OBJ export.** `export_mesh` to a `.obj` path writes each object's world mesh as its own `o` group, in millimetres, z up,
  or y up with `up: "y"` for programs that take y as up; it reads back through `import_mesh` with the same `up`. The window's
  Export mesh offers it.
- **2.1 Mirror.** `Transform::det`, `Mesh::transformed` turns the winding when a transform mirrors, and `mirror {id, axis}`
  mirrors an object about its centre (a Mirror button in the window). Projects keep mirrored transforms; booleans take mirrored
  objects the right way out (corners reversed, normals turned).
- **2.2 Mesh repair.** `crates/core/src/repair.rs` holds the passes the booleans used (T-junctions, skins, fins, snapping, hole
  filling), now shared, and a new one that makes winding consistent across shared edges and turns each shell outward unless it lies
  inside another (a void). `import_mesh` repairs on the way in and says what it did; `repair {id}` mends an object, as one undo step.
- **2.3 Report problems clearly.** `info` gives `open_edges`, `nonmanifold_edges` and `flipped_faces` for each object; the plate
  panel shows a warning sign with the problems on hover, and a Repair button for the picked object.
- **2.4 Simplify heavy meshes.** `crates/core/src/simplify.rs`: quadric edge collapse written from Garland and Heckbert's paper,
  with the link condition (the surface stays manifold), a check that no face turns over or shrinks to nothing, and heavy planes
  along open borders so holes do not grow. `simplify {id, triangles | ratio}` is one undo step; the object panel offers it for a
  mesh over 1000 triangles, on a thread. About 164,000 triangles go to 8,000 in under a second; a closed mesh stays closed.
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
- **3.14 Cut into pieces with connectors, and split to fit.** `cut {id, axis, at, connectors?, lay_out?}` cuts along x, y or z
  (`{id, z}` still works) and keeps the pieces where they stood unless asked to lay them out. Connectors are automatic by default:
  pins where the face has room (pegs on one piece and sockets with clearance in the other, nothing loose), else none with a note;
  or dowels (holes in both pieces and a loose chamfered pin each), or none. They are placed on the cut face (the slicer's section, shrunk by the sections down each hole's
  depth and inset by its radius and a wall) by farthest-point sampling, two or more so the pieces cannot turn, a square key where only
  one fits. `split_to_fit {id, bed?, height?, connectors?, gap?}` cuts an object too big for the printer into as few equal pieces
  as fit, longest side first, keeps connectors off the other cuts, turns each piece to print and arranges the plate. Tested through
  the pieces' sections: holes line up with holes and pegs with sockets. A Cut into pieces section in the window, with the plane
  drawn over the 3D view, and both commands in the modelling skill. Cuts are axis-aligned; a tilted plane is for later.
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
- **3.10 Text.** `text {text, height, depth, spacing?, at?}`: glyph outlines from IBM Plex Mono Regular (bundled under the OFL, its
  licence in every package) read with `ttf-parser`, curves cut into steps, lines one under another, extruded by the new
  `sketch::extrude_contours` (non-zero winding, so a letter's holes are holes). A boolean engraves or embosses it; tested through
  the CLI, the box stays closed. In the window under More shapes.
- **3.11 Fillets and chamfers, where tractable.** Rounded boxes, rounded corners on any extruded outline and tapered cones exist,
  and the modelling skill now says plainly that there is no fillet or chamfer for an arbitrary edge. A chamfer of the bottom edge
  of any object, against elephant's foot, is still open: it needs the object's bottom outline lofted to an inset of itself.
- **4.1 Supports.** `crates/slicer/src/support.rs`: where a layer leans past `support_angle` from vertical, all of it that is over
  air is held up; overhangs are carried down to the model or the plate (`support`: `off`, `everywhere`, `from_plate`), keep
  `support_xy_gap` from the walls and stop `support_z_gap` below the model, with `support_interface_layers` of denser lines on top.
  Sparse lines run one way so layers stack; the interface crosses them. A golden T is sliced with supports.
- **4.1b Tree supports.** `support_style: "tree"` (`columns`, the default, is 4.1). The columns' interface stays; under the bottom
  of each column's top go tips `tree_tip_spacing` apart (one per cell of a grid, fewer on a slope, whose bottom is a strip on each
  layer), walked down a layer at a time. Each leans at most `tree_branch_angle` toward the nearest branch it can still meet, and
  keeps out of the region from which the plate cannot be reached (the model grown by the xy gap, and its shadow shrunk by the lean
  for each layer up). Branches that meet go on as one, as thick as the square root of their tips times a line width (plus about a
  degree's growth on the way down), up to `tree_max_radius`, printed as a loop with lines inside. A branch that cannot reach the
  plate stands on the model, or with `from_plate` is dropped with its tips. Under a T 20 mm tall with arms 12 mm out the tree
  takes 71 mm of filament against the columns' 244 (the interface, 16 mm, alike); a ball on a stalk 142 against 175; a ball on
  the plate about the same. Settings in the window and the skills. Not yet printed.
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
- **4.15 Adaptive layer height.** `adaptive_layers` and `min_layer_height`: `slice::adaptive_tops` chooses the plate's layer tops
  from every object's sloped faces (a face whose normal makes `n_z` with the vertical allows at most `min / n_z`, walls and flat
  faces ask nothing, growth at most half a layer per layer), and each object is sliced at those tops (`slice::layers_at`), so the
  objects stay level. A process preset field, in the window beside the layer height. Uniform slicing is unchanged.
- **4.16 Ironing and fuzzy skin.** `ironing` (with `ironing_flow`, `ironing_spacing`, `ironing_speed`): last on each layer, close
  lines at right angles to the top skin over what no layer above covers, `;TYPE:ironing`. `fuzzy_skin` (`fuzzy_thickness`,
  `fuzzy_spacing`): the outer wall's points resampled and moved across it by a seeded generator, so a slice is the same each time;
  never on the first layer. Both may be set per object, and are in the window's Surface group. Arc fitting is 4.16b, below.
- **4.16b Arc fitting.** `arc_fitting` and `arc_tolerance`: the writer turns a run of four or more points on one circle (within
  the tolerance at the points and at each chord's middle, turning one way, radius 0.5 to 500 mm) into one `G2` or `G3` with
  `I J`, its plastic the arc's length; a hexagon stays a hexagon. The time planner still walks the points, and the preview's
  reader draws arcs. A 128-sided rod's G-code is a quarter shorter with the same plastic.
- **4.17 Several extruders.** `extruders` (1 to 8) and an `extruder` for each object (a per-object field, from 1). Each layer
  prints one extruder's objects, then the next's, the one in use first, so a tool changes at most once per extruder per layer. A
  change retracts, sets the old tool to its standby (`standby_temp_delta` below its own, off after its last layer), runs
  `toolchange_gcode` (with `[previous_tool]`, `[next_tool]`, `[layer_z]`), writes `T<n>` and waits: `M104 T`/`M109 T` on Marlin
  and Klipper (both document the `T` index), `G10 P R S` and `M116 P` on RepRapFirmware. `extruder_temps` gives the extruders
  after the first their own temperature. The purge tower (`tower_x`, `tower_y`, `tower_size`) is refused off the plate or over an
  object's box; each change wipes across it, then lays `purge_volume` mm3 as lines taken in turn from its inside
  (`;TYPE:purge`), and the rest of the layer is one line in three (`;TYPE:purge-tower`), on every layer up to the last change; a
  layer whose purges do not fit is refused. The tower is in the footprint, so a brim, a raft and the skirt go round it too. One
  extruder slices as before (the golden files are unchanged). In the window: an Extruders group, the tool change G-code, and
  Extruder in This object's settings. Tool offsets are left to the firmware (Marlin `M218`, RepRapFirmware `G10 P X Y`, a
  Klipper macro).
- **4.17b Sequential printing.** `sequential` prints each object whole, the shortest first, with the print head's clearance
  (`clearance_radius` round the nozzle, `gantry_height` above its tip, printer preset fields). It refuses, naming them, two
  objects whose bounding boxes stand closer than the radius, an object other than the tallest that is taller than the gantry, and
  objects on different extruders. Between objects the head retracts, rises 2 mm above the tallest printed (at most to
  `max_height`), travels to the next object's first point at that height and only then goes down, so the low moves stay inside
  the next object's own reach. The skirt and brim go round everything on the first object's first layer; the fan starts off again
  on each object's first layer. A One at a time group in the window.
- **4.18 The skirt round the convex hull.** The skirt is an offset of the convex hull (`geom::hull`, Andrew's monotone chain) of
  everything on the first layer (the outer walls, the brim or the raft, the purge tower), as PrusaSlicer and Cura draw theirs: one
  closed loop for each of `skirt_loops`, round all the objects at once, never into a notch. Tested with an L and two cubes apart
  (each loop convex, every wall inside it, `skirt_distance` from the outer wall); the golden bridge and two-object cases now have one
  skirt loop round both parts.
- **4.19 Supports planned across the plate.** Every object is sliced before any support is planned, and `support::plan` takes
  `solid`, all that stands on each of the plate's layers, beside the object's own layers: an overhang carried down stops on another
  object, keeps the xy gap from it, is dropped with `from_plate` where it meets one, and a tree goes round it. Objects are planned
  in order and a later one's support leaves out what an earlier one's already covers, so none is printed twice. With `from_plate`, a
  tree's branch is also cut back where it would hang over what is below it, so its edge never rests on a model's top. Objects
  printed one at a time stand apart and are planned alone. Tested with a T whose arm reaches over a lower box: in the G-code, columns
  and trees put no support in the box and stand on its top; from the plate only, nothing over it; two copies of a T in one place
  share one support.
- **4.20 A tree's interface only where its tips hold it.** After the walk, each interface layer of a tree keeps only what lies within
  three quarters of a tip's spacing of a tip that was kept or that rests on the model, from a tip's spacing above it to the
  interface's own layers below; a branch dropped with `from_plate` takes its part of the interface with it. Tested with a roof over
  a solid block and one beside it: from the plate only, nothing is left over the block while the roof beside it keeps its interface;
  everywhere, both keep theirs. A T's arms keep the whole interface as before.
- **4.21 The purge tower and one-at-a-time printing by outlines.** The tower is refused only where it overlaps what an object prints
  (its layers and their supports) on a layer the tower rises through, and the error names the object and the layer: an L's notch
  holds it, and so does the room under a shelf above the last tool change, while a support standing there is refused. One at a time
  measures the distance between the objects' convex hulls seen from above (`geom::hull_distance`) against `clearance_radius`, so two
  wedges whose boxes overlap but whose slanted sides are 28 mm apart print with a smaller clearance. Both cases were refused before.
- **5.1 Process presets.** A third kind in `presets.rs`, setting only the layers and the strength (`PROCESS_FIELDS`): 0.12 mm Fine,
  0.20 mm Standard, 0.28 mm Draft and Strong. `slice` and `slice_check` take `process` (after the printer and the filament, before
  `profile`); `slicer_presets` lists them; the window has a Process picker and Save process, kept in the settings file and the
  project.
- **5.2 All settings in the window.** Every number of the profile has its limits in one table, `profile::LIMITS`, which
  `Profile::check` reads (a test checks every number is in it) and the window reads for each field's range, shown on hover. The
  print settings panel has groups for speeds, retraction and travel, the skirt, cooling, support details and the machine, and a
  start and end G-code editor that lists the placeholders and says when one is unknown.
- **5.3 Preset inheritance.** A user's preset may name another, the user's or a built-in, in `inherits`; `presets::resolve` lays
  each over the one it inherits, at most eight deep, and says when a chain comes back to itself or names a preset nobody has.
  Saved over the preset chosen, a new one keeps only what differs from it, so a change to the base carries over.
- **5.4 Import other slicers' profiles.** `crates/slicer/src/import.rs` and `import_profile {path}`: a PrusaSlicer `.ini` or an
  OrcaSlicer or Bambu Studio JSON preset (values as text, lists of one per extruder), mapped through a table we wrote from their
  published setting names, never their code, and no profile file of theirs bundled. Each key with no Lamina setting, or a value
  Lamina cannot take (a pattern it lacks, a G-code with their placeholders), is reported as left out. The window's Import
  profile button lays the result over the settings shown.
- **5.5 Expressions in start G-code.** `{expr}` beside `[name]` (`crates/slicer/src/expr.rs`): numbers, the placeholders, `+ - * / %`,
  comparisons, `and`, `or`, `not`, parentheses, and `if`, `min`, `max`, `round`, `floor`, `ceil`, `abs`; `{{` writes a brace. A
  recursive-descent evaluator of our own, 500 characters and 32 levels at most, fuzzed. A whole number is written without a point.
- **6.1 Send to Moonraker** and **6.2 Send to OctoPrint.** `crates/printer`: one multipart upload to the address the user typed
  (`/server/files/upload` with `root` and `print`, or `/api/files/local` with `select` and `print` and the key in `X-Api-Key`),
  no redirects, the reply read up to 1 MB, the address checked (http or https, no credentials, query or fragment). The
  `send_to_printer` command reads a G-code file, so the assistant never runs it; `lamina-cli run` has it, the MCP server does not.
  The window's Printer connection keeps the kind and address, and the key only when Remember is ticked, sealed as the
  assistant's is; Send to printer and Send and print slice the plate and upload it. Tested against a fake printer on this machine.
  The second crate allowed on the network (`network_boundary` in `lamina-core`).
- **6.3 Printer status.** `lamina_printer::status`: Moonraker's printer objects, or OctoPrint's job and printer, read leniently
  (state, file, progress, nozzle and bed temperatures) with a five-second limit. Printer connection's Show what it is doing asks
  every five seconds on a thread. Not a command, so no agent can turn it on other addresses. Tested against the fake printer.
- **6.4 PrusaLink and Duet.** `crates/printer/src/prusalink.rs` (one `PUT /api/v1/files/usb/NAME` with `Print-After-Upload`, the key in
  `X-Api-Key`; `GET /api/v1/status`) and `duet.rs` (`rr_connect` with the board's password, then `rr_upload` into `0:/gcodes/` and
  `M32` through `rr_gcode`, with the session key; `rr_status`), from their published API descriptions. Both in `send_to_printer`,
  the window's Printer connection and its status line. Tested against the fake printer only.
- **6.4b PrusaLink with a user name and password.** `crates/printer/src/digest.rs`: HTTP digest from RFC 7616 and RFC 2617 (MD5 and
  SHA-256 and their `-sess` forms, `qop=auth` or RFC 2069's form without it, `userhash`), with MD5 and SHA-256 written from RFC 1321
  and FIPS 180-4. An unsigned status request fetches the printer's challenge and the upload carries the answer; the password never
  leaves the computer. `send_to_printer` takes `user`, the window a User field for PrusaLink. Tested against the RFCs' worked
  examples, the standards' hash examples, a fuzzed challenge parser, and a fake printer that issues a challenge and checks the answer.
- **6.4b A Duet run by a single-board computer.** `crates/printer/src/dsf.rs`, from the OpenAPI description of DSF's web server:
  `GET /machine/connect` with the password for a session key in `X-Session-Key`, `PUT /machine/file/0:/gcodes/NAME`,
  `POST /machine/code` with `M32` to start it, and `GET /machine/model` for the status. A Duet is asked DSF's connect first; a board on its
  own answers it with RepRapFirmware's `err`, a page or 404, and gets its `rr_` requests as before. Tested against fakes of both.
- **6.4b Printing over a USB cable.** `crates/printer/src/serial.rs`, from Marlin's G-code documentation and RepRap's checksum rule:
  `M110 N0`, then each line as `N<n> <line>*<xor>`, `ok` counted, `Resend: N` and `rs N` answered from line N (the refused lines
  sent before it, and their `ok`s, not counted twice), `busy:` waited through, at most four lines in flight, a silence taken as one
  lost `ok` and then given up on, a restart at the port's opening started over, a firmware error, a restart part way or the same
  line asked for again and again stopping it, and Cancel stopping it; each stop turns the heaters off. Written over `Read + Write`
  and tested against a Marlin in memory and a fuzzed one; the port is the `serial2` crate (BSD-2-Clause or Apache-2.0).
  `send_to_printer` takes `printer: "serial"` and `baud`; the window a USB cable with Port and Baud, and the progress as status.
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
- **7.4 GPU viewport.** `apps/lamina/src/gpu.rs` and `gpu.wgsl`, ported from BIMCraft's: the 3D view drawn with wgpu into a
  texture egui shows, the scene uploaded only when its `Arc` changes, the camera, selection and drag preview as uniforms and a small
  buffer of objects, edges in a second pass over normal, depth and id targets, and ids and depth read back only for a click. The
  processor stays the fallback (no adapter, OpenGL, a device error, a test harness, or View, Draw the 3D view on the processor, kept
  in the settings file); a failure is said on the status bar. Ignored tests compare the two pictures, drive the window on the card and time a 512,000-triangle plate.
- **7.5 Comfort.** Shortcuts: G, L and M pick the move, lay flat and measure tools (rotating is the gizmo's rings, in the move tool),
  R turns the picked objects 90 degrees, F frames them, A arranges, Ctrl+D duplicates, Ctrl+A picks all. A right-click menu on
  objects in the 3D view and the plate list. Edit, History lists the undo and redo steps (`Session::history`). Every setting has a
  tooltip from `profile::ABOUT` (a sentence for each field, checked by a test) and its range. A light theme, or the system's, in the
  View menu, kept in the settings file. Vase mode is in the window now.
- **7.6 Translations.** `crates/i18n` (`lamina-i18n`) and `apps/lamina/src/lang.rs`, lifted from BIMCraft's: the window in English,
  Brazilian Portuguese or Spanish, chosen in View, Language and kept in the settings file; the first start takes the system's
  language, else English. Every interface string goes through `t`, `tf` or `tc`, the slicer's setting and preset help is marked with
  `ui_msg!`, and `translations/ui/*.po` are compiled in by the app's build script; a missing or unfinished translation shows the
  English, and a finished one that drops a `{}` stops the build. `cargo i18n check` runs in CI. What stays English, because agents
  read it or it is not the interface's: command parameters and errors (the status bar shows an error from a command as the command
  gave it), the CLI, the MCP server, the assistant's skills, prompt and notes, the printer's own status line, preset and firmware
  names, G-code, and the log's own lines. Object, file and preset names the user gave are never translated. How to add or change a
  language: [TRANSLATING.md](TRANSLATING.md). Since: sections keep their English as their id (`lang::section`), so changing the
  language leaves them open or closed; number fields and the numbers in labels have a decimal comma in Portuguese and Spanish
  (`lang::number`, `lang::mark`); `pictures_of_menus_and_dialogs` draws every menu, dialog and settings group in each language
  for a look, and a test keeps the 3D view at 90 % or more of its English width in each.
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
- **7.9 Updater.** `crates/update` and `apps/lamina/src/updates.rs`, lifted from BIMCraft's: at the start the window asks
  `api.github.com` for the latest release (not with File, Check for updates at start unticked, kept in the settings file, nor with
  `LAMINA_NO_UPDATE_CHECK` set), compares its tag by semver, and offers it in a banner. Update now downloads the MSI, the AppImage
  or the DMG for this install, by the names in `release.yml`, capped at its listed size and checked against GitHub's SHA-256
  digest; Restart and install asks about unsaved changes, then on Windows a copy of the program runs the MSI after the window
  exits, on Linux the AppImage is replaced, on macOS the DMG is mounted and the app bundle swapped. HTTPS only, redirects only to
  GitHub's release hosts. The third crate allowed on the network, and a dependency of the window alone (`network_boundary`).
  Tested on a fake network, a server on this machine and stand-in scripts for msiexec, hdiutil and the rest; not yet against a
  real release, as none has been tagged.
- **8.2 Benchmarks.** Ignored tests that print their times: `crates/slicer/tests/bench.rs` (about 100,000 triangles, three ways)
  and `crates/csg/tests/bench.rs` (a 5,120-triangle sphere with a 384-triangle cylinder, each boolean), plus
  `simplify::tests::heavy`. First run, on the session's machine in a dev build: slicing 6.1 s with the defaults, 9.4 s with gyroid
  and supports, 8.1 s with adaptive layers; difference 13.1 s, intersect 11.9 s, and the union refused after 16.8 s (the engine's
  glancing-curve limit, see Known limits).
- **8.3 User guide.** `docs/USER-GUIDE.md`: installing, the window, the first print, arranging and changing parts, presets of your
  own, the assistant and its key, sending to a printer, projects and autosave, where Lamina keeps things, and a table of every
  setting made from `profile::ABOUT`, `LIMITS` and the defaults (`crates/slicer/tests/guide.rs` checks it; `LAMINA_BLESS=1`
  writes it again). Its pictures are drawn by Lamina's CPU renderer (`docs_pictures`, ignored); the picture of the whole window
  in the README is drawn by `picture_of_the_window`, which a software Vulkan driver (lavapipe) can run.
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
- **9.2 More skills.** `troubleshooting-prints` (stringing, warping, layer shifts, sagging, gaps, blobs, the first layer),
  `calibration` (a cube, single-wall flow through `filament_diameter` as Lamina has no flow multiplier, temperature by separate
  prints as it has no temperature tower, retraction towers), `materials` (the five presets, nylon, PC, filled filaments, food and
  heat) and `supports` (when they are needed, turning and cutting first, every support setting). The skill test now takes the
  tools from `tools::all()`, so a skill may name `preview`.
- **9.3 Local models.** `Provider::Local` (`crates/assistant/src/local.rs`): an OpenAI-compatible server on this computer
  (Ollama, LM Studio, llama.cpp), spoken to in Chat Completions, with tools and pictures. `local::endpoint` refuses any host but
  this computer and writes `localhost` as 127.0.0.1; `net::allowed` lets plain HTTP through to 127.0.0.1 and ::1 only, by address,
  with tests that every other host is refused. No key is needed. The window shows the server's address and a free model field,
  kept in the settings file. A whole turn is tested against a server on this machine.
- **9.4 Agents on the open window.** BIMCraft's control port, lifted: the protocol, the token and the bridge file in
  `lamina_core::bridge`; the listener in `apps/lamina/src/control.rs` (127.0.0.1 only, a 128-bit token in its first line or the
  connection is dropped, capped lines, eight agents, answered in the frame as a button's command); `lamina-cli mcp --connect
  [PORT]` in `apps/lamina-cli/src/window.rs`. The Agents section of the plate panel (or `lamina --control PORT`) opens it, kept in
  the settings file, with read-only as an option. A window's agent never replaces the plate or sends it to a printer. Tested with
  a real port: an agent adds a box that becomes an undo step in the window.
- **9.5 Saved chats.** BIMCraft's `saved.rs`, lifted into `crates/assistant`: each project's chat (the log, the conversation and the
  checklist, never the key or a picture) in `chats/` beside the settings, named for a hash of the project's path. The window
  writes it when a turn ends and brings it back when the project opens; a new plate starts a new chat. A file is read whole or
  not at all, within a live chat's caps.
- **9.6 Streaming replies.** Every provider is asked for server-sent events (Anthropic and OpenAI `stream: true`, Gemini
  `streamGenerateContent?alt=sse`, a local server `stream: true`), each format as its provider's docs give it. `Net::stream` hands
  the body over as it arrives; `receive` reads it event by event, `MAX_REPLY` over the whole stream, and each provider's `Stream`
  gives the text as it comes and puts the whole turn back together in the provider's own form, so tool calls, thinking, counts and
  `Reply::raw` are as before. The answer grows in one line of the panel; the tools run once the reply has ended. Stop closes the
  stream at its next event, and a stream silent for two minutes, or running past ten, is given up. A local server that answers
  whole is still read. Tested on recorded streams (text, one and two calls, cut off, malformed, refusals and error events) and in
  the window with a fake network; not yet tried against the real APIs.

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

## 2. Meshes

**2.5 Booleans of earlier results (P1).** `chain_score` passes 73 of 120 chains of 4 to 6 booleans; what fails is refused, never
wrong. How: after the repair, and before `sound`, clean what the repair leaves: merge each run of coplanar triangles (a fan, the
slivers along a cut) into its outline and triangulate it again with `i_triangle`, so a flat face is one plane with no needles; drop
the zero-area triangles that remain and weld their corners. Keep a result's flat faces flat by snapping the corners of each merged
region onto its plane. The `sound` gate stays as it is: a result it refuses is still refused. Measure with `chain_score` before and
after, and keep a test that a chain which passed still passes with the same volume.

## 3. Modelling

## 4. Slicer

**4.22 The wait for a tool to heat in the time estimate (P2).** A tool change waits (`M109`, `M116`) for the new tool to come up
from its standby temperature, which the estimate leaves out. How: a printer setting `heating_rate` (degrees per second, 2 by
default, about what a 40 W heater does near printing temperature); each change adds the rise from where the tool was left (its
standby, or off) to its temperature over the rate, as time the planner waits with the head at rest. Test: two extruders with a
standby 30 degrees below add 15 s a change at the default rate.

**4.23 Walls of varying width (P2).** A thin feature gets walls of a fixed width and a gap-fill line, so a 0.6 mm fin prints as one
line of a third more plastic and a 1 mm wall as two walls and a sliver. How, in the spirit of what PrusaSlicer and Cura document
for Arachne (written from the documentation and the geometry, never their code): `wall_generator`, `"classic"` (today's, the
default) or `"variable"`. With `"variable"`, where a region is narrower than its walls need, its medial axis (from a fine sampling
of the outline: each sample's inward normal walked to the nearest point of the opposite side) gives a centre line and a width at
each point; the walls take the number of lines that width holds, from one to `perimeters`, each as wide as the width divided by
the count (within 0.5 and 1.5 line widths), and the flow follows the width along the line. Thicker regions keep their classic
walls. Test: a wedge from 0.3 to 2 mm prints one line widening, then two, with the plastic of the wedge's area within 10 %; a
0.6 mm fin one line at 0.6 mm; the golden files unchanged with `"classic"`.

## 5. Profiles and settings

## 6. Printers

**6.4b Bambu over the LAN (P2).** Left out for now, and why, in [QUESTIONS.md](QUESTIONS.md): MQTT over TLS and implicit FTPS,
which Bambu's 2025 authorization keeps to Bambu Connect or a printer in Developer Mode. Its own module in `crates/printer` if done.

## 7. Desktop

## 8. Quality and infrastructure

## 9. The assistant

## Known limits today

- Walls are constant width; a thin feature gets one gap-fill line, not walls of varying width.
- The time estimate has not been checked against a real print.
- A mesh whose holes are too big to fill without guessing stays open, and prints with parts missing, with a warning. STL, 3MF, OBJ and glTF in; STL and 3MF out.
- Booleans refuse about 1 in 100 random pairs (the rest of a sphere-with-sphere union mostly). Booleans of earlier results are
  refused more often: 9 steps in 10 come through in `chain_score` (73 of 120 chains of 4 to 6 booleans clean), and what fails is
  refused, never wrong. A boolean's result keeps the repair's slivers and fans, and works on its world mesh, so its tilted faces are
  flat only to f32.
- Tool changes are for a nozzle per extruder: a single nozzle fed several filaments (a multi-material unit, with its ramming and
  long purges) is not handled. The time estimate leaves out the wait for a tool to heat. None of it has run on a printer with two
  extruders.
- Printing one object at a time takes the head as a cylinder of `clearance_radius` and the gantry as a bar across the whole
  plate, and each object as its convex outline from above, so it may refuse an arrangement a real head would clear (a notch the
  head could reach into). It takes one extruder.
- Nothing has been printed on real hardware.
