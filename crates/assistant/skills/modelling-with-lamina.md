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
  `{kind: "sphere", radius, segments?}`. Also `rounded_box` (`size` and `corner`, the edges' radius), `cone` (`radius`, `height`,
  `radius_top` for a frustum), `prism` and `pyramid` (`radius` to a corner, `height`, `sides`), `tube` (`radius`, `inner`,
  `height`: no boolean needed) and `torus` (`radius` to the ring's middle, `tube`). The default 48 segments is fine for most parts;
  raise `segments` (up to 256) when a round part must look round.
- Flat parts: `extrude {shape: "rounded_rect", size: [60, 40], corner: 5, height: 4, round_holes: [[-22, -12, 4], [22, -12, 4]]}`
  lifts an outline into a solid (also `shape: "slot"` or `"circle"`, or your own `outline: [[x, y], ...]` with `holes`). Brackets,
  plates with mounting holes and spacers are one extrude each: prefer it to a box with cylinders subtracted.
- Place and reshape: `move`, `rotate` (about the object's centre, then it is set back on the plate), `scale` (positive factors,
  one per axis with `by`, so a sphere can become an ellipsoid and a cylinder a disc), `mirror {id, axis}` (a left part from a right
  one), `drop_to_bed`, `delete`, `undo`, `redo`.
- `lay_flat {id, face}` turns an object so one of its triangles lies on the plate (the user usually clicks the face in the window);
  `measure {a, b}` gives the distance between two points.
- Several parts: `duplicate {id, count?}` makes copies and lays the plate out; `arrange {}` lays every object out in rows, none
  touching, and says whether they fit the plate.
- Opening and saving files is the user's job in the app. You cannot read or write files.

## Joining and cutting

`union {ids: [a, b]}`, `difference {id, minus: [b]}` and `intersect {ids: [a, b]}` replace their objects with one new solid where
the result stands, as one undo step. The result keeps the first object's name unless you pass `name`.

- Make a hole or a pocket with `difference`: build the cutting shape (a cylinder or box) and let it stick out past the faces it
  passes through by a millimetre or more, so a through hole's cylinder is taller than the part.
- Build one part out of several shapes with `union`. Objects that merely overlap are not joined: they print twice where they overlap.
- Booleans are reliable on boxes, cylinders and spheres, upright or turned. A boolean that cannot give a clean solid says so and
  changes nothing: move or resize a shape by a hundredth of a millimetre and try again. Do turns before booleans where you can: the
  first boolean on an object sees its faces exactly flat, a later one only to the precision stored.
- `cut {id, z}` splits an object at a height into two closed parts, the upper one turned onto its flat cut face beside the lower:
  for a part too tall for the printer, or one that prints better in two halves.
- Booleans take longer on shapes with many segments. Use 24 to 32 segments for a cutting tool.
- Still missing: fillets and chamfers (a rounded box and a rounded_rect extrude round their edges), text, threads and sweeps. Say so plainly, and offer the closest thing that works.

## Parts you may change later

Build a part the user may want to adjust as a recipe: `build {recipe: [{command: "add_primitive", params: {kind: "box", size:
[20, 20, 10]}}, {command: "add_primitive", params: {kind: "cylinder", radius: 3, height: 14}}, {command: "move", params: {id: 2,
by: [0, 0, -2]}}, {command: "difference", params: {id: 1, minus: [2]}}]}`. The steps run on a plate of their own, where ids count
from 1 in the order objects are made, and must end with one object. The object keeps its recipe (`inspect_scene` shows it). To
change it ("make the hole 6 mm"), edit the step instead of building it again: `edit {id, step: 1, params: {radius: 3}}`. The part is
rebuilt where it stands.

## Work in this order

1. Look first: `inspect_scene` before you change anything.
2. If a missing number would make the part meaningless (the diameter of the rod a holder must grip), ask one question. Otherwise
   do not interview the user: choose defensible values, keep a list of every value you chose, and hand the list back.
3. Make it, one change at a time.
4. Check with numbers before you describe it. `inspect_scene` gives size, min, max, volume and whether the mesh is closed. The
   size must match what was asked, `watertight` must be true, and the object must sit on the plate (min z = 0). An imported mesh
   is mended on the way in; if `open_edges`, `nonmanifold_edges` or `flipped_faces` is still above 0, run `repair {id}` and read
   what it says. A mesh that stays open has a hole too big to fill without guessing: tell the user. An imported scan of hundreds of
   thousands of triangles slices and draws slowly: `simplify {id, ratio: 0.2}` keeps its shape with a fifth of them.
5. `slice_check` says whether it fits the plate and how long it will take. A model that fails it is not done.
6. Report in this order: what is on the plate; the values you chose, one per line, so a wrong one is easy to spot; anything the
   request needed that Lamina cannot do yet. Then stop and let the user correct you.

## Mistakes to avoid

- Overlapping objects without joining them: they print twice. `union` them, or keep them apart (`arrange` does).
- Stacking objects to make a taller one. A taller single shape is one `scale` away.
- Moving something below z = 0. `drop_to_bed` puts it back.
- Calling a result ready to print. Nothing here has been tested on a printer. Say what was checked (closed mesh, fits the plate,
  estimated time) and what was not.
