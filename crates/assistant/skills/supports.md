---
name: supports
description: When a part needs supports, how to avoid them by turning or splitting it, and Lamina's support settings. Read before turning supports on or when a part has overhangs or bridges.
---
# Supports

Supports cost time and filament and leave marks where they touch. Try, in order: turn the part, change the model, split it, and
only then add supports.

## Does it need them

- A surface leaning more than about 45 degrees from vertical sags without support (`support_angle`, 45 by default, is that limit).
- A flat span between two walls bridges up to about 10 mm; longer, it sags.
- An overhang with nothing at one end (a ledge, a T's arms) never bridges, whatever its length: it needs support or another orientation.
- A hole with its axis level: its top is an overhang. Up to about 6 mm across it prints acceptably; larger, make it a teardrop
  (a point at the top) in the model.

`inspect_scene` shows each part's size and position; `preview {elevation: 10}` from the side shows what hangs over air.

## Turn it first

Put the largest flat face on the plate. `rotate {id, axis: "x", degrees: 90}` turns a part about its centre and sets it back on
the plate; the user can also click a face with the window's Lay flat tool. A part printed on its side is often stronger too, when
the load then runs along the layers.

## Change the model

- A chamfer of 45 degrees under an edge prints where a round fillet would sag: use chamfers on downward faces.
- Split a part that cannot sit flat: `cut {id, z: 12}` cuts it at a height into two parts, each turned onto its flat cut face; the
  user glues or screws them together. Say where you cut and why.

## Lamina's support settings

- `support`: `"off"` (default), `"everywhere"`, or `"from_plate"` (only where the column under the overhang reaches the plate, so
  nothing is built on the part itself; easier to remove, may leave some overhangs unsupported).
- `support_angle`: degrees from vertical beyond which a surface is held up. Raise it (50 to 60) for fewer supports on a printer
  that bridges well.
- `support_density`: 0.1 to 0.2. Higher holds better and is harder to remove.
- `support_z_gap`: the space between the support's top and the part (one layer, 0.2 mm, by default). Larger peels more easily but
  leaves a rougher underside.
- `support_xy_gap`: the space beside the part's walls (0.6 mm).
- `support_interface_layers`: denser layers at the top of the support (2) for a cleaner underside.

Try them: `slice_check {profile: {support: "from_plate", support_angle: 50}}`. In the result, `features` gives the filament the
support and its interface take: a large share means another orientation is worth trying.

## One part supported, the others not

`set_object_settings {id, profile: {support: "everywhere"}}` turns supports on for one object only.

## A raft or a brim is not a support

A brim (`brim_width`) holds the first layer's edges down and a raft (`raft_layers`) lifts the part onto a few coarse layers; neither
holds up an overhang.

## What is not there yet

Tree supports, painted support areas and support blockers. Say so if the user asks for one, and use orientation, `cut` or
`set_object_settings` instead.
