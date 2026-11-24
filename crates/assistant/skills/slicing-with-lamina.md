---
name: slicing-with-lamina
description: The slicer's settings, sensible values for them, and how to check a slice. Read before changing a setting or answering a question about print time, strength or quality.
---
# Slicing with Lamina

You check a slice. The user exports the G-code from the app. `slice` writes a file, so you cannot run it. `slice_check
{ids?, profile?}` runs the same slice and writes nothing. It returns layers, filament length, time and warnings, or an error that
says what is wrong.

## Settings

`slicer_presets` lists the built-in printers, filaments and processes. Name them instead of setting fields by hand:
`slice_check {printer: "Generic 300 mm, Klipper", filament: "PETG", process: "Strong"}`. A printer preset sets the plate, nozzle,
start and end G-code and retraction; a filament preset sets the temperatures and the fan; a process sets the layers and the strength
("0.12 mm Fine", "0.20 mm Standard", "0.28 mm Draft", "Strong"); `profile` fields given as well win over all three. The user's own
presets live in the app's settings panel, not in these commands.

`slicer_profile` prints every setting with its default. Pass only the fields that differ, for example
`slice_check {profile: {layer_height: 0.16, infill_density: 0.3}}`. A misspelled field is an error, and so is a value out of range.

- Printer: `bed` (x and y, mm), `max_height`, `nozzle`, `filament_diameter`, `origin_center` (true for delta printers),
  `start_gcode` and `end_gcode` (placeholders like `[bed_temp]`, unknown ones refused, and expressions in braces such as
  `{first_layer_bed_temp - 5}` or `{if(bed_temp > 90, 1, 0)}`), `firmware` (`"marlin"`, `"klipper"`:
  objects marked so one can be cancelled, or `"reprap"`), `firmware_retraction` (G10 and G11), `thumbnail` (pixels; a picture for
  the printer's screen), `max_acceleration` and `junction_deviation` (for the time estimate), `arc_fitting` (curves as `G2`/`G3`
  arcs within `arc_tolerance`; smaller files and smoother circles, but Klipper needs `[gcode_arcs]` in its config). A filament may set
  `pressure_advance`.
- Quality: `layer_height` from 0.02 mm up to 0.8 times the nozzle (0.12 for fine detail, 0.2 normal, 0.28 draft),
  `first_layer_height`, `perimeters` (2 normal, 3 to 4 for strength), `wall_generator` (`"classic"`, the default, or
  `"variable"`: a part too thin for its walls, a fin, a thin rib or a tube wall, gets as many lines as its width holds, each a share
  of it, instead of loops that fold onto themselves and gap fill; try it for thin-walled parts), `top_layers` and `bottom_layers` (enough for 0.8 to 1.0 mm of skin).
  `adaptive_layers: true` varies each layer from `min_layer_height` (0.08) up to `layer_height`: thin on domes and gentle slopes,
  thick on walls; good for figures and rounded parts, little use for boxy ones. By hand, for the whole plate:
  `layer_heights {action: "set", from, to, height}` (fine layers only where a detail is), `"adaptive"` to start from adaptive
  layers, `"smooth"`, `"clear"`; these take the place of `layer_height` and `adaptive_layers`.
- Strength: `infill_density` from 0 to 1. Use 0.1 to 0.2 for looks and 0.3 to 0.5 for parts that bear load. Walls add more
  strength than infill does. `infill_pattern`: `"lines"` (the default, quickest), `"grid"`, `"triangles"` (stiff in the plane),
  `"gyroid"` (strong in every direction, the usual choice for functional parts) or `"honeycomb"`.
- Speeds, mm/s: `perimeter_speed` 40 to 60, `infill_speed` 60 to 100, `first_layer_speed` 15 to 25, `travel_speed` 120 to 200.
  Filament fields: `max_volumetric_speed` (mm3/s, 0 for none; 12 to 15 for PLA on a common hot end) slows a line that needs more
  plastic than that; `flow_ratio` (1.0) scales all the plastic, set from the single-wall test.
