---
name: modelling-with-lamina
description: How to build and check a model on the plate with Lamina's commands, what they cannot do yet, and how to report. Read before any modelling request.
---
# Modelling with Lamina

## The frame

Millimetres. x east, y north, z up. The origin is the middle of the plate and z = 0 is the plate's surface. A new object is
centred on x and y with its lowest point on the plate, so everything you add lands in the same spot: spread objects out at once
with `move {id, by: [x, y, 0]}`.

## What you can do today

- Shapes: `add_primitive {kind: "box", size: [x, y, z]}`, `{kind: "cylinder", radius, height, segments?}` and
  `{kind: "sphere", radius, segments?}`. The default 48 segments is fine for most parts; raise `segments` (up to 256) when a round
  part must look round.
- Place and reshape: `move`, `rotate` (about the object's centre, then it is set back on the plate), `scale` (positive factors,
  one per axis with `by`, so a sphere can become an ellipsoid and a cylinder a disc), `drop_to_bed`, `delete`, `undo`, `redo`.
- Opening and saving files is the user's job in the app. You cannot read or write files.

## What you cannot do yet

There are no booleans, so no holes through a solid, no unions, no fillets, no text and no threads. Each object is its own solid
and is sliced alone, so two objects that overlap are printed twice where they overlap. Never build one part from overlapping
pieces. When a request needs something missing, say so plainly, offer the closest thing that works (separate pieces the user
joins with glue or screws, a plain solid in place of a hollow one) and let the user choose.

## Work in this order

1. Look first: `inspect_scene` before you change anything.
2. If a missing number would make the part meaningless (the diameter of the rod a holder must grip), ask one question. Otherwise
   do not interview the user: choose defensible values, keep a list of every value you chose, and hand the list back.
3. Make it, one change at a time.
4. Check with numbers before you describe it. `inspect_scene` gives size, min, max, volume and whether the mesh is closed. The
   size must match what was asked, `watertight` must be true, and the object must sit on the plate (min z = 0).
5. `slice_check` says whether it fits the plate and how long it will take. A model that fails it is not done.
6. Report in this order: what is on the plate; the values you chose, one per line, so a wrong one is easy to spot; anything the
   request needed that Lamina cannot do yet. Then stop and let the user correct you.

## Mistakes to avoid

- Stacking objects to make a taller one. They overlap or float. A taller single shape is one `scale` away.
- Moving something below z = 0. `drop_to_bed` puts it back.
- Calling a result ready to print. Nothing here has been tested on a printer. Say what was checked (closed mesh, fits the plate,
  estimated time) and what was not.
