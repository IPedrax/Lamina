---
name: slicing-with-lamina
description: The slicer's settings, sensible values for them, and how to check a slice. Read before changing a setting or answering a question about print time, strength or quality.
---
# Slicing with Lamina

You check a slice. The user exports the G-code from the app. `slice` writes a file, so you cannot run it. `slice_check
{ids?, profile?}` runs the same slice and writes nothing. It returns layers, filament length, time and warnings, or an error that
says what is wrong.

## Settings

`slicer_presets` lists the built-in printers and filaments. Name them instead of setting temperatures by hand:
`slice_check {printer: "Generic 300 mm, Klipper", filament: "PETG"}`. A printer preset sets the plate, nozzle, start and end G-code
and retraction; a filament preset sets the temperatures and the fan; `profile` fields given as well win over both. The user's own
presets live in the app's settings panel, not in these commands.

`slicer_profile` prints every setting with its default. Pass only the fields that differ, for example
`slice_check {profile: {layer_height: 0.16, infill_density: 0.3}}`. A misspelled field is an error, and so is a value out of range.

- Printer: `bed` (x and y, mm), `max_height`, `nozzle`, `filament_diameter`, `origin_center` (true for delta printers),
  `start_gcode` and `end_gcode` (placeholders like `[bed_temp]`; unknown ones are refused).
- Quality: `layer_height` from 0.02 mm up to 0.8 times the nozzle (0.12 for fine detail, 0.2 normal, 0.28 draft),
  `first_layer_height`, `perimeters` (2 normal, 3 to 4 for strength), `top_layers` and `bottom_layers` (enough for 0.8 to 1.0 mm of skin).
- Strength: `infill_density` from 0 to 1. Use 0.1 to 0.2 for looks and 0.3 to 0.5 for parts that bear load. Walls add more
  strength than infill does.
- Speeds, mm/s: `perimeter_speed` 40 to 60, `infill_speed` 60 to 100, `first_layer_speed` 15 to 25, `travel_speed` 120 to 200.
- Temperatures are typical ranges and the filament's label wins. PLA: nozzle 190 to 220, bed 50 to 65. PETG: nozzle 230 to 250,
  bed 70 to 85. ABS: nozzle 230 to 260, bed 90 to 110.
- Cooling and retraction: `fan_speed` 0 to 255 (PLA full, PETG about half, ABS off). `retract_length` about 0.5 to 2 mm on a
  direct-drive extruder and 3 to 6 mm on a Bowden tube, `retract_speed` 25 to 45.
- The skirt (`skirt_loops`, `skirt_distance`) primes the nozzle. There is no brim yet.

## Reading the result

- `print_time` and `filament_mm` are lower bounds. Time is distance over speed with no acceleration, so expect longer. A metre of
  1.75 mm filament is about 2.4 cm3, roughly 3 g of PLA.
- A warning that a mesh is not closed means parts of it may be missing from the print. Tell the user, and do not call the slice good.
- An error that an object does not fit, is below the plate or is too tall names the object. Fix the scene (`scale`, `move`,
  `drop_to_bed`) and check again.

## What is not there yet

Supports, bridging, brim, raft, seam control, gap fill and multi-material. Say so when a model needs one, instead of letting the
user find out at the printer. Nothing has been verified on a real printer, so never promise that a print will succeed.
