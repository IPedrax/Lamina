# Roadmap

Ordered by what unblocks the next thing. Each step ends with a test that looks at output (volumes, layer counts, G-code lines).

## Done

- Command engine with rollback, undo, scripts, a catalog agents read, and read-only access.
- Meshes, primitives (box, cylinder, sphere), transforms, STL in and out.
- Slicer: layers, walls, solid and sparse infill, skirt, retraction, flow, time and filament estimates, Marlin-style G-code.
- CLI and MCP server. AI chat engine ported from BIMCraft (providers, sealed key, attachments, tool layer).

## Next

1. **Real prints.** Run G-code on two printers (one Marlin, one Klipper) before trusting any number. Nothing here has touched hardware.
2. **Mesh repair and import.** Weld, fix winding, report non-manifold edges; 3MF and OBJ readers (lift `mesh_in.rs` from BIMCraft).
3. **Slicer quality.** Connect infill lines instead of retracting per line, seam placement, gyroid and grid infill, brim, cooling per layer time.
4. **Profiles.** Printer, filament and process presets with inheritance; Prusa-style `{expr}` placeholders; import Orca and Prusa profiles.
5. **Send to printer.** Moonraker, then OctoPrint (one multipart POST each), as a separate crate that only the desktop and CLI use.
6. **Desktop.** egui window with the chat palette, an orbit viewport (lift the camera and mesh pass from BIMCraft's `orbit.rs` and
   `gpu.rs`), a move/rotate/scale gizmo, a G-code preview with a layer slider.
7. **AI modelling loop.** A text part format the assistant edits (parametric CSG), then slice and read back checks (watertight, bounds, time).
   Needs the boolean decision (Manifold or `csgrs`). Make a chat turn one undo step.

## Later

Supports (normal, then tree), bridging, raft, adaptive layers, arc fitting, pressure advance per firmware, variable-width walls,
per-object settings, sketch and extrude, hollow and cut, text and emboss, multi-material, PrusaLink, Duet and Bambu, STEP.

## Known limits today

- Walls are constant width, so thin features vanish instead of getting gap fill. No supports, so overhangs print in the air. No bridging.
- Infill lines are separate strokes. The skirt is an outline of the first layer, not a hull. Time is distance over speed, without acceleration.
- A mesh that is not closed prints with parts missing, with a warning. STL only.
