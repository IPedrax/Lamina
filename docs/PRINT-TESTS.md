# Print tests

Roadmap item 0.2: three small parts printed on one Marlin printer and one Klipper printer, measured with calipers, before Lamina is
trusted with real work. Nothing on this page has been printed yet. The tables are for the results, filled in at the printer, with
photos. Until they are, no claim about how a Lamina print comes out is made anywhere in this repository.

The parts are built by Lamina's own commands, in scripts in [print-tests/](print-tests/), and sliced with the built-in presets.
`crates/slicer/tests/print_tests.rs` builds and slices each, so the commands below keep working as the code changes; it checks the
G-code (sizes, layers, one wall, where the bridges are), never the print.

## Before you print

- Use a printer that prints well with the slicer it came with, so a fault seen here is Lamina's. Note its firmware and version.
- PLA, dried if it has been open long, at the temperature its label gives. The commands use Lamina's PLA preset (210 C, first layer
  215 C, bed 60 C); change `nozzle_temp` in `profile` if the label says otherwise, and write down what you used.
- The presets are generic: Generic 220 mm, Marlin (direct drive, 0.8 mm retraction; use Generic 220 mm, Marlin, Bowden for a
  Bowden tube), and Generic 300 mm, Klipper (its start and end G-code call `PRINT_START BED=... EXTRUDER=...` and `PRINT_END`, so
  your Klipper config needs those macros). If your printer differs (bed size, start G-code, retraction), add the fields to `profile` in
  the command, or save your own printer preset in the window, and write down what you changed.
- Run from the repository's root, with `lamina-cli` built (`cargo build --release -p lamina-cli`, then
  `target/release/lamina-cli`). Each command builds the part and writes the G-code beside you.

## 1. The 20 mm cube

What it checks: the size of things in x, y and z, the first layer, the seam.

Marlin:

```bash
lamina-cli run --script docs/print-tests/cube.txt --cmd 'slice {"path": "cube-marlin.gcode", "printer": "Generic 220 mm, Marlin", "filament": "PLA", "process": "0.20 mm Standard"}'
```

Klipper:

```bash
lamina-cli run --script docs/print-tests/cube.txt --cmd 'slice {"path": "cube-klipper.gcode", "printer": "Generic 300 mm, Klipper", "filament": "PLA", "process": "0.20 mm Standard"}'
```

Measure each side at its middle, away from the first layers (elephant's foot) and the seam (at the back, aligned). Expected:
20.00 mm. Within 0.1 to 0.2 mm in x and y is normal; z out by a fixed amount is the first layer's height (the printer's z offset);
all three out by one percentage is the printer's steps per mm.

| Printer | X, mm | Y, mm | Z, mm | First layer (smooth, squashed, gaps) | Seam | Time printed / estimated | Photo |
|---|---|---|---|---|---|---|---|
| Marlin | | | | | | / | |
| Klipper | | | | | | / | |

## 2. The single-wall box

What it checks: the flow, that the extruder pushes what the G-code asks. Vase mode prints the bottom, then one wall climbing
without a stop, so the wall is one line: Lamina's line width, 1.125 times the nozzle, 0.45 mm for a 0.4 mm nozzle.

Marlin:

```bash
lamina-cli run --script docs/print-tests/single-wall-box.txt --cmd 'slice {"path": "wall-marlin.gcode", "printer": "Generic 220 mm, Marlin", "filament": "PLA", "process": "0.20 mm Standard", "profile": {"vase": true, "bottom_layers": 1}}'
```

Klipper:

```bash
lamina-cli run --script docs/print-tests/single-wall-box.txt --cmd 'slice {"path": "wall-klipper.gcode", "printer": "Generic 300 mm, Klipper", "filament": "PLA", "process": "0.20 mm Standard", "profile": {"vase": true, "bottom_layers": 1}}'
```

Measure the wall at the middle of each side, halfway up, with the calipers' jaws flat on it. Expected: 0.45 mm. Lamina has no
flow multiplier: a wall 3 % too thick is fixed by raising `filament_diameter` from 1.75 to about 1.776 (1.75 times the square root
of 1.03), one too thin by lowering it the same way (the calibration skill says so too).

| Printer | Front, mm | Back, mm | Left, mm | Right, mm | Mean, mm | Gaps or blobs | Photo |
|---|---|---|---|---|---|---|---|
| Marlin | | | | | | | |
| Klipper | | | | | | | |

## 3. The bridge test

What it checks: bridges of 10, 20 and 30 mm (a bar 85 mm long and 10 mm tall, three windows 7 mm tall under a 2 mm top), printed
at `bridge_speed` (25 mm/s) with `bridge_flow` 1.0 and the fan full. Slice it as it is, with no supports: the windows are the test.

Marlin:

```bash
lamina-cli run --script docs/print-tests/bridge.txt --cmd 'slice {"path": "bridge-marlin.gcode", "printer": "Generic 220 mm, Marlin", "filament": "PLA", "process": "0.20 mm Standard"}'
```

Klipper:

```bash
lamina-cli run --script docs/print-tests/bridge.txt --cmd 'slice {"path": "bridge-klipper.gcode", "printer": "Generic 300 mm, Klipper", "filament": "PLA", "process": "0.20 mm Standard"}'
```

Look under each bridge and measure how far its middle hangs below the window's top edge (a depth gauge, or the calipers' tail
against the base: the window is 7.00 mm tall where it does not sag). Note strands that broke or curled.

| Printer | 10 mm: window height at the middle, mm | 20 mm | 30 mm | Strands broken or curled | Top surface over the bridges | Photo |
|---|---|---|---|---|---|---|
| Marlin | | | | | | |
| Klipper | | | | | | |

## Afterwards

- Name photos `cube-marlin.jpg`, `wall-klipper.jpg` and so on, and put them in `docs/images/print-tests/`.
- Keep the G-code files that were printed, or say which commit of Lamina made them (`lamina-cli --version` and the commit).
- A result far from the expected one is an issue to fix before more features (the roadmap's rule for 0.2): open one with the table
  row, the photo and the G-code.