- Temperatures (the label wins): PLA nozzle 190 to 220, bed 50 to 65; PETG 230 to 250, bed 70 to 85; ABS 230 to 260, bed 90 to 110.
- Cooling and retraction: `fan_speed` 0 to 255 (PLA full, PETG about half, ABS off), off for the first layer and up in steps to
  full by `fan_full_layer` (3). `min_layer_time` (5 s) slows a small layer so it cools before the next, never below
  `min_print_speed` (10 mm/s): raise it to 10 to 15 for small, tall parts that droop. `retract_length` 0.5 to 2 mm direct drive, 3 to 6
  Bowden; `retract_speed` 25 to 45. `combing` (on) travels inside the part without
  retracting; `z_hop` (0; 0.2 to 0.4 mm lifts the nozzle over what is printed, against scrapes and knocked-over parts) and
  `wipe_distance` (0; 1 to 2 mm wipes the nozzle along the last line while retracting, against blobs and strings).
- The skirt (`skirt_loops`, `skirt_distance`) primes the nozzle. `brim_width` (mm, 0; 3 to 8 for a small footprint, sharp
  corners that lift, or ABS) adds loops touching the first layer. `raft_layers` (0; 2 or 3 when used) lifts the part onto coarse
  layers, for a warped bed or a tiny contact area, at the cost of time and a rougher underside.
- Vase mode: `vase: true` prints the bottom layers, then one outer wall climbing without a stop, no infill or top: vases, cups,
  lamp shades, one object at a time; one line thick, so fragile.
- Surfaces: `ironing: true` passes over each top surface again with close lines (`ironing_spacing` 0.1 mm) and a trickle of
  plastic (`ironing_flow` 0.1) at `ironing_speed` (15): smooth tops for lids and nameplates, at the cost of time. `fuzzy_skin:
  true` roughens the outer wall (`fuzzy_thickness` 0.3, `fuzzy_spacing` 0.8 mm) for a grip that hides layer lines. Both may be set
  for one object alone.
- Seams: `seam` is where each wall loop starts and ends, which leaves a small mark: `"aligned"` (the default, one tidy line up the
  part), `"rear"` (at the back, out of sight), `"sharpest"` (hidden in an inside corner) or `"nearest"` (least travel, marks
  scattered). To put it in one place, `paint {id, kind: "seam", at: [x, y, z]}` at points up the side: each wall starts there.
- Bridges: solid infill over air (a span between two supports) is printed straight across, at the angle whose lines rest on the
  layer below at both ends, with `bridge_speed` (25 mm/s), `bridge_flow` (1.0 of the normal flow) and `bridge_fan` (255). A span
  up to about 10 mm bridges; longer ones need support.
- Supports: `support` is `"off"` (the default), `"everywhere"` or `"from_plate"` (only where the column below reaches the plate,
  so nothing stands on the model). `support_angle` (degrees from vertical, 45 by default; a surface leaning further is held up),
  `support_density` (0.1 to 0.2), `support_z_gap` (one layer, 0.2 mm, so it breaks away), `support_xy_gap` (0.6 mm from the walls)
  and `support_interface_layers` (2 denser layers under the model). Turn them on for a model with overhangs past 45 degrees or
  flat parts over air: `slice_check {profile: {support: "everywhere"}}`. Better still, turn the part so it needs none.
  `support_style` is `"columns"` (the default: lines under the whole overhang) or `"tree"`: branches from small tips
  `tree_tip_spacing` apart (3 mm) that lean up to `tree_branch_angle` (40 degrees) to gather into trunks up to `tree_max_radius`
  (3 mm), go round the part and touch it only through the interface. Trees use less plastic and leave smaller marks under tall
  overhangs far from the plate, such as a figure's arms or a ball on a stalk; under a low, wide overhang the two cost about the same.
  Compare them: `slice_check {profile: {support: "everywhere", support_style: "tree"}}` and read `features`.

## Several objects, heights and extruders

Settings for one object, settings by height, two colours and printing one object at a time are in the skill
`objects-heights-and-extruders`.

## Reading the result

- `features` splits the filament by kind (walls, infill, support, brim, raft): if support is a large share, the part may print
  better turned another way.
- `print_time` plans each move with the printer's `max_acceleration` and `junction_deviation`; not yet checked against a real
  print. `filament_mm` is what the G-code pushes: a metre of 1.75 mm filament is about 2.4 cm3, roughly 3 g of PLA.
- A warning that a mesh is not closed means parts of it may be missing from the print. Tell the user, and do not call the slice good.
- An error that an object does not fit, is below the plate or is too tall names the object. Fix the scene (`scale`, `move`,
  `drop_to_bed`) and check again.

## What is not there yet

A single nozzle fed several filaments (a multi-material unit): Lamina's tool changes need a nozzle per extruder. Say so when a
model needs one. Nothing has been verified on a real printer, so never promise that a print will succeed.
