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
  `first_layer_height`, `perimeters` (2 normal, 3 to 4 for strength), `top_layers` and `bottom_layers` (enough for 0.8 to 1.0 mm of skin).
  `adaptive_layers: true` varies each layer from `min_layer_height` (0.08) up to `layer_height`: thin on domes and gentle slopes,
  thick on walls; good for figures and rounded parts, little use for boxy ones.
- Strength: `infill_density` from 0 to 1. Use 0.1 to 0.2 for looks and 0.3 to 0.5 for parts that bear load. Walls add more
  strength than infill does. `infill_pattern`: `"lines"` (the default, quickest), `"grid"`, `"triangles"` (stiff in the plane),
  `"gyroid"` (strong in every direction, the usual choice for functional parts) or `"honeycomb"`.
- Speeds, mm/s: `perimeter_speed` 40 to 60, `infill_speed` 60 to 100, `first_layer_speed` 15 to 25, `travel_speed` 120 to 200.
- Temperatures are typical ranges and the filament's label wins. PLA: nozzle 190 to 220, bed 50 to 65. PETG: nozzle 230 to 250,
  bed 70 to 85. ABS: nozzle 230 to 260, bed 90 to 110.
- Cooling and retraction: `fan_speed` 0 to 255 (PLA full, PETG about half, ABS off), off for the first layer and up in steps to
  full by `fan_full_layer` (3). `min_layer_time` (5 s) slows a small layer so it cools before the next, never below
  `min_print_speed` (10 mm/s): raise it to 10 to 15 for small, tall parts that droop. `retract_length` about 0.5 to 2 mm on a
  direct-drive extruder and 3 to 6 mm on a Bowden tube, `retract_speed` 25 to 45. `combing` (on) travels inside the part without
  retracting; `z_hop` (0; 0.2 to 0.4 mm lifts the nozzle over what is printed, against scrapes and knocked-over parts) and
  `wipe_distance` (0; 1 to 2 mm wipes the nozzle along the last line while retracting, against blobs and strings).
- The skirt (`skirt_loops`, `skirt_distance`) primes the nozzle. `brim_width` (mm, 0 by default; 3 to 8 for parts with a small
  footprint or sharp corners that may lift, and for ABS) adds loops round the first layer, touching it. `raft_layers` (0 by default,
  2 or 3 when used) prints coarse layers under the part and lifts it onto them: for a warped bed or a tiny contact area; it costs
  time and a rougher underside.
- Vase mode: `vase: true` prints the bottom layers, then one outer wall that climbs without stopping, with no infill and no top:
  for vases, cups and lamp shades, one object at a time. The wall is one line thick, so it is fragile.
- Surfaces: `ironing: true` passes over each top surface again with close lines (`ironing_spacing` 0.1 mm) and a trickle of
  plastic (`ironing_flow` 0.1) at `ironing_speed` (15): smooth tops for lids and nameplates, at the cost of time. `fuzzy_skin:
  true` roughens the outer wall (`fuzzy_thickness` 0.3, `fuzzy_spacing` 0.8 mm) for a grip that hides layer lines. Both may be set
  for one object alone.
- Seams: `seam` is where each wall loop starts and ends, which leaves a small mark: `"aligned"` (the default, one tidy line up the
  part), `"rear"` (at the back, out of sight), `"sharpest"` (hidden in an inside corner) or `"nearest"` (least travel, marks
  scattered).
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

## One object unlike the others

`set_object_settings {id, profile: {infill_density: 0.5, perimeters: 4}}` gives one object settings of its own (walls, top and
bottom layers, infill, seam, speeds, supports, its extruder); the printer, the filament and the layer heights stay the plate's.
`profile: {}` clears them; `inspect_scene` shows them.

## Two colours or materials

A part in two colours is two objects placed together, each with its `extruder` (from 1) on a printer with `extruders` (up to 8):
`set_object_settings {id, profile: {extruder: 2}}`, then `slice_check {profile: {extruders: 2}}`. Each layer changes tool once
per extruder. The idle tool cools by `standby_temp_delta` (30); `extruder_temps` gives the extruders after the first their own
temperature (`[235]`). Each change purges `purge_volume` mm3 (20) on a tower `tower_size` mm wide (20) at `tower_x`, `tower_y`
(60, 60 from the plate's middle); a tower off the plate, over an object or too small is refused. `features` counts `purge` and
`purge-tower`. `toolchange_gcode` runs before each `T`, with `[previous_tool]`, `[next_tool]` (from 0) and `[layer_z]`. Nozzle
offsets are the firmware's.

## One object at a time

`sequential: true` prints each object whole, shortest first, so a failure spoils one. Objects must stand `clearance_radius` (35
mm, the head's reach round the nozzle) apart, and only the last may pass `gantry_height` (20 mm): ask the user for the real
figures. All on one extruder. An error names the objects; `arrange {gap: 35}` spreads them.

## Reading the result

- `features` splits the filament by kind (walls, infill, support, brim, raft): if support is a large share, the part may print
  better turned another way.
- `print_time` plans each move with the printer's `max_acceleration` and `junction_deviation` (set by the printer preset), so it is
  close for a printer set like that; it has not been checked against a real print yet. `filament_mm` is what the G-code pushes. A metre of
  1.75 mm filament is about 2.4 cm3, roughly 3 g of PLA.
- A warning that a mesh is not closed means parts of it may be missing from the print. Tell the user, and do not call the slice good.
- An error that an object does not fit, is below the plate or is too tall names the object. Fix the scene (`scale`, `move`,
  `drop_to_bed`) and check again.

## What is not there yet

Walls of varying width, and a single nozzle fed several filaments (a multi-material unit): Lamina's tool changes are for printers
with a nozzle for each extruder. Say so when a model needs one, instead of letting the user find out at the printer. Nothing has been verified on a real printer, so never promise that a print will succeed.
