---
name: troubleshooting-prints
description: What causes common print faults (stringing, warping, layer shifts, sagging, gaps, blobs) and which settings or model changes help. Read when the user describes a print that went wrong.
---
# Troubleshooting prints

Ask what the fault looks like and where on the part it is, and which printer, filament and settings were used, before you change
anything. Change one thing at a time, so the next print shows what helped. Say which causes are in the printer (belts, nozzle, a
wet spool) and not in the settings: no setting fixes those. Nothing in Lamina has been tested on a real printer yet, so offer
these as things to try, not promises.

## Stringing (thin hairs between parts)

The nozzle oozes while it travels.

- Retract more: `retract_length` up by 0.5 mm at a time (direct drive 0.5 to 2 mm, Bowden 3 to 6 mm), `retract_speed` 35 to 45.
- Cooler nozzle: `nozzle_temp` down by 5 C, staying in the filament's range.
- Keep `combing` on (travel inside the part needs no retraction and crosses no gap), and try `wipe_distance` 1 to 2 mm.
- PETG strings more than PLA whatever the settings; a wet spool strings badly and pops (dry it).

Check with `slice_check {profile: {retract_length: 1.5, wipe_distance: 1}}`.

## Warping (corners lift off the plate)

The part shrinks as it cools and pulls its corners up.

- A brim holds the edges: `brim_width` 5 to 8. For a very small footprint or a bed that is not flat, `raft_layers` 2 or 3.
- Plate temperature in the filament's range (`bed_temp`, `first_layer_bed_temp`), a clean plate, a slower first layer
  (`first_layer_speed` 15 to 20).
- ABS and ASA warp unless enclosed; say so, and use `fan_speed` 0.
- In the model, round sharp corners: a `rounded_box` with `corner` 3 to 5 lifts less than a box.

## Layer shifts (the layers above a height are offset)

Mechanical: a loose belt, a pulley grub screw, the nozzle hitting a curled edge, or moves too fast for the motors.

- Lower `travel_speed` (120) and `infill_speed`, and try `z_hop` 0.2 to 0.4 so the nozzle clears curled edges.
- Tell the user to check the belts and pulleys: Lamina cannot fix a loose belt.

## Sagging overhangs and droopy bridges

- An overhang past 45 degrees needs support or a better orientation (read the supports skill).
- More cooling on small, tall parts: `min_layer_time` 10 to 15, so each layer cools before the next.
- A bridge longer than about 10 mm sags: support it, or split the part with `cut` so the span prints flat.

## Gaps, holes and weak parts

- Holes in the top surface (pillowing): more `top_layers` (5 to 7, or about 1 mm of skin).
- Gaps between walls and infill, or thin layers that do not bond: the nozzle may be too cold or the filament wet; the flow may be
  low (read the calibration skill).
- Walls that come apart: more `perimeters` (3 to 4); infill adds less strength than walls.
- A part that is thinner than two lines prints as one line along its middle (gap fill): thicken it in the model where it must be strong.

## Blobs, zits and a visible seam

- A line of marks up the part is the seam: `seam: "rear"` hides it at the back, `"sharpest"` in an inside corner.
- Scattered blobs: `wipe_distance` 1 to 2, a cooler nozzle, and `seam: "aligned"` instead of `"nearest"`.

## First layer

- Does not stick: slower and hotter first layer (`first_layer_speed` 15, `first_layer_nozzle_temp` 5 C up), a brim, a clean plate,
  and the printer's own levelling. A thicker `first_layer_height` (0.25 to 0.3) forgives an uneven plate.
- Elephant's foot (the bottom flares out): a little lower `first_layer_bed_temp`; in the model, a 0.5 mm chamfer round the bottom edge.

## When to look at the model, not the settings

If `inspect_scene` shows an object that is not watertight, or a slice warns that a mesh is not closed, parts of it are missing from
the print: `repair {id}` first, and say what it could not mend.
