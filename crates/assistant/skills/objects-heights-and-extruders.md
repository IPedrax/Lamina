---
name: objects-heights-and-extruders
description: Settings for one object, settings that change with height, several extruders and printing one object at a time. Read before any of these.
---
# Objects, heights and extruders

## One object unlike the others

`set_object_settings {id, profile: {infill_density: 0.5, perimeters: 4}}` gives one object settings of its own (walls, top and
bottom layers, infill, seam, speeds, supports, its extruder); the printer, the filament and the layer heights stay the plate's.
`profile: {}` clears them; `inspect_scene` shows them. Settings by height: `set_height_range {id, from: 4, to: 6, profile:
{nozzle_temp: 230}}` gives the layers whose middle is between 4 and 6 mm their own walls, infill, speeds, seam, nozzle_temp,
fan_speed, pressure_advance, retraction or flow_ratio (a temperature tower, denser infill where a load goes).

## Two colours or materials

A part in two colours is two objects placed together, each with its `extruder` (from 1) on a printer with `extruders` (up to 8):
`set_object_settings {id, profile: {extruder: 2}}`, then `slice_check {profile: {extruders: 2}}`. Each layer changes tool once
per extruder. The idle tool cools by `standby_temp_delta` (30), and `print_time` counts the wait to heat it again at
`heating_rate` (2 C a second, a printer field); `extruder_temps` gives the extruders after the first their own
temperature (`[235]`). Each change purges `purge_volume` mm3 (20) on a tower `tower_size` mm wide (20) at `tower_x`, `tower_y`
(60, 60 from the plate's middle); a tower off the plate, in an object's or a support's way on a layer it rises through, or too
small is refused (a notch, or the room under a part above the last tool change, will do). `features` counts `purge` and
`purge-tower`. `toolchange_gcode` runs before each `T`, with `[previous_tool]`, `[next_tool]` (from 0) and `[layer_z]`. Nozzle
offsets are the firmware's.

## One object at a time

`sequential: true` prints each object whole, shortest first, so a failure spoils one. Objects must stand `clearance_radius` (35
mm, the head's reach round the nozzle) apart, measured between their convex outlines seen from above, and only the last may pass `gantry_height` (20 mm): ask the user for the real
figures. All on one extruder. An error names the objects; `arrange {gap: 35}` spreads them.
