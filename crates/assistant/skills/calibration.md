---
name: calibration
description: How to tune a printer and a filament with Lamina: test parts to make, what to measure, and which setting each result changes. Read when the user asks to calibrate or a fault points at flow, temperature or retraction.
---
# Calibration

Each test is a small part you build with commands and the user prints and measures. Clear the plate only if the user agrees, build
the part, check it with `slice_check`, and tell the user what to look at and to report back. Change one setting per test. Settings
that turn out right belong in the user's own filament or printer preset, which the user saves in the app.

## First, the size of things (a 20 mm cube)

`add_primitive {kind: "box", size: [20, 20, 20]}`, printed with the user's usual settings. The user measures x, y and z with
calipers.

- Within 0.1 to 0.2 mm in x and y is normal.
- Wrong in z by a fixed amount: the first layer is squashed or high (the printer's z offset, not a setting here).
- All three off by the same percentage: the printer's steps per mm, which are set in the printer, not in Lamina.

## Temperature, retraction and pressure advance: towers

`calibration {kind}` puts a part on the plate whose settings change with height, as one undo step, and returns its `steps`
(each height band and its value) and `look` (what to judge). Ask before clearing the plate; slice it with the user's own printer
and filament (`slice_check` to check), and have them read the result back to you as a height.

- `calibration {kind: "temperature", from: 230, to: 190, step: 5}`: blocks 8 mm tall, each with a 45 degree overhang, the hottest
  at the bottom. The coolest block that still bonds well (the user tries to snap it), with a clean overhang and little
  stringing, is the filament's `nozzle_temp`. Centre `from` and `to` on the label's range.
- `calibration {kind: "retraction", from: 0.4, to: 2.0, step: 0.4}`: two towers; each 5 mm band retracts its own length (use from
  2 to 6 by 1 on a Bowden printer). The lowest band without hairs between the towers is `retract_length`; then lower `nozzle_temp`
  by 5 C if a few hairs remain. Too much retraction grinds the filament.
- `calibration {kind: "pressure_advance", from: 0, to: 0.1, step: 0.005}`: a hollow square tower, one value a millimetre (for
  Bowden try to 1.0 by 0.05). The height where the corners are sharpest, neither bulging nor rounded, gives the value:
  `pressure_advance` is `from` plus that height in mm times `step`. Needs Klipper, or Marlin with linear advance.

## Flow (blocks, then one wall)

1. `calibration {kind: "flow", from: 0.9, to: 1.1, step: 0.05}`: five blocks, left to right, each printed at its own
   `flow_ratio`. The block with a smooth, closed top and no ridges gives the ratio.
2. To check it: `add_primitive {kind: "box", size: [20, 20, 15]}` sliced with `vase: true` and `bottom_layers: 1`; the user measures
   the wall at several places. It should be the line width (0.45 mm for a 0.4 nozzle). A wall 3 % too thick wants the ratio
   times 0.97.

## Speed and acceleration

`max_acceleration` and `junction_deviation` only make the time estimate right; they are not sent to the printer. Set them to what
the printer's firmware uses, so `print_time` from `slice_check` is close.

## Afterwards

Sum up what changed and why, and suggest a name for the user to save the settings as their own filament or printer preset.
