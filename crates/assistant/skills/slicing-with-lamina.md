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
  `start_gcode` and `end_gcode` (placeholders like `[bed_temp]`; unknown ones are refused), `firmware` (`"marlin"`, `"klipper"`:
  objects marked so one can be cancelled, or `"reprap"`), `firmware_retraction` (G10 and G11), `thumbnail` (pixels; a picture for
  the printer's screen), `max_acceleration` and `junction_deviation` (for the time estimate). A filament may set
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

## One object unlike the others

`set_object_settings {id, profile: {infill_density: 0.5, perimeters: 4}}` gives one object settings of its own (walls, top and
bottom layers, infill, seam, speeds, supports); the printer, the filament and the layer heights stay the plate's. `profile: {}`
clears them; `inspect_scene` shows them.

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

Tree supports, walls of varying width and multi-material. Say so when a model needs one, instead of letting the
user find out at the printer. Nothing has been verified on a real printer, so never promise that a print will succeed.
