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

## Flow (one wall, measured)

A box printed as a single wall shows whether the extruder pushes what the slicer asks.

1. `add_primitive {kind: "box", size: [20, 20, 15]}` and slice it with `vase: true` and `bottom_layers: 1`.
2. The user measures the wall at several places. It should be the line width, which is the nozzle's diameter (0.4 mm for a 0.4
   nozzle).
3. `flow_ratio` (a filament field, 1.0) multiplies every line's plastic: a wall 3 % too thick wants `flow_ratio` 0.97, one 3 %
   too thin 1.03. Change it in steps of no more than 0.05, and print the box again to check.

## Temperature

Lamina cannot change the temperature part way up a print (no temperature tower), so print the same small part at a few
temperatures, one print each.

1. A part with an overhang and a bridge: `add_primitive {kind: "box", size: [30, 10, 10]}` is enough for looks and layer bonding.
2. Slice it at the filament's middle temperature, then 10 C either side: `slice_check {filament: "PETG", profile: {nozzle_temp: 230}}`.
3. The user compares: too cold shows weak layers that snap apart and a dull surface; too hot shows stringing, blobs and droopy
   overhangs. Take the coolest temperature that still bonds well.

## Retraction (two towers)

1. `add_primitive {kind: "cylinder", radius: 4, height: 30, at: [-20, 0]}` and
   `add_primitive {kind: "cylinder", radius: 4, height: 30, at: [20, 0]}`.
2. Print with `combing` off so every travel between them retracts, starting at `retract_length` 0.8 (direct drive) or 4
   (Bowden), and raise it by 0.4 or 1 per print until the hairs between the towers stop.
3. Then lower `nozzle_temp` by 5 C if hairs remain. Too much retraction grinds the filament or leaves gaps where printing starts again.

## Pressure advance

On Klipper and Marlin with linear advance, `pressure_advance` (a filament field) sharpens corners. Lamina has no test pattern for
it: suggest the firmware's own tuning (Klipper's tuning tower, Marlin's K factor test), then put the value they find in the
filament's `pressure_advance`.

## Speed and acceleration

`max_acceleration` and `junction_deviation` only make the time estimate right; they are not sent to the printer. Set them to what
the printer's firmware uses, so `print_time` from `slice_check` is close.

## Afterwards

Sum up what changed and why, and suggest a name for the user to save the settings as their own filament or printer preset.
